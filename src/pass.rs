use crate::brew::{self, Brew, Kind, Outdated, Service};
use crate::config::Config;
use crate::services::{Plan, Platform, Unit};
use crate::shadow::{self, Shadow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SELF: &str = "sth-update";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    NoService,
    Restarted(Vec<String>),
    NextRestart(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upgraded {
    pub at: u64,
    pub tool: String,
    pub from: String,
    pub to: String,
    pub effect: Effect,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub at: u64,
    pub upgraded: Vec<Upgraded>,
    pub failures: Vec<String>,
    pub shadowed: Vec<Shadow>,
    pub notes: Vec<String>,
}

impl Upgraded {
    pub fn line(&self) -> String {
        let change = format!("{} {} to {}", self.tool, self.from, self.to);
        match &self.effect {
            Effect::NoService => change,
            Effect::Restarted(what) if what.is_empty() => change,
            Effect::Restarted(what) => format!("{change}, restarted {}", what.join(", ")),
            Effect::NextRestart(what) => {
                format!("{change}, takes effect on next restart ({} is left running)", what.join(", "))
            }
        }
    }
}

impl Report {
    pub fn upgraded_self(&self) -> bool {
        self.upgraded.iter().any(|item| item.tool == SELF)
    }
}

pub struct Context<'a> {
    pub config: &'a Config,
    pub brew: Brew,
    pub platform: Platform,
    pub services: Vec<Service>,
    pub repo: PathBuf,
}

struct Outcome {
    upgraded: Option<Upgraded>,
    failures: Vec<String>,
}

impl<'a> Context<'a> {
    pub fn open(config: &'a Config, home: &Path) -> Result<Context<'a>, String> {
        let brew = Brew::locate()?;
        let services = brew.services();
        let repo = brew.tap_repo(&config.tap)?;
        Ok(Context { config, brew, platform: Platform::current(home), services, repo })
    }

    pub fn kinds(&self) -> Vec<Kind> {
        let casks = self.config.casks && self.platform.mac;
        [Kind::Formula].into_iter().chain(casks.then_some(Kind::Cask)).collect()
    }

    pub fn installed(&self) -> Result<Vec<String>, String> {
        let listings: Result<Vec<Vec<String>>, String> =
            self.kinds().iter().map(|kind| self.brew.installed(&self.config.tap, *kind)).collect();
        let all: Vec<String> = listings?.into_iter().flatten().collect();
        Ok(all
            .iter()
            .enumerate()
            .filter(|(at, tool)| !all[..*at].contains(tool))
            .map(|(_, tool)| tool.clone())
            .collect())
    }

    pub fn outdated(&self) -> (Vec<Outdated>, Vec<String>) {
        let found: Vec<Result<Vec<Outdated>, String>> =
            self.kinds().iter().map(|kind| self.outdated_of(*kind)).collect();
        let items = found.iter().filter_map(|result| result.clone().ok()).flatten().filter(|item| !item.pinned);
        let failures = found.iter().filter_map(|result| result.clone().err());
        (items.collect(), failures.collect())
    }

    fn split_by_offer(&self, kind: Kind) -> Result<(Vec<String>, Vec<String>), String> {
        let installed = self.brew.installed(&self.config.tap, kind)?;
        Ok(installed.into_iter().partition(|tool| brew::offers(&self.repo, kind, tool)))
    }

    fn outdated_of(&self, kind: Kind) -> Result<Vec<Outdated>, String> {
        let (offered, _) = self.split_by_offer(kind)?;
        self.brew.outdated(&self.config.tap, kind, &offered)
    }

    fn dropped_of(&self, kind: Kind) -> Vec<String> {
        let dropped = self.split_by_offer(kind).map(|(_, dropped)| dropped).unwrap_or_default();
        dropped.iter().map(|tool| format!("{tool} ({})", kind.noun())).collect()
    }

    pub fn dropped(&self) -> Vec<String> {
        self.kinds().iter().flat_map(|kind| self.dropped_of(*kind)).collect()
    }

    fn running(&self, tool: &str) -> Vec<Unit> {
        self.platform
            .units(&self.brew, tool, &self.services)
            .into_iter()
            .filter(|unit| self.platform.active(unit))
            .collect()
    }

    pub fn plan(&self, tool: &str) -> Plan {
        crate::services::plan(&self.config.rule(tool), &self.running(tool))
    }

    pub fn shadows(&self, tool: &str, home: &Path) -> Vec<Shadow> {
        shadow::find(&self.brew.prefix, tool, &self.brew.binaries(tool), &self.config.shadow_dirs(home))
    }

    fn restart(&self, item: &Outdated, by_daemon: bool) -> (Effect, Vec<String>) {
        if item.tool == SELF && by_daemon {
            return (Effect::Restarted(vec!["itself".to_string()]), Vec::new());
        }
        match self.plan(&item.tool) {
            Plan::Nothing => (Effect::NoService, Vec::new()),
            Plan::Deferred(units) => (Effect::NextRestart(units), Vec::new()),
            Plan::Restart(actions) => {
                let results: Vec<Result<String, String>> =
                    actions.iter().map(|action| self.platform.perform(&self.brew, action)).collect();
                let restarted = results.iter().filter_map(|result| result.clone().ok()).collect();
                let failed = results.iter().filter_map(|result| result.clone().err());
                let failures = failed.map(|error| {
                    format!("{} {} is installed but its service did not restart: {error}", item.tool, item.to)
                });
                (Effect::Restarted(restarted), failures.collect())
            }
        }
    }

    fn upgrade(&self, item: &Outdated, at: u64, by_daemon: bool) -> Outcome {
        if let Err(error) = self.brew.upgrade(&self.config.tap, item) {
            return Outcome {
                upgraded: None,
                failures: vec![format!("upgrading {} to {} failed: {error}", item.tool, item.to)],
            };
        }
        let (effect, failures) = self.restart(item, by_daemon);
        let upgraded = Upgraded { at, tool: item.tool.clone(), from: item.from.clone(), to: item.to.clone(), effect };
        Outcome { upgraded: Some(upgraded), failures }
    }
}

pub fn self_last(outdated: Vec<Outdated>) -> Vec<Outdated> {
    let (own, others): (Vec<Outdated>, Vec<Outdated>) = outdated.into_iter().partition(|item| item.tool == SELF);
    others.into_iter().chain(own).collect()
}

fn hidden(item: &Upgraded, shadowed: &[Shadow]) -> Option<String> {
    let shadow = shadowed.iter().find(|shadow| shadow.tool == item.tool)?;
    Some(format!("{} {} is installed, but the dev build at {} runs instead", item.tool, item.to, shadow.path))
}

fn in_the_way(upgraded: &[Upgraded], shadowed: &[Shadow]) -> Vec<String> {
    upgraded.iter().filter_map(|item| hidden(item, shadowed)).collect()
}

fn failed(at: u64, error: String) -> Report {
    Report { at, failures: vec![error], ..Report::default() }
}

pub fn run(config: &Config, home: &Path, at: u64, by_daemon: bool) -> Report {
    let context = match Context::open(config, home) {
        Ok(context) => context,
        Err(error) => return failed(at, error),
    };
    let refresh =
        context.brew.refresh_tap(&context.repo).err().map(|error| format!("refreshing {} failed: {error}", config.tap));
    let trusted = context.brew.trust(&config.tap);
    let distrust = trusted.clone().err().map(|error| format!("trusting {} failed: {error}", config.tap));
    let notes =
        trusted.ok().filter(|done| *done).map(|_| format!("brew did not trust {}, so it was trusted", config.tap));
    let (outdated, unasked) = context.outdated();
    let outdated = self_last(outdated);
    let outcomes: Vec<Outcome> = outdated.iter().map(|item| context.upgrade(item, at, by_daemon)).collect();
    let upgraded: Vec<Upgraded> = outcomes.iter().filter_map(|outcome| outcome.upgraded.clone()).collect();
    let installed = context.installed().unwrap_or_default();
    let shadowed: Vec<Shadow> = installed.iter().flat_map(|tool| context.shadows(tool, home)).collect();
    let step_failures = outcomes.iter().flat_map(|outcome| outcome.failures.clone());
    let failures = refresh
        .into_iter()
        .chain(distrust)
        .chain(unasked)
        .chain(step_failures)
        .chain(in_the_way(&upgraded, &shadowed))
        .collect();
    Report { at, upgraded, failures, shadowed, notes: notes.into_iter().collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outdated(tool: &str) -> Outdated {
        Outdated { tool: tool.into(), kind: Kind::Formula, from: "1".into(), to: "2".into(), pinned: false }
    }

    fn upgraded(tool: &str, effect: Effect) -> Upgraded {
        Upgraded { at: 0, tool: tool.into(), from: "0.9.0".into(), to: "0.9.1".into(), effect }
    }

    #[test]
    fn the_updater_upgrades_itself_after_everything_else() {
        let order = self_last(vec![outdated("nudge"), outdated(SELF), outdated("reach")]);
        assert_eq!(order.iter().map(|item| item.tool.as_str()).collect::<Vec<_>>(), vec!["nudge", "reach", SELF]);
    }

    #[test]
    fn an_upgrade_reads_as_what_changed_and_what_was_restarted() {
        assert_eq!(upgraded("reach", Effect::NoService).line(), "reach 0.9.0 to 0.9.1");
        let restarted = upgraded("nudge", Effect::Restarted(vec!["nudge (brew service)".into()]));
        assert_eq!(restarted.line(), "nudge 0.9.0 to 0.9.1, restarted nudge (brew service)");
        let deferred = upgraded("herdr-core", Effect::NextRestart(vec!["herdr-core.service".into()]));
        assert_eq!(
            deferred.line(),
            "herdr-core 0.9.0 to 0.9.1, takes effect on next restart (herdr-core.service is left running)"
        );
    }

    #[test]
    fn an_upgrade_hidden_behind_a_dev_build_is_a_problem_and_other_shadows_are_not() {
        let done = vec![upgraded("nudge", Effect::NoService)];
        let shadows = vec![
            Shadow { tool: "nudge".into(), path: "/home/a/.local/bin/nudge".into() },
            Shadow { tool: "reach".into(), path: "/home/a/.bun/bin/reach".into() },
        ];
        assert_eq!(
            in_the_way(&done, &shadows),
            vec!["nudge 0.9.1 is installed, but the dev build at /home/a/.local/bin/nudge runs instead"]
        );
    }

    #[test]
    fn a_report_knows_when_the_updater_itself_was_upgraded() {
        let report = Report { upgraded: vec![upgraded(SELF, Effect::NoService)], ..Report::default() };
        assert!(report.upgraded_self());
        assert!(!Report::default().upgraded_self());
    }
}
