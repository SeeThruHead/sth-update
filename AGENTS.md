# sth-update

A small Rust daemon that keeps the tools from one Homebrew tap current and restarts their services. The README says what it does for a user; this file is for working on it.

## Commands

- `cargo test` runs the unit tests.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` before a commit.
- `cargo build --release` builds `target/release/sth-update`.

## How it starts

`main.rs` dispatches one command. `run` is the daemon: load the config, run one pass, sleep for the interval, repeat. `now` runs one pass. Both go through `update`, which takes the lock, runs `pass::run`, logs, notifies and saves the state.

## Modules

- `exec.rs`: the only place a process is started. Every command has a timeout, no stdin, its own process group, and the environment that stops brew updating everything.
- `brew.rs`: what is asked of brew (installed, outdated, upgrade, services) and the parsing of its output.
- `services.rs`: finds the user units that run a tool, decides what to restart (`plan`, pure) and performs it. The only code that differs by OS.
- `shadow.rs`: finds dev builds ahead of a brew binary.
- `pass.rs`: one update run, top to bottom, producing a `Report`.
- `state.rs`: the lock, `state.json` and the log.
- `notify.rs`: tells the person about failures.
- `config.rs`: the optional config and the tools that are never restarted.

## Rules

- No comments, no `for`/`while`, no mutation; iterators and guard clauses. The daemon's `loop` is the one exception.
- Never add a restart of herdr, herdr-core, herdr-app or hc, or any machine-wide limiter or priority change.
- Only the tap is refreshed and only the tap's tools are upgraded.
- Tests assert what a caller sees (parsed output, the plan, the report), not how it got there.

## Release

Bump `version` in `Cargo.toml`, commit, tag `v<version>`, push the tag, then update `url` and `sha256` in the tap's `Formula/sth-update.rb`.
