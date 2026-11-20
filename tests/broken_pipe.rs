use std::process::{Command, Output, Stdio};

fn into_a_closed_pipe(args: &[&str], state: &std::path::Path) -> Output {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    Command::new(env!("CARGO_BIN_EXE_sth-update"))
        .args(args)
        .env("STH_UPDATE_STATE", state)
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn state_with_a_long_log(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sth-update-pipe-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = (0..80).map(|number| format!("line {number}")).collect();
    std::fs::write(dir.join("log"), lines.join("\n")).unwrap();
    dir
}

#[test]
fn output_into_a_reader_that_has_gone_ends_quietly() {
    let state = state_with_a_long_log("gone");
    ["--help", "--version", "log", "status"].iter().for_each(|command| {
        let output = into_a_closed_pipe(&[command], &state);
        assert!(output.status.success(), "{command}: {:?}", output.status);
        assert_eq!(String::from_utf8_lossy(&output.stderr), "", "{command}");
    });
}
