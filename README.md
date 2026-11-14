# sth-update

Keeps the tools from one Homebrew tap current on this machine. A release reaches every machine that runs it within the hour, with nothing to paste.

Every hour (and on `sth-update now`) it:

1. Refreshes only the tap (`git pull --ff-only` in the tap's checkout). It never runs a full `brew update`.
2. Upgrades only the tap's installed formulas and casks that are outdated, one at a time. Other brew packages are left alone, except a dependency a tool's own upgrade needs.
3. Restarts the services that run an upgraded tool, so they run the new binary.
4. Records what happened, and notifies you only if something failed.

## Install

```sh
brew install seethruhead/tap/sth-update
brew services start sth-update
```

## Commands

| Command | What it does |
|---|---|
| `sth-update run` | The daemon: update now, then again every hour. `brew services` runs this. |
| `sth-update now` | Update once, now. Waits if a run is already going, so only one brew run happens at a time. |
| `sth-update status` | The last run: what was upgraded, what failed, dev builds in the way, recent upgrades. |
| `sth-update check` | Changes nothing: lists each tool from the tap, whether it is outdated and what would restart. |
| `sth-update log` | The recent log. |

## What gets restarted

After an upgrade, sth-update looks at your user services (systemd user units in `~/.config/systemd/user`, launchd agents in `~/Library/LaunchAgents`) for ones that run the tool: anything under `$(brew --prefix)/opt/<tool>/`, or one of the tool's binaries at `$(brew --prefix)/bin/<binary>`. The ones that are running are restarted:

- a service `brew services` manages, with `brew services restart <name>`
- any other unit, with `systemctl --user restart <unit>` or `launchctl kickstart -k`

A tool with no running service needs nothing: the next time it is run, it is the new version.

`herdr`, `herdr-core`, `herdr-app` and `hc` are never restarted, whatever the config says, because they host terminals and agents. They are upgraded and reported as "takes effect on next restart".

sth-update upgrades itself last, then starts the new version in place of the running daemon.

## Dev builds in the way

If a copy of a tool's binary sits in `~/.local/bin`, `~/.cargo/bin` or `~/.bun/bin` and is not a link back into brew, your shell runs that copy instead of the brew one. sth-update does not touch it. `status` and `check` list it, and when such a tool is upgraded the run counts as failed, because the upgrade did not take effect.

## Config

Optional, at `~/.config/sth-update/config.toml`. Everything has a default.

```toml
tap = "seethruhead/tap"
every = "1h"                 # no shorter than 5m
casks = true                 # macOS only
shadow_dirs = ["~/.local/bin", "~/.cargo/bin", "~/.bun/bin"]

# Where failures are sent. The message is added as the last argument.
# Default: sth-seat notify where sth-seat is installed, else a desktop notification.
notify = ["sth-seat", "notify", "--title", "sth-update"]

[restart]
sth-encode-worker = "never"            # upgrade it, leave the running one alone
sth-seat = ["sth-seat", "install"]     # run this instead of restarting units
nudge = "auto"                         # the default for every tool
```

## Notifications

Only failures: a tap that would not refresh, an upgrade that failed, a service that did not restart, an upgrade hidden behind a dev build. The same failure is reported once, not every hour.

## Files

- State: `~/.local/state/sth-update/state.json`, log `~/.local/state/sth-update/log`
- `STH_UPDATE_CONFIG`, `STH_UPDATE_STATE` and `STH_UPDATE_BREW` point it somewhere else.

## Build

```sh
cargo build --release
cargo test
```
