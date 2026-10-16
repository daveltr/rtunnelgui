# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## State of the repo

All phases of `plans/ssh-tunnel-manager-plan.md` are implemented plus favorite groups. Only macOS has been run by hand; Windows compiles (`cargo check --target x86_64-pc-windows-msvc`) and Linux (GTK tray thread) has not been run. The tray test matrix and packaging are unverified; see the unchecked plan items.

## What it is

A cross-platform (Windows 10+, macOS 12+, Linux) Rust desktop app (`eframe`/egui) that manages SSH port forwards (`-L`/`-R`/`-D`) by spawning the system `ssh` binary. It never edits `~/.ssh/config`. Features: favorite groups (one server, many ports), system tray with hide-to-tray, session history, in-use local port scanner, known-hosts list from SSH config, auto-reconnect.

Commands: `cargo run`, `cargo test`, `cargo test <name>` for a single test, `cargo clippy --all-targets -- -D warnings`, `cargo packager --release` for installers.

## Architecture (the parts that span files)

- **Single event channel.** One `crossbeam_channel::unbounded::<AppEvent>()`. Every background thread (tunnel watchers, port scanner, host watcher, tray and signal handlers) holds a `Sender` and an `egui::Context` clone, and must call `ctx.request_repaint()` after each send. A hidden window does not repaint on its own, so a missed call means events stall while the app is in the tray.
- **UI thread owns all mutable state.** No `Mutex` on the model. It drains `AppEvent` each frame.
- **Tunnels are child `ssh -N` processes**, one watcher thread per child. The watcher captures the last ~20 lines of stderr and sends `TunnelEvent::Exited`. A child alive after 2s counts as `Running`. An earlier exit is `Failed(stderr)`. This heuristic relies on `-o ExitOnForwardFailure=yes`.
- **ssh args are always a `Vec`, never a shell string.** `BatchMode=yes` is on, so key or agent auth is required and there are no password prompts.
- **Tray.** On macOS/Windows the `TrayIcon` lives on the UI thread; on Linux GTK and the tray run on a dedicated thread and receive `TrayState` over a channel (`tray.rs`). Menu clicks arrive via `MenuEvent::set_event_handler` and become `AppEvent::Tray`. The close button is intercepted (`CancelClose` + `Visible(false)`) only when a tray exists. Quit calls `stop_all` then `process::exit`, because a hidden window may not process `Close`.
- **Groups.** A `Group` (server + ports) expands to one Local `Forward` per port. Forward ids are `Uuid::new_v5(group.id, local_port)`, so status, history and the ports tab key off the same `HashMap<Uuid, Status>` as standalone forwards. Use `find_forward(id)` (standalone or group port), not `forward(id)`.
- **Reconnect.** `App.attempts`/`due` hold backoff state (1s → 60s). Always stop through `App::stop_tunnel` (not `manager.stop`) so a pending retry is cancelled. The backoff resets only after 30s of being `Running`.
- **History.** `history.jsonl` gets a line at session start and another (same `session` id) at end; `history::load` folds them. The entry holds a `Forward` snapshot for Re-run.
- **Hosts.** `hosts::spawn_watcher` rescans when mtimes of the parsed config files change.
- **Single instance.** `instance.rs`: first instance listens on 127.0.0.1:47653; a second launch sends a magic line and exits.
- **Child cleanup is per-OS** (`platform/`). Windows: Job Object kill-on-close and `CREATE_NO_WINDOW`. Linux: `PR_SET_PDEATHSIG`. All Unix: `ctrlc` routes SIGINT/SIGTERM to a clean quit.
- **macOS gotcha.** Apps launched from Finder lack `SSH_AUTH_SOCK`; `tunnel.rs` asks `launchctl getenv`, and Settings has an override.
- **Storage** via `directories::ProjectDirs`:
  - `forwards.json`, `groups.json` and `settings.json` in the config dir, written atomically (tmp file, fsync, rename).
  - `history.jsonl` (append-only, trimmed to 5000 entries on startup) and `app.log` in the data dir.
  - Each `HistoryEntry` stores a full `Forward` snapshot so Re-run works after edits or deletes.
- **Known hosts.**
  - Parse `Host` lines in `~/.ssh/config`. Follow `Include` (globs, relative to `~/.ssh`, depth ≤ 16, visited set). Skip wildcard and negated patterns. Ignore `Match` blocks.
  - Resolve aliases with `ssh -G <alias>`, at most about 8 in parallel, cached.

## Modules

`app/` (state + event handling in `mod.rs`, one file per tab, `Bus`, `AppEvent`), `model`, `tunnel` (`TunnelManager`, `build_args`), `store`, `ports`, `history`, `hosts`, `tray`, `instance`, `platform/`.

## Build deps on Linux

`libgtk-3-dev`, `libayatana-appindicator3-dev`, `libxdo-dev`

## Out of scope for v1

Password and keyboard-interactive auth, editing `~/.ssh/config`, a pure-Rust SSH client (`russh`), and multi-hop UI beyond `extra_opts` (`-J`).
