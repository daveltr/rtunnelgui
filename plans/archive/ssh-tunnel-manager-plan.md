# SSH Tunnel Manager — Implementation Plan

A small cross-platform Rust desktop app that manages SSH port forwards without touching `~/.ssh/config`. Minimizes to the system tray, keeps a session history, shows in-use local ports, and lists hosts known from SSH config.

**Platforms:** Windows 10+, macOS 12+, Linux (X11/Wayland with AppIndicator support)

---

## TODO (by phase)

### Phase 0 — Project setup
- [x] `cargo new ssh-tunnel-manager`; set edition 2021, `rust-version`
- [x] Add deps (tray-icon, image, rfd deferred to the phases that use them): `eframe`, `egui_extras`, `serde`, `serde_json`, `directories`, `uuid`, `chrono`, `anyhow`, `crossbeam-channel`, `listeners`, `tracing`, `tracing-subscriber`
- [x] Module skeleton: `main, app/, model, tunnel, store, history, ports, hosts, tray, instance, platform/{mod,windows,unix}`
- [x] Embed icon PNGs (`assets/icon_idle.png`, `assets/icon_active.png`) via `include_bytes!`
- [x] CI: GitHub Actions matrix build (ubuntu, macos, windows) + `cargo clippy -D warnings` + `cargo test`

### Phase 1 — Single tunnel end-to-end
- [x] `model.rs`: `Forward`, `Kind`, `Status` types (see Data Model)
- [x] `tunnel.rs`: `build_command(&Forward, &Settings) -> Command`
- [x] Unit tests for `build_command` arg generation (L/R/D, optional port, identity, extra opts)
- [x] `TunnelManager::start(id)` spawns `ssh -N …`, stores `Child`
- [x] Watcher thread per child: capture stderr (last ~20 lines), `wait()`, send `TunnelEvent` on channel
- [x] "Alive after 2s" ⇒ `Running`; earlier exit ⇒ `Failed(stderr tail)`
- [x] `TunnelManager::stop(id)` kills child, waits, emits `Stopped`
- [x] Minimal egui window with one hardcoded forward + Start/Stop + status label
- [x] Windows: spawn with `CREATE_NO_WINDOW`

### Phase 2 — Forward CRUD + persistence
- [x] `store.rs`: load/save `forwards.json` and `settings.json` in config dir
- [x] Atomic writes (write `*.tmp`, `fsync`, rename)
- [x] Forwards table view (`egui_extras::TableBuilder`): name, summary `L 8080 → db:5432 via host`, status dot, Start/Stop/Edit/Delete
- [x] Add/Edit modal with validation (port 1–65535, host non-empty, unique name)
- [x] Live preview of exact `ssh` command in the dialog (copyable)
- [x] Delete confirmation; stop tunnel first if running
- [x] Start all `auto_start` forwards on launch

### Phase 2b — In-use ports
- [x] Add `listeners` crate (fallback: `netstat2`)
- [x] `ports.rs`: `scan() -> Vec<PortInfo>` on background thread, every 4s while window visible
- [x] Map PIDs to running tunnels → `PortInfo.tunnel`
- [x] Ports tab: table (port, proto, addr, process, pid, tunnel); filter box; "only mine" toggle; Stop button on tunnel rows
- [x] Add/Edit dialog: inline conflict warning ("8080 in use by postgres (pid 1234)")
- [x] "Pick free port" button (bind `127.0.0.1:0`, read port, drop)
- [x] Pre-spawn check: test-bind local port for `Local`/`Dynamic`; fail fast with clear message
- [x] Graceful "unknown" for process names hidden by permissions

### Phase 3 — Tray + hide-to-tray (highest risk — test all 3 OSes)
- [x] `tray.rs`: build tray icon + menu (Show/Hide, separator, per-forward check items, separator, Quit)
- [x] Linux: run GTK init + tray on dedicated thread (`gtk::init()`, `gtk::main()`)
- [x] Event pump thread: read `TrayIconEvent` / `MenuEvent` receivers → forward to app channel → `ctx.request_repaint()`
- [x] Intercept close: `ctx.input(|i| i.viewport().close_requested())` → `CancelClose` + `Visible(false)`
- [x] Real quit path (tray Quit) stops all tunnels then exits
- [x] Rebuild tray menu when forwards change; sync check state with status
- [x] Swap tray icon idle/active when any tunnel running; tooltip "N tunnels active"
- [x] Left-click tray toggles window (Win/Linux); macOS: menu on click
- [x] Setting: start minimized
- [ ] Manual test matrix: Win11, macOS, Ubuntu GNOME (+ AppIndicator ext), KDE — NOT DONE: only a macOS smoke test (launch, single-instance, SIGTERM) was possible

### Phase 4 — History + known hosts
- [x] `history.rs`: append `HistoryEntry` JSON lines to `history.jsonl` (data dir) on start and end
- [x] Trim to last 5000 entries on startup
- [x] History tab: table (time, forward name, summary, duration, result), filter, Re-run, Clear (confirm)
- [x] Re-run: recreate forward from entry snapshot (entry stores full `Forward` copy)
- [x] `hosts.rs`: parse `~/.ssh/config` `Host` lines, follow `Include` (globs, relative to `~/.ssh`), skip wildcard/negated patterns
- [x] Optional `known_hosts` hostnames (skip hashed `|1|` entries, strip `[host]:port`)
- [x] Resolve each alias via `ssh -G <alias>` (parallel, bounded to ~8, cached) → user/hostname/port/identityfile
- [x] Merge history hosts with `HostSource::History`
- [x] Hosts panel: searchable list, source badges, click → fill ssh host in form
- [x] Autocomplete in Add/Edit for ssh host (hosts) and target host/ports (history)
- [x] Re-scan on config mtime change (or `notify` crate)

### Phase 5 — Polish + packaging
- [x] Auto-reconnect with exponential backoff (1s → 60s cap), per-forward toggle, reset on manual stop
- [x] Windows Job Object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) so children die with app
- [x] Unix: kill children on SIGTERM/SIGINT (`ctrlc` crate); Linux `prctl(PR_SET_PDEATHSIG)` via `pre_exec`
- [x] Settings tab: ssh path (auto-detect), `SSH_AUTH_SOCK` override (macOS), start minimized, launch at login (`auto-launch`), reconnect default
- [x] Single-instance guard (loopback socket in `instance.rs` instead of the `single-instance` crate, since the crate cannot signal the first instance); second launch shows existing window
- [x] Desktop notification on tunnel failure (`notify-rust`)
- [x] Logging to file in data dir via `tracing-appender`
- [ ] Packaging with `cargo-packager`: `.msi`, `.dmg`/`.app` (LSUIElement optional), `.deb` + AppImage — config in Cargo.toml + CI job; unverified until cargo-packager has run
- [x] README (screenshots still to add)

---

## Architecture

```
┌──────────────── UI thread (eframe) ────────────────┐
│ App state: forwards, statuses, ports, hosts, hist  │
│ drains AppEvent channel each frame                 │
└───────▲───────────────▲───────────────▲────────────┘
        │ AppEvent      │               │
┌───────┴─────┐ ┌───────┴──────┐ ┌──────┴─────────┐
│ Tunnel      │ │ Port scanner │ │ Tray pump      │
│ watchers    │ │ thread       │ │ thread         │
│ (1/child)   │ └──────────────┘ └────────────────┘
└─────────────┘        Host resolver (on demand)
```

- One `crossbeam_channel::unbounded::<AppEvent>()`; every background thread holds a `Sender` plus an `egui::Context` clone and calls `request_repaint()` after sending.
- UI thread is the only owner of mutable app state; no `Mutex` on the model.
- Hidden windows don't repaint on their own — this is why every background thread must call `request_repaint()`.

```rust
enum AppEvent {
    Tunnel(TunnelEvent),
    Ports(Vec<PortInfo>),
    Hosts(Vec<KnownHost>),
    Tray(TrayAction),
}
enum TunnelEvent { Started { id: Uuid, pid: u32 }, Running { id: Uuid }, Exited { id: Uuid, code: Option<i32>, stderr: String } }
enum TrayAction { ToggleWindow, Toggle(Uuid), Quit }
```

## Data Model

```rust
#[derive(Serialize, Deserialize, Clone)]
struct Forward {
    id: Uuid,
    name: String,
    kind: Kind,
    bind_addr: Option<String>,   // default 127.0.0.1
    bind_port: u16,
    target_host: String,         // unused for Dynamic
    target_port: u16,            // unused for Dynamic
    ssh_host: String,            // alias or user@host
    ssh_port: Option<u16>,
    identity: Option<PathBuf>,
    extra_opts: Vec<String>,     // e.g. ["-J", "bastion"]
    auto_start: bool,
    auto_reconnect: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
enum Kind { Local, Remote, Dynamic }

enum Status { Stopped, Starting, Running { pid: u32, since: DateTime<Local> }, Reconnecting { attempt: u32 }, Failed(String) }

#[derive(Serialize, Deserialize)]
struct HistoryEntry {
    forward: Forward,            // snapshot, so re-run works after edits/deletes
    started: DateTime<Local>,
    ended: Option<DateTime<Local>>,
    exit_code: Option<i32>,
    error: Option<String>,
}

struct PortInfo { port: u16, proto: Proto, addr: IpAddr, pid: Option<u32>, process: Option<String>, tunnel: Option<Uuid> }

struct KnownHost { alias: String, user: Option<String>, hostname: String, port: u16, identity: Option<PathBuf>, source: HostSource }
enum HostSource { Config, KnownHosts, History }

#[derive(Serialize, Deserialize)]
struct Settings {
    ssh_path: Option<PathBuf>,
    ssh_auth_sock: Option<String>,
    start_minimized: bool,
    launch_at_login: bool,
    reconnect_default: bool,
}
```

## SSH command

```
ssh -N
    -o ExitOnForwardFailure=yes
    -o ServerAliveInterval=30
    -o ServerAliveCountMax=3
    -o BatchMode=yes
    [-p PORT] [-i IDENTITY] [extra_opts...]
    -L [bind_addr:]bind_port:target_host:target_port   # Local
    -R [bind_addr:]bind_port:target_host:target_port   # Remote
    -D [bind_addr:]bind_port                           # Dynamic
    SSH_HOST
```

- `BatchMode=yes` → no interactive prompts; key/agent auth required (document this).
- `ExitOnForwardFailure=yes` → bind failures exit quickly, so the 2s liveness heuristic works.
- Pass args as a vector (never a shell string) — no injection risk from user fields.
- ssh binary resolution: settings override → `PATH` → Windows `C:\Windows\System32\OpenSSH\ssh.exe`.

## Storage

| File | Location (`directories::ProjectDirs`) | Format |
|---|---|---|
| `forwards.json` | config dir | JSON array, atomic write |
| `settings.json` | config dir | JSON, atomic write |
| `history.jsonl` | data dir | append-only JSON lines |
| `app.log` | data dir | tracing output |

Qualifier suggestion: `ProjectDirs::from("dev", "yourname", "ssh-tunnel-manager")`.

## UI layout

Top bar tabs: **Forwards | Ports | History | Settings**, plus an always-available **Hosts** side panel (collapsible) on Forwards and the Add/Edit dialog.

- **Forwards:** table + "Add" button; status dot colors: grey stopped, yellow starting/reconnecting, green running, red failed (hover = error text).
- **Add/Edit:** kind radio; bind addr/port (+ conflict warning, "Pick free"); target host/port; ssh host (autocomplete); ssh port; identity file picker (`rfd`); extra opts; auto-start/reconnect; command preview.
- **Ports:** filterable table; "only mine" toggle; refresh indicator.
- **History:** newest first; filter; Re-run; Clear.
- **Settings:** fields from `Settings`; "Open data folder" button.

## Known hosts parsing details

- Tokenize lines: case-insensitive keyword, value may be `=`-separated or whitespace; strip comments.
- `Host a b c` → three aliases; skip any containing `*`, `?`, or starting with `!`.
- `Include` paths: `~` expansion; relative paths resolve against `~/.ssh`; expand globs (`glob` crate); guard against recursion (depth ≤ 16, visited set).
- Ignore `Match` blocks for discovery; `ssh -G` resolves them correctly anyway.
- `ssh -G` output: lowercase `key value` lines → read `user`, `hostname`, `port`, `identityfile` (first).

## Platform notes

| Concern | Windows | macOS | Linux |
|---|---|---|---|
| Tray | works natively | menu on click; consider `LSUIElement` | needs AppIndicator; GTK thread |
| ssh | built-in OpenSSH | system ssh | system ssh |
| Agent | OpenSSH agent service / Pageant (not supported by OpenSSH) | Finder launch lacks `SSH_AUTH_SOCK` → settings override / `launchctl getenv` | usually fine |
| Child cleanup | Job Object | signal handler | `PR_SET_PDEATHSIG` + signal handler |
| Console flash | `CREATE_NO_WINDOW` | n/a | n/a |
| Build deps | none | none | `libgtk-3-dev`, `libayatana-appindicator3-dev`, `libxdo-dev` |

## Testing

- **Unit:** command builder, config parser (fixtures with Include/globs/wildcards), history trim, atomic write.
- **Integration (Linux CI):** run `sshd` in Docker (`linuxserver/openssh-server`), start a local forward to an HTTP container, assert reachability, kill, assert `Exited` event.
- **Manual:** tray matrix in Phase 3; sleep/wake with auto-reconnect; port conflict path; passworded key without agent → clear error.

## Out of scope (v1)

- Password / keyboard-interactive auth (could add later via `SSH_ASKPASS` helper)
- Editing `~/.ssh/config`
- Pure-Rust SSH (`russh`) — possible later behind the `TunnelManager` interface
- Multi-hop UI beyond `extra_opts` (`-J`)
