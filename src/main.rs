mod brew;
mod config;
mod exec;
mod notify;
mod pass;
mod services;
mod shadow;
mod state;

use config::Config;
use pass::{Context, Report, SELF};
use services::{Action, Plan};
use state::{State, Store};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE: &str = "sth-update: keeps the tools from one Homebrew tap current on this machine

usage:
  sth-update run       the daemon: update now, then again every hour
  sth-update now       update once, now
  sth-update status    the last run: what was upgraded, what failed, dev builds in the way
  sth-update check     what is installed from the tap, what is outdated and what would restart
  sth-update log       the recent log
  sth-update --version

Each run refreshes only the tap, upgrades only the tap's installed tools, then restarts
the services that run them. herdr, herdr-core, herdr-app and hc are never restarted.

config: $STH_UPDATE_CONFIG, or ~/.config/sth-update/config.toml (optional)
state:  $STH_UPDATE_STATE, or ~/.local/state/sth-update";

struct Paths {
    home: PathBuf,
    config: PathBuf,
    store: Store,
}

fn paths() -> Paths {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let named =
        |variable: &str, usual: &str| std::env::var_os(variable).map(PathBuf::from).unwrap_or_else(|| home.join(usual));
    let config = named("STH_UPDATE_CONFIG", ".config/sth-update/config.toml");
    let store = Store { dir: named("STH_UPDATE_STATE", ".local/state/sth-update") };
    Paths { home, config, store }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0)
}

fn when(at: u64) -> String {
    let local: chrono::DateTime<chrono::Local> = (UNIX_EPOCH + Duration::from_secs(at)).into();
    local.format("%a %Y-%m-%d %H:%M").to_string()
}

fn tell(paths: &Paths, config: &Config, report: &Report, already: &str) -> String {
    let signature = report.failures.join("\n");
    if signature.is_empty() || signature == already {
        return signature;
    }
    let sent = notify::send(&config.notify, &notify::message(&notify::machine(), &report.failures));
    if let Err(error) = sent {
        paths.store.log(&format!("could not notify: {error}"));
    }
    signature
}

fn summary(report: &Report) -> String {
    match (report.upgraded.len(), report.failures.len()) {
        (0, 0) => "everything is current".to_string(),
        (upgraded, 0) => format!("{upgraded} upgraded"),
        (upgraded, failed) => format!("{upgraded} upgraded, {failed} failed"),
    }
}

fn update(paths: &Paths, config: &Config, by_daemon: bool) -> Report {
    let store = &paths.store;
    let lock = store.lock();
    if let Err(error) = &lock {
        store.log(&format!("running without the lock: {error}"));
    }
    let report = pass::run(config, &paths.home, now(), by_daemon);
    report.upgraded.iter().for_each(|item| store.log(&format!("upgraded {}", item.line())));
    report.failures.iter().for_each(|failure| store.log(&format!("failed: {failure}")));
    store.log(&summary(&report));
    let before = store.load();
    let notified = tell(paths, config, &report, &before.notified);
    if let Err(error) = store.save(&before.after(&report, notified)) {
        store.log(&format!("could not save the state: {error}"));
    }
    store.trim_log();
    report
}

fn load(paths: &Paths) -> Result<Config, String> {
    config::load(&paths.config)
}

fn daemon() -> ExitCode {
    let paths = paths();
    paths.store.log(&format!("started {}", env!("CARGO_PKG_VERSION")));
    loop {
        let config = load(&paths).unwrap_or_else(|error| {
            paths.store.log(&format!("config unreadable, using the defaults: {error}"));
            Config::default()
        });
        if update(&paths, &config, true).upgraded_self() {
            paths.store.log("upgraded itself, stopping so the service starts the new version");
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(config.interval());
    }
}

fn once() -> ExitCode {
    let paths = paths();
    let config = match load(&paths) {
        Ok(config) => config,
        Err(error) => return fail(&error),
    };
    let report = update(&paths, &config, false);
    if report.failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn section(title: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    println!("{title}:");
    lines.iter().for_each(|line| println!("  {line}"));
}

fn status() -> ExitCode {
    let State { last, history, .. } = paths().store.load();
    let Some(report) = last else {
        println!("sth-update has not run yet; sth-update now runs it");
        return ExitCode::SUCCESS;
    };
    println!("last run: {}, {}", when(report.at), summary(&report));
    section("upgraded", &report.upgraded.iter().map(pass::Upgraded::line).collect::<Vec<_>>());
    section("failed", &report.failures);
    section(
        "dev builds in the way",
        &report.shadowed.iter().map(|shadow| format!("{}: {}", shadow.tool, shadow.path)).collect::<Vec<_>>(),
    );
    section(
        "recent upgrades",
        &history.iter().rev().take(10).map(|item| format!("{}  {}", when(item.at), item.line())).collect::<Vec<_>>(),
    );
    ExitCode::SUCCESS
}

fn action_line(action: &Action) -> String {
    match action {
        Action::BrewService(name) => format!("{name} (brew service)"),
        Action::Unit(id) => id.clone(),
        Action::Command(command) => format!("by running {}", command.join(" ")),
    }
}

fn plan_line(plan: &Plan) -> String {
    match plan {
        Plan::Nothing => "no running service".to_string(),
        Plan::Deferred(units) => {
            format!("never restarted ({} keeps running the old version until its next restart)", units.join(", "))
        }
        Plan::Restart(actions) => {
            format!("restarts {}", actions.iter().map(action_line).collect::<Vec<_>>().join(", "))
        }
    }
}

fn tool_line(context: &Context, paths: &Paths, tool: &str, outdated: &[brew::Outdated]) -> String {
    let plan = if tool == SELF { "restarts itself".to_string() } else { plan_line(&context.plan(tool)) };
    let newer =
        outdated.iter().find(|item| item.tool == tool).map(|item| format!(", outdated: {} to {}", item.from, item.to));
    let shadows: Vec<String> = context.shadows(tool, &paths.home).iter().map(|shadow| shadow.path.clone()).collect();
    let shadowed =
        if shadows.is_empty() { String::new() } else { format!(", dev build in the way: {}", shadows.join(", ")) };
    format!("{tool}: {plan}{}{shadowed}", newer.unwrap_or_default())
}

fn check() -> ExitCode {
    let paths = paths();
    let config = match load(&paths) {
        Ok(config) => config,
        Err(error) => return fail(&error),
    };
    let context = match Context::open(&config, &paths.home) {
        Ok(context) => context,
        Err(error) => return fail(&error),
    };
    let installed = match context.installed() {
        Ok(installed) => installed,
        Err(error) => return fail(&error),
    };
    let (outdated, unasked) = context.outdated();
    println!(
        "brew {}, tap {}, every {}",
        context.brew.bin.display(),
        config.tap,
        humantime::format_duration(config.interval())
    );
    println!("the tap is not refreshed by check, so outdated is as of the last run");
    installed.iter().for_each(|tool| println!("{}", tool_line(&context, &paths, tool, &outdated)));
    section("installed but no longer in the tap, left alone", &context.dropped());
    section("could not ask brew", &unasked);
    ExitCode::SUCCESS
}

fn show_log() -> ExitCode {
    let lines = paths().store.read_log();
    lines.iter().skip(lines.len().saturating_sub(60)).for_each(|line| println!("{line}"));
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    eprintln!("sth-update: {message}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["run"] => daemon(),
        ["now"] => once(),
        ["status"] => status(),
        ["check"] => check(),
        ["log"] => show_log(),
        ["--version" | "-V"] => {
            println!("sth-update {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => fail(&format!("unknown command\n\n{USAGE}")),
    }
}
