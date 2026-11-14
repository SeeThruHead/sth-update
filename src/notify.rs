use crate::exec;
use std::path::PathBuf;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
const LONGEST: usize = 400;
const TITLE: &str = "sth-update";

pub fn machine() -> String {
    exec::run("hostname", &[], TIMEOUT)
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "this machine".to_string())
}

pub fn message(machine: &str, failures: &[String]) -> String {
    let full = format!("{machine}: {}", failures.join("; "));
    if full.chars().count() <= LONGEST {
        return full;
    }
    let cut: String = full.chars().take(LONGEST).collect();
    format!("{cut}... (sth-update status has the rest)")
}

fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn command(configured: &[String], seat: Option<PathBuf>, text: &str) -> Vec<String> {
    if !configured.is_empty() {
        return configured.iter().cloned().chain([text.to_string()]).collect();
    }
    if let Some(seat) = seat {
        return vec![seat.display().to_string(), "notify".into(), text.into(), "--title".into(), TITLE.into()];
    }
    if cfg!(target_os = "macos") {
        let script = format!("display notification \"{}\" with title \"{TITLE}\"", quoted(text));
        return vec!["osascript".into(), "-e".into(), script];
    }
    vec!["notify-send".into(), TITLE.into(), text.into()]
}

pub fn send(configured: &[String], text: &str) -> Result<(), String> {
    let command = command(configured, exec::find("sth-seat"), text);
    let args: Vec<&str> = command.iter().skip(1).map(String::as_str).collect();
    exec::run(&command[0], &args, TIMEOUT).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_the_machine_and_every_problem() {
        assert_eq!(message("sthdev", &["a failed".into(), "b failed".into()]), "sthdev: a failed; b failed");
    }

    #[test]
    fn a_long_message_is_cut_and_points_at_status() {
        let text = message("sthdev", &["x".repeat(900)]);
        assert!(text.ends_with("... (sth-update status has the rest)"));
        assert!(text.chars().count() < 450);
    }

    #[test]
    fn a_configured_command_gets_the_message_as_its_last_argument() {
        let built = command(&["mail".to_string(), "me".to_string()], Some(PathBuf::from("/x/sth-seat")), "broken");
        assert_eq!(built, vec!["mail", "me", "broken"]);
    }

    #[test]
    fn sth_seat_is_used_where_it_exists() {
        let built = command(&[], Some(PathBuf::from("/x/sth-seat")), "broken");
        assert_eq!(built, vec!["/x/sth-seat", "notify", "broken", "--title", "sth-update"]);
    }

    #[test]
    fn quotes_in_a_message_cannot_end_the_notification_script() {
        assert_eq!(quoted("say \"hi\" \\ bye"), "say \\\"hi\\\" \\\\ bye");
    }
}
