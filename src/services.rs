use crate::brew::{Brew, Service};
use crate::config::{Mode, Rule};
use crate::exec;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub id: String,
    pub brew_service: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    BrewService(String),
    Unit(String),
    Command(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Nothing,
    Deferred(Vec<String>),
    Restart(Vec<Action>),
}

pub struct Platform {
    pub mac: bool,
    pub dir: PathBuf,
    pub domain: String,
}

impl Platform {
    pub fn current(home: &Path) -> Platform {
        if !cfg!(target_os = "macos") {
            return Platform { mac: false, dir: home.join(".config/systemd/user"), domain: String::new() };
        }
        let uid = exec::run("id", &["-u"], TIMEOUT).unwrap_or_default();
        Platform { mac: true, dir: home.join("Library/LaunchAgents"), domain: format!("gui/{}", uid.trim()) }
    }

    fn extension(&self) -> &'static str {
        if self.mac { "plist" } else { "service" }
    }

    fn files(&self) -> Vec<PathBuf> {
        let entries = std::fs::read_dir(&self.dir).into_iter().flatten().flatten().map(|entry| entry.path());
        entries.filter(|path| path.extension().is_some_and(|extension| extension == self.extension())).collect()
    }

    fn unit(&self, file: &Path, needles: &[String], services: &[Service]) -> Option<Unit> {
        let text = std::fs::read_to_string(file).ok()?;
        let searched = if self.mac { text.clone() } else { systemd_exec(&text) };
        if !needles.iter().any(|needle| mentions(&searched, needle)) {
            return None;
        }
        let id = if self.mac { plist_label(&text)? } else { file.file_name()?.to_string_lossy().to_string() };
        let brew_service =
            services.iter().find(|service| same_file(&service.file, file)).map(|service| service.name.clone());
        Some(Unit { id, brew_service })
    }

    pub fn units(&self, brew: &Brew, tool: &str, services: &[Service]) -> Vec<Unit> {
        let needles = needles(&brew.prefix, tool, &brew.binaries(tool));
        self.files().iter().filter_map(|file| self.unit(file, &needles, services)).collect()
    }

    pub fn active(&self, unit: &Unit) -> bool {
        if self.mac {
            let printed = exec::run("launchctl", &["print", &format!("{}/{}", self.domain, unit.id)], TIMEOUT);
            return printed.is_ok_and(|description| launchd_running(&description));
        }
        exec::run("systemctl", &["--user", "is-active", "--quiet", &unit.id], TIMEOUT).is_ok()
    }

    pub fn perform(&self, brew: &Brew, action: &Action) -> Result<String, String> {
        match action {
            Action::BrewService(name) => brew.restart_service(name).map(|_| format!("{name} (brew service)")),
            Action::Unit(id) if self.mac => {
                exec::run("launchctl", &["kickstart", "-k", &format!("{}/{id}", self.domain)], TIMEOUT)
                    .map(|_| id.clone())
            }
            Action::Unit(id) => exec::run("systemctl", &["--user", "restart", id], TIMEOUT).map(|_| id.clone()),
            Action::Command(command) => run_command(brew, command),
        }
    }
}

fn run_command(brew: &Brew, command: &[String]) -> Result<String, String> {
    let Some((program, args)) = command.split_first() else { return Err("the restart command is empty".to_string()) };
    let from_brew = brew.prefix.join("bin").join(program);
    let resolved = if !program.contains('/') && from_brew.is_file() { from_brew } else { PathBuf::from(program) };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    exec::run(&resolved, &args, TIMEOUT).map(|_| command.join(" "))
}

fn same_file(left: &Path, right: &Path) -> bool {
    left == right || left.canonicalize().ok().is_some_and(|real| right.canonicalize().ok() == Some(real))
}

fn is_name_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '-' | '_' | '.')
}

pub fn mentions(text: &str, needle: &str) -> bool {
    let open_ended = needle.ends_with('/');
    text.match_indices(needle)
        .any(|(at, _)| open_ended || !text[at + needle.len()..].chars().next().is_some_and(is_name_char))
}

pub fn needles(prefix: &Path, tool: &str, binaries: &[String]) -> Vec<String> {
    let prefix = prefix.display();
    let linked = binaries.iter().map(|binary| format!("{prefix}/bin/{binary}"));
    std::iter::once(format!("{prefix}/opt/{tool}/")).chain(linked).collect()
}

pub fn systemd_exec(text: &str) -> String {
    text.lines().filter(|line| line.trim_start().starts_with("ExecStart")).collect::<Vec<_>>().join("\n")
}

pub fn launchd_running(description: &str) -> bool {
    description.lines().any(|line| line.trim() == "state = running")
}

pub fn plist_label(text: &str) -> Option<String> {
    let after_key = text.split_once("<key>Label</key>")?.1;
    let value = after_key.split_once("<string>")?.1;
    Some(value.split_once("</string>")?.0.trim().to_string())
}

fn action(unit: &Unit) -> Action {
    unit.brew_service.clone().map(Action::BrewService).unwrap_or_else(|| Action::Unit(unit.id.clone()))
}

pub fn plan(rule: &Rule, active: &[Unit]) -> Plan {
    match rule {
        Rule::Command(command) => Plan::Restart(vec![Action::Command(command.clone())]),
        _ if active.is_empty() => Plan::Nothing,
        Rule::Mode(Mode::Never) => Plan::Deferred(active.iter().map(|unit| unit.id.clone()).collect()),
        Rule::Mode(Mode::Auto) => Plan::Restart(active.iter().map(action).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "/home/linuxbrew/.linuxbrew";

    fn seat_needles() -> Vec<String> {
        needles(Path::new(PREFIX), "sth-seat", &["sth-seat".to_string()])
    }

    #[test]
    fn a_unit_that_runs_the_linked_binary_mentions_the_tool() {
        let unit = "[Service]\nExecStart=/home/linuxbrew/.linuxbrew/bin/sth-seat daemon\n";
        assert!(seat_needles().iter().any(|needle| mentions(&systemd_exec(unit), needle)));
    }

    #[test]
    fn a_unit_that_runs_anything_under_the_tools_opt_folder_mentions_it() {
        let unit = "ExecStart=\"/home/linuxbrew/.linuxbrew/opt/sth-seat/libexec/helper\" \"run\"";
        assert!(seat_needles().iter().any(|needle| mentions(unit, needle)));
    }

    #[test]
    fn a_longer_binary_name_or_a_path_entry_is_not_a_mention() {
        let other = "ExecStart=/home/linuxbrew/.linuxbrew/bin/sth-seat-helper\n";
        let path_only = "[Service]\nEnvironment=\"PATH=/home/linuxbrew/.linuxbrew/bin/sth-seat\"\nExecStart=/usr/bin/herdr server\n";
        assert!(!seat_needles().iter().any(|needle| mentions(&systemd_exec(other), needle)));
        assert!(!seat_needles().iter().any(|needle| mentions(&systemd_exec(path_only), needle)));
    }

    #[test]
    fn a_launch_agent_is_matched_by_its_program_and_named_by_its_label() {
        let plist = "<dict>\n<key>Label</key>\n<string>sth-seat</string>\n<key>ProgramArguments</key>\n<array><string>/opt/homebrew/bin/sth-seat</string><string>daemon</string></array></dict>";
        let wanted = needles(Path::new("/opt/homebrew"), "sth-seat", &["sth-seat".to_string()]);
        assert!(wanted.iter().any(|needle| mentions(plist, needle)));
        assert_eq!(plist_label(plist), Some("sth-seat".to_string()));
    }

    #[test]
    fn a_launch_agent_counts_as_running_only_when_launchd_says_so() {
        assert!(launchd_running("gui/501/sh.brew.nudge = {\n\tactive count = 1\n\tstate = running\n}"));
        assert!(!launchd_running("gui/501/sh.brew.forkyard-bar = {\n\tstate = not running\n\tlast exit code = 0\n}"));
    }

    #[test]
    fn a_tool_with_no_running_service_needs_nothing() {
        assert_eq!(plan(&Rule::Mode(Mode::Auto), &[]), Plan::Nothing);
        assert_eq!(plan(&Rule::Mode(Mode::Never), &[]), Plan::Nothing);
    }

    #[test]
    fn brew_services_restart_through_brew_and_other_units_directly() {
        let managed = Unit { id: "sh.brew.nudge.service".into(), brew_service: Some("nudge".into()) };
        let own = Unit { id: "sth-seat.service".into(), brew_service: None };
        let wanted = Plan::Restart(vec![Action::BrewService("nudge".into()), Action::Unit("sth-seat.service".into())]);
        assert_eq!(plan(&Rule::Mode(Mode::Auto), &[managed, own]), wanted);
    }

    #[test]
    fn a_tool_that_must_not_restart_is_deferred_while_its_service_runs() {
        let core = Unit { id: "herdr-core.service".into(), brew_service: None };
        assert_eq!(plan(&Rule::Mode(Mode::Never), &[core]), Plan::Deferred(vec!["herdr-core.service".to_string()]));
    }

    #[test]
    fn a_configured_command_replaces_the_usual_restart() {
        let command = vec!["sth-seat".to_string(), "install".to_string()];
        assert_eq!(plan(&Rule::Command(command.clone()), &[]), Plan::Restart(vec![Action::Command(command)]));
    }
}
