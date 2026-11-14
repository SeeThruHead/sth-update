use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const PROTECTED: [&str; 4] = ["herdr", "herdr-core", "herdr-app", "hc"];
const SHORTEST_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Auto,
    Never,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Rule {
    Mode(Mode),
    Command(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub tap: String,
    pub every: String,
    pub casks: bool,
    pub notify: Vec<String>,
    pub shadow_dirs: Vec<String>,
    pub restart: BTreeMap<String, Rule>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            tap: "seethruhead/tap".to_string(),
            every: "1h".to_string(),
            casks: true,
            notify: Vec::new(),
            shadow_dirs: ["~/.local/bin", "~/.cargo/bin", "~/.bun/bin"].map(String::from).to_vec(),
            restart: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn interval(&self) -> Duration {
        humantime::parse_duration(&self.every)
            .map(|every| every.max(SHORTEST_INTERVAL))
            .unwrap_or(Duration::from_secs(3600))
    }

    pub fn rule(&self, tool: &str) -> Rule {
        if PROTECTED.contains(&tool) {
            return Rule::Mode(Mode::Never);
        }
        self.restart.get(tool).cloned().unwrap_or(Rule::Mode(Mode::Auto))
    }

    pub fn shadow_dirs(&self, home: &Path) -> Vec<PathBuf> {
        self.shadow_dirs.iter().map(|dir| expand(dir, home)).collect()
    }
}

fn expand(dir: &str, home: &Path) -> PathBuf {
    dir.strip_prefix("~/").map(|rest| home.join(rest)).unwrap_or_else(|| PathBuf::from(dir))
}

pub fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|error| error.to_string())?;
    humantime::parse_duration(&config.every).map_err(|error| format!("every = \"{}\": {error}", config.every))?;
    Ok(config)
}

pub fn load(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_updates_the_tap_hourly() {
        let config = parse("").unwrap();
        assert_eq!(config.tap, "seethruhead/tap");
        assert_eq!(config.interval(), Duration::from_secs(3600));
        assert_eq!(config.rule("nudge"), Rule::Mode(Mode::Auto));
    }

    #[test]
    fn a_tool_can_be_left_alone_or_given_its_own_restart_command() {
        let config =
            parse("[restart]\nsth-encode-worker = \"never\"\nsth-seat = [\"sth-seat\", \"install\"]\n").unwrap();
        assert_eq!(config.rule("sth-encode-worker"), Rule::Mode(Mode::Never));
        assert_eq!(config.rule("sth-seat"), Rule::Command(vec!["sth-seat".to_string(), "install".to_string()]));
    }

    #[test]
    fn the_programs_that_host_sessions_are_never_restarted_whatever_the_config_says() {
        let config = parse("[restart]\nherdr-core = \"auto\"\nherdr-app = [\"pkill\", \"herdr-app\"]\n").unwrap();
        assert_eq!(config.rule("herdr-core"), Rule::Mode(Mode::Never));
        assert_eq!(config.rule("herdr-app"), Rule::Mode(Mode::Never));
    }

    #[test]
    fn the_interval_never_goes_below_five_minutes() {
        assert_eq!(parse("every = \"10s\"").unwrap().interval(), Duration::from_secs(300));
    }

    #[test]
    fn a_bad_interval_or_an_unknown_key_is_refused() {
        assert!(parse("every = \"soon\"").unwrap_err().contains("soon"));
        assert!(parse("evry = \"1h\"").unwrap_err().contains("evry"));
    }

    #[test]
    fn shadow_dirs_expand_the_home_directory() {
        let dirs = parse("shadow_dirs = [\"~/bin\", \"/opt/x\"]").unwrap().shadow_dirs(Path::new("/home/a"));
        assert_eq!(dirs, vec![PathBuf::from("/home/a/bin"), PathBuf::from("/opt/x")]);
    }
}
