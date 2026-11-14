use std::ffi::OsStr;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

const ENVIRONMENT: [(&str, &str); 6] = [
    ("HOMEBREW_NO_AUTO_UPDATE", "1"),
    ("HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK", "1"),
    ("HOMEBREW_NO_ENV_HINTS", "1"),
    ("HOMEBREW_NO_COLOR", "1"),
    ("NONINTERACTIVE", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

const USUAL_PLACES: [&str; 3] = ["/opt/homebrew/bin", "/home/linuxbrew/.linuxbrew/bin", "/usr/local/bin"];

pub fn find(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let usual = USUAL_PLACES.iter().map(PathBuf::from);
    std::env::split_paths(&path).chain(usual).map(|dir| dir.join(program)).find(|candidate| candidate.is_file())
}

pub fn run(program: impl AsRef<OsStr>, args: &[&str], timeout: Duration) -> Result<String, String> {
    let name = program.as_ref().to_string_lossy().to_string();
    let child = Command::new(program.as_ref())
        .args(args)
        .envs(ENVIRONMENT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("could not start {name}: {error}"))?;
    let group = format!("-{}", child.id());
    let (finished, waiting) = mpsc::channel::<()>();
    let watchdog = thread::spawn(move || {
        let timed_out = waiting.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout);
        if timed_out {
            let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
        }
        timed_out
    });
    let output = child.wait_with_output();
    let _ = finished.send(());
    let timed_out = watchdog.join().unwrap_or(false);
    if timed_out {
        return Err(format!("{name} {} did not finish in {}", args.join(" "), humantime::format_duration(timeout)));
    }
    let output = output.map_err(|error| format!("{name} did not report a result: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Err(format!("{name} {}: {}", args.join(" "), tail(&stderr, &stdout)))
}

fn tail(stderr: &str, stdout: &str) -> String {
    let source = if stderr.trim().is_empty() { stdout } else { stderr };
    let lines: Vec<&str> = source.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let kept = lines.iter().skip(lines.len().saturating_sub(3)).copied().collect::<Vec<_>>().join(" | ");
    if kept.is_empty() { "failed with no output".to_string() } else { kept }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_what_the_command_printed() {
        assert_eq!(run("sh", &["-c", "echo hello"], Duration::from_secs(5)), Ok("hello\n".to_string()));
    }

    #[test]
    fn a_failure_carries_the_last_lines_of_its_error_output() {
        let error = run("sh", &["-c", "echo one >&2; echo two >&2; exit 3"], Duration::from_secs(5)).unwrap_err();
        assert!(error.ends_with("one | two"), "{error}");
    }

    #[test]
    fn a_command_that_outlives_its_timeout_is_killed_and_reported() {
        let error = run("sh", &["-c", "sleep 30"], Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("did not finish in 200ms"), "{error}");
    }

    #[test]
    fn a_missing_program_is_reported_by_name() {
        let error = run("sth-update-no-such-program", &[], Duration::from_secs(1)).unwrap_err();
        assert!(error.starts_with("could not start sth-update-no-such-program"), "{error}");
    }
}
