use crate::exec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
const TAP_TIMEOUT: Duration = Duration::from_secs(90);
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const SERVICE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Formula,
    Cask,
}

impl Kind {
    fn flag(self) -> &'static str {
        match self {
            Kind::Formula => "--formula",
            Kind::Cask => "--cask",
        }
    }

    fn folders(self) -> &'static [&'static str] {
        match self {
            Kind::Formula => &["Formula", "HomebrewFormula", ""],
            Kind::Cask => &["Casks"],
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Kind::Formula => "formula",
            Kind::Cask => "cask",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Kind::Formula => "formulae",
            Kind::Cask => "casks",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outdated {
    pub tool: String,
    pub kind: Kind,
    pub from: String,
    pub to: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub name: String,
    pub file: PathBuf,
}

pub struct Brew {
    pub bin: PathBuf,
    pub prefix: PathBuf,
}

fn binary() -> Option<PathBuf> {
    std::env::var_os("STH_UPDATE_BREW").map(PathBuf::from).or_else(|| exec::find("brew"))
}

pub fn short_name(full: &str) -> String {
    full.rsplit('/').next().unwrap_or(full).to_string()
}

pub fn in_tap(listing: &str, tap: &str) -> Vec<String> {
    let prefix = format!("{}/", tap.to_lowercase());
    listing.lines().map(str::trim).filter(|line| line.to_lowercase().starts_with(&prefix)).map(short_name).collect()
}

pub fn offers(repo: &Path, kind: Kind, tool: &str) -> bool {
    kind.folders().iter().any(|folder| repo.join(folder).join(format!("{tool}.rb")).is_file())
}

fn versions(value: &Value) -> String {
    match value {
        Value::Array(all) => all.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Value::String(one) => one.clone(),
        _ => String::new(),
    }
}

fn outdated_entry(entry: &Value, kind: Kind) -> Option<Outdated> {
    Some(Outdated {
        tool: short_name(entry.get("name")?.as_str()?),
        kind,
        from: versions(entry.get("installed_versions")?),
        to: entry.get("current_version")?.as_str()?.to_string(),
        pinned: entry.get("pinned").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub fn parse_outdated(json: &str, kind: Kind) -> Result<Vec<Outdated>, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| format!("brew outdated printed something unreadable: {error}"))?;
    let entries = value.get(kind.key()).and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(entries.iter().filter_map(|entry| outdated_entry(entry, kind)).collect())
}

fn service_entry(entry: &Value) -> Option<Service> {
    Some(Service { name: entry.get("name")?.as_str()?.to_string(), file: PathBuf::from(entry.get("file")?.as_str()?) })
}

pub fn parse_services(json: &str) -> Vec<Service> {
    let entries =
        serde_json::from_str::<Value>(json).ok().and_then(|value| value.as_array().cloned()).unwrap_or_default();
    entries.iter().filter_map(service_entry).collect()
}

pub fn service_status(json: &str, name: &str) -> Option<String> {
    let entries = serde_json::from_str::<Value>(json).ok()?.as_array()?.clone();
    let entry = entries.iter().find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))?;
    Some(entry.get("status")?.as_str()?.to_string())
}

impl Brew {
    pub fn locate() -> Result<Brew, String> {
        let bin = binary().ok_or("brew was not found on PATH or in its usual places; set STH_UPDATE_BREW")?;
        let prefix = exec::run(&bin, &["--prefix"], QUERY_TIMEOUT)?;
        Ok(Brew { bin, prefix: PathBuf::from(prefix.trim()) })
    }

    pub fn tap_repo(&self, tap: &str) -> Result<PathBuf, String> {
        let repo = PathBuf::from(exec::run(&self.bin, &["--repo", tap], QUERY_TIMEOUT)?.trim());
        if !repo.join(".git").exists() {
            return Err(format!("the tap {tap} is not installed here; run brew tap {tap}"));
        }
        Ok(repo)
    }

    pub fn refresh_tap(&self, repo: &Path) -> Result<(), String> {
        let repo = repo.display().to_string();
        exec::run("git", &["-C", &repo, "pull", "--ff-only", "--quiet"], TAP_TIMEOUT).map(|_| ())
    }

    pub fn installed(&self, tap: &str, kind: Kind) -> Result<Vec<String>, String> {
        exec::run(&self.bin, &["list", kind.flag(), "--full-name"], QUERY_TIMEOUT).map(|listing| in_tap(&listing, tap))
    }

    pub fn outdated(&self, tap: &str, kind: Kind, tools: &[String]) -> Result<Vec<Outdated>, String> {
        if tools.is_empty() {
            return Ok(Vec::new());
        }
        let full: Vec<String> = tools.iter().map(|tool| format!("{tap}/{tool}")).collect();
        let args: Vec<&str> =
            ["outdated", "--json=v2", kind.flag()].into_iter().chain(full.iter().map(String::as_str)).collect();
        let finished = exec::finish(&self.bin, &args, QUERY_TIMEOUT)?;
        parse_outdated(&finished.stdout, kind)
            .map_err(|error| if finished.succeeded { error } else { finished.failure })
    }

    pub fn upgrade(&self, tap: &str, item: &Outdated) -> Result<(), String> {
        let full = format!("{tap}/{}", item.tool);
        exec::run(&self.bin, &["upgrade", item.kind.flag(), &full], UPGRADE_TIMEOUT).map(|_| ())
    }

    pub fn services(&self) -> Vec<Service> {
        exec::run(&self.bin, &["services", "list", "--json"], QUERY_TIMEOUT)
            .map(|json| parse_services(&json))
            .unwrap_or_default()
    }

    pub fn service_status(&self, name: &str) -> Option<String> {
        let json = exec::run(&self.bin, &["services", "list", "--json"], QUERY_TIMEOUT).ok()?;
        service_status(&json, name)
    }

    pub fn restart_service(&self, name: &str) -> Result<(), String> {
        exec::run(&self.bin, &["services", "restart", name], SERVICE_TIMEOUT).map(|_| ())
    }

    pub fn binaries(&self, tool: &str) -> Vec<String> {
        let entries = std::fs::read_dir(self.prefix.join("opt").join(tool).join("bin")).into_iter().flatten().flatten();
        entries.map(|entry| entry.file_name().to_string_lossy().to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_taps_own_tools_are_kept_from_a_listing() {
        let listing = "git\nseethruhead/tap/nudge\nSeeThruHead/tap/reach\noven-sh/bun/bun\n";
        assert_eq!(in_tap(listing, "seethruhead/tap"), vec!["nudge", "reach"]);
    }

    #[test]
    fn a_tool_is_offered_only_while_the_tap_still_has_its_file_for_that_kind() {
        let repo = std::env::temp_dir().join(format!("sth-update-tap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join("Formula")).unwrap();
        std::fs::create_dir_all(repo.join("Casks")).unwrap();
        std::fs::write(repo.join("Formula/forkyard-bar.rb"), "").unwrap();
        std::fs::write(repo.join("Casks/sth-dictate.rb"), "").unwrap();
        assert!(offers(&repo, Kind::Formula, "forkyard-bar"));
        assert!(!offers(&repo, Kind::Cask, "forkyard-bar"));
        assert!(offers(&repo, Kind::Cask, "sth-dictate"));
        assert!(!offers(&repo, Kind::Formula, "sth-dictate"));
    }

    #[test]
    fn outdated_formulas_carry_both_versions_and_whether_they_are_pinned() {
        let json = r#"{"formulae":[{"name":"seethruhead/tap/nudge","installed_versions":["0.9.0"],"current_version":"0.9.1","pinned":false,"pinned_version":null},{"name":"reach","installed_versions":["0.1.0","0.2.0"],"current_version":"0.3.0","pinned":true}],"casks":[]}"#;
        let found = parse_outdated(json, Kind::Formula).unwrap();
        let nudge = Outdated {
            tool: "nudge".into(),
            kind: Kind::Formula,
            from: "0.9.0".into(),
            to: "0.9.1".into(),
            pinned: false,
        };
        let reach = Outdated {
            tool: "reach".into(),
            kind: Kind::Formula,
            from: "0.1.0, 0.2.0".into(),
            to: "0.3.0".into(),
            pinned: true,
        };
        assert_eq!(found, vec![nudge, reach]);
    }

    #[test]
    fn outdated_casks_are_read_whether_the_installed_version_is_a_list_or_a_string() {
        let json = r#"{"formulae":[],"casks":[{"name":"sth-dictate","installed_versions":"1.0","current_version":"1.1"},{"name":"forkyard-bar","installed_versions":["2"],"current_version":"3"}]}"#;
        let found = parse_outdated(json, Kind::Cask).unwrap();
        assert_eq!(
            found.iter().map(|item| (item.tool.as_str(), item.from.as_str())).collect::<Vec<_>>(),
            vec![("sth-dictate", "1.0"), ("forkyard-bar", "2")]
        );
        assert!(parse_outdated(json, Kind::Formula).unwrap().is_empty());
    }

    #[test]
    fn unreadable_outdated_output_is_an_error_not_an_empty_list() {
        assert!(parse_outdated("Error: something", Kind::Formula).is_err());
    }

    #[test]
    fn a_services_status_is_read_by_name_and_missing_when_brew_does_not_list_it() {
        let json = r#"[{"name":"nudge","status":"started"},{"name":"sth-update","status":"none"}]"#;
        assert_eq!(service_status(json, "sth-update"), Some("none".to_string()));
        assert_eq!(service_status(json, "nudge"), Some("started".to_string()));
        assert_eq!(service_status(json, "reach"), None);
    }

    #[test]
    fn brew_services_are_read_with_the_file_each_one_owns() {
        let json = r#"[{"name":"nudge","status":"started","user":"sth","file":"/home/sth/.config/systemd/user/sh.brew.nudge.service","exit_code":null},{"name":"temporal","status":"none"}]"#;
        let nudge = Service {
            name: "nudge".into(),
            file: PathBuf::from("/home/sth/.config/systemd/user/sh.brew.nudge.service"),
        };
        assert_eq!(parse_services(json), vec![nudge]);
        assert!(parse_services("not json").is_empty());
    }
}
