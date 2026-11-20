use crate::pass::{Report, Upgraded};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

const HISTORY: usize = 30;
const LOG_LIMIT: usize = 600;
const LOG_KEPT: usize = 400;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub last: Option<Report>,
    pub history: Vec<Upgraded>,
    pub notified: String,
}

impl State {
    pub fn after(&self, report: &Report, notified: String) -> State {
        let all: Vec<Upgraded> = self.history.iter().chain(report.upgraded.iter()).cloned().collect();
        let history = all.iter().skip(all.len().saturating_sub(HISTORY)).cloned().collect();
        State { last: Some(report.clone()), history, notified }
    }
}

pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn lock(&self) -> Result<File, String> {
        fs::create_dir_all(&self.dir).map_err(|error| format!("{}: {error}", self.dir.display()))?;
        let file = File::create(self.file("lock")).map_err(|error| format!("lock: {error}"))?;
        file.lock().map_err(|error| format!("lock: {error}"))?;
        Ok(file)
    }

    pub fn load(&self) -> State {
        fs::read_to_string(self.file("state.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state: &State) -> Result<(), String> {
        let text = serde_json::to_string_pretty(state).map_err(|error| error.to_string())?;
        fs::create_dir_all(&self.dir)
            .and_then(|_| fs::write(self.file("state.json"), text))
            .map_err(|error| format!("state: {error}"))
    }

    pub fn log(&self, line: &str) {
        let stamped = format!("{} {line}", humantime::format_rfc3339_seconds(SystemTime::now()));
        crate::out::warn(&stamped);
        let _ = fs::create_dir_all(&self.dir);
        let opened = OpenOptions::new().create(true).append(true).open(self.file("log"));
        let _ = opened.and_then(|mut file| writeln!(file, "{stamped}"));
    }

    pub fn read_log(&self) -> Vec<String> {
        fs::read_to_string(self.file("log")).unwrap_or_default().lines().map(String::from).collect()
    }

    pub fn trim_log(&self) {
        let lines = self.read_log();
        if lines.len() <= LOG_LIMIT {
            return;
        }
        let kept: Vec<&str> = lines.iter().skip(lines.len() - LOG_KEPT).map(String::as_str).collect();
        let _ = fs::write(self.file("log"), format!("{}\n", kept.join("\n")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::Effect;

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("sth-update-state-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store { dir }
    }

    fn upgrade(tool: &str) -> Upgraded {
        Upgraded { at: 1, tool: tool.into(), from: "1".into(), to: "2".into(), effect: Effect::NoService }
    }

    #[test]
    fn a_saved_state_reads_back_the_same() {
        let store = store("roundtrip");
        let report =
            Report { at: 5, upgraded: vec![upgrade("nudge")], failures: vec!["boom".into()], ..Report::default() };
        let state = State::default().after(&report, "boom".into());
        store.save(&state).unwrap();
        assert_eq!(store.load(), state);
        assert_eq!(state.history, vec![upgrade("nudge")]);
    }

    #[test]
    fn a_missing_or_broken_state_file_reads_as_never_run() {
        let store = store("missing");
        assert_eq!(store.load(), State::default());
        fs::create_dir_all(&store.dir).unwrap();
        fs::write(store.dir.join("state.json"), "{ not json").unwrap();
        assert_eq!(store.load(), State::default());
    }

    #[test]
    fn history_keeps_only_the_most_recent_upgrades() {
        let many =
            Report { upgraded: (0..40).map(|number| upgrade(&format!("tool{number}"))).collect(), ..Report::default() };
        let state = State::default().after(&many, String::new());
        assert_eq!(state.history.len(), 30);
        assert_eq!(state.history.last().map(|item| item.tool.as_str()), Some("tool39"));
    }

    #[test]
    fn the_log_is_cut_back_to_its_newest_lines_once_it_grows_long() {
        let store = store("log");
        fs::create_dir_all(&store.dir).unwrap();
        let lines: Vec<String> = (0..700).map(|number| format!("line {number}")).collect();
        fs::write(store.dir.join("log"), lines.join("\n")).unwrap();
        store.trim_log();
        let kept = store.read_log();
        assert_eq!(kept.len(), 400);
        assert_eq!(kept.last().map(String::as_str), Some("line 699"));
    }
}
