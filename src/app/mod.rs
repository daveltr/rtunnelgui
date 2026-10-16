mod forwards;
mod history_tab;
mod hosts_panel;
mod ports_tab;
mod settings_tab;

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::Local;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::{self, Color32, ViewportCommand};
use uuid::Uuid;

use crate::history::{self, HistoryEntry};
use crate::hosts::{self, HostSource, KnownHost};
use crate::model::{parse_ports, Forward, Group, Kind, Settings, Status};
use crate::ports::{self, PortInfo};
use crate::tray::{self, Tray, TrayAction, TrayState};
use crate::tunnel::{self, TunnelEvent, TunnelManager};

/// A connection must stay up this long before the reconnect backoff resets.
const STABLE_AFTER_SECS: i64 = 30;

pub enum AppEvent {
    Tunnel(TunnelEvent),
    Ports(Vec<PortInfo>),
    Hosts(Vec<KnownHost>),
    Tray(TrayAction),
}

/// Channel sender + repaint handle: a hidden window never repaints on its own.
#[derive(Clone)]
pub struct Bus {
    pub(crate) tx: Sender<AppEvent>,
    pub(crate) ctx: egui::Context,
}

impl Bus {
    pub fn send(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Forwards,
    Ports,
    History,
    Settings,
}

/// Reconnect delay: 1s, 2s, 4s … capped at 60s.
fn backoff_secs(attempt: u32) -> u64 {
    (1u64 << attempt.saturating_sub(1).min(6)).min(60)
}

fn is_active(st: &Status) -> bool {
    matches!(st, Status::Starting | Status::Running { .. } | Status::Reconnecting { .. })
}

fn notify(title: String, body: String) {
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().summary(&title).body(&body).show();
    });
}

fn opt(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parse_port(label: &str, s: &str) -> Result<u16, String> {
    match s.trim().parse::<u16>() {
        Ok(p) if p >= 1 => Ok(p),
        _ => Err(format!("{label} must be a port 1–65535")),
    }
}

/// Text-field mirror of a `Forward` while the dialog is open.
struct Draft {
    id: Uuid,
    is_new: bool,
    name: String,
    kind: Kind,
    bind_addr: String,
    bind_port: String,
    target_host: String,
    target_port: String,
    ssh_host: String,
    ssh_port: String,
    identity: String,
    extra_opts: String,
    auto_start: bool,
    auto_reconnect: bool,
}

impl Draft {
    fn new(reconnect_default: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            is_new: true,
            name: String::new(),
            kind: Kind::Local,
            bind_addr: String::new(),
            bind_port: String::new(),
            target_host: "localhost".into(),
            target_port: String::new(),
            ssh_host: String::new(),
            ssh_port: String::new(),
            identity: String::new(),
            extra_opts: String::new(),
            auto_start: false,
            auto_reconnect: reconnect_default,
        }
    }

    fn from(f: &Forward) -> Self {
        Self {
            id: f.id,
            is_new: false,
            name: f.name.clone(),
            kind: f.kind,
            bind_addr: f.bind_addr.clone().unwrap_or_default(),
            bind_port: f.bind_port.to_string(),
            target_host: f.target_host.clone(),
            target_port: f.target_port.to_string(),
            ssh_host: f.ssh_host.clone(),
            ssh_port: f.ssh_port.map(|p| p.to_string()).unwrap_or_default(),
            identity: f.identity.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            extra_opts: f.extra_opts.join(" "),
            auto_start: f.auto_start,
            auto_reconnect: f.auto_reconnect,
        }
    }

    fn to_forward(&self, others: &[Forward]) -> Result<Forward, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("name is required".into());
        }
        if others.iter().any(|o| o.id != self.id && o.name == name) {
            return Err(format!("name \"{name}\" is already used"));
        }
        if self.ssh_host.trim().is_empty() {
            return Err("SSH host is required".into());
        }
        let dynamic = self.kind == Kind::Dynamic;
        if !dynamic && self.target_host.trim().is_empty() {
            return Err("target host is required".into());
        }
        Ok(Forward {
            id: self.id,
            name: name.into(),
            kind: self.kind,
            bind_addr: opt(&self.bind_addr),
            bind_port: parse_port("bind port", &self.bind_port)?,
            target_host: self.target_host.trim().into(),
            target_port: if dynamic { 0 } else { parse_port("target port", &self.target_port)? },
            ssh_host: self.ssh_host.trim().into(),
            ssh_port: opt(&self.ssh_port).map(|_| parse_port("SSH port", &self.ssh_port)).transpose()?,
            identity: opt(&self.identity).map(Into::into),
            // ponytail: split on whitespace, no quoting; fine for -J/-o style options.
            extra_opts: self.extra_opts.split_whitespace().map(String::from).collect(),
            auto_start: self.auto_start,
            auto_reconnect: self.auto_reconnect,
        })
    }
}

/// Text-field mirror of a `Group` while the dialog is open.
struct GroupDraft {
    id: Uuid,
    is_new: bool,
    name: String,
    ssh_host: String,
    ssh_port: String,
    identity: String,
    extra_opts: String,
    ports: String,
    auto_start: bool,
    auto_reconnect: bool,
}

impl GroupDraft {
    fn new(reconnect_default: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            is_new: true,
            name: String::new(),
            ssh_host: String::new(),
            ssh_port: String::new(),
            identity: String::new(),
            extra_opts: String::new(),
            ports: String::new(),
            auto_start: false,
            auto_reconnect: reconnect_default,
        }
    }

    fn from(g: &Group) -> Self {
        Self {
            id: g.id,
            is_new: false,
            name: g.name.clone(),
            ssh_host: g.ssh_host.clone(),
            ssh_port: g.ssh_port.map(|p| p.to_string()).unwrap_or_default(),
            identity: g.identity.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            extra_opts: g.extra_opts.join(" "),
            ports: g.ports.iter().map(|p| if p.local == p.remote { p.local.to_string() } else { format!("{}:{}", p.local, p.remote) }).collect::<Vec<_>>().join(" "),
            auto_start: g.auto_start,
            auto_reconnect: g.auto_reconnect,
        }
    }

    fn to_group(&self, others: &[Group]) -> Result<Group, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("name is required".into());
        }
        if others.iter().any(|o| o.id != self.id && o.name == name) {
            return Err(format!("group \"{name}\" already exists"));
        }
        if self.ssh_host.trim().is_empty() {
            return Err("SSH host is required".into());
        }
        Ok(Group {
            id: self.id,
            name: name.into(),
            ssh_host: self.ssh_host.trim().into(),
            ssh_port: opt(&self.ssh_port).map(|_| parse_port("SSH port", &self.ssh_port)).transpose()?,
            identity: opt(&self.identity).map(Into::into),
            extra_opts: self.extra_opts.split_whitespace().map(String::from).collect(),
            ports: parse_ports(&self.ports)?,
            auto_start: self.auto_start,
            auto_reconnect: self.auto_reconnect,
        })
    }
}

pub struct App {
    rx: Receiver<AppEvent>,
    manager: TunnelManager,
    forwards: Vec<Forward>,
    groups: Vec<Group>,
    settings: Settings,
    status: HashMap<Uuid, Status>,
    ports: Vec<PortInfo>,
    hosts: Vec<KnownHost>,
    history: Vec<HistoryEntry>,
    /// Open history session per running forward id.
    sessions: HashMap<Uuid, HistoryEntry>,
    /// Consecutive failed attempts, and when the next retry fires.
    attempts: HashMap<Uuid, u32>,
    due: HashMap<Uuid, Instant>,
    tray: Option<Tray>,
    window_visible: bool,
    tab: Tab,
    draft: Option<Draft>,
    group_draft: Option<GroupDraft>,
    confirm_delete: Option<Uuid>,
    confirm_delete_group: Option<Uuid>,
    confirm_clear_history: bool,
    show_hosts: bool,
    host_filter: String,
    history_filter: String,
    port_filter: String,
    only_mine: bool,
    notice: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, guard: Option<TcpListener>) -> Self {
        let (tx, rx) = unbounded();
        let bus = Bus { tx, ctx: cc.egui_ctx.clone() };
        ports::spawn_scanner(bus.clone());
        if let Some(l) = guard {
            crate::instance::listen(l, bus.clone());
        }
        // Ctrl-C / SIGTERM: quit through the UI loop so children are stopped.
        let b = bus.clone();
        let _ = ctrlc::set_handler(move || {
            b.send(AppEvent::Tray(TrayAction::Quit));
            // ponytail: macOS may not wake an occluded/hidden window for request_repaint, so the UI never sees Quit.
            // Fallback: kill our ssh children and exit. Windows children die with the Job Object.
            std::thread::sleep(std::time::Duration::from_secs(2));
            #[cfg(unix)]
            let _ = std::process::Command::new("pkill").args(["-TERM", "-P", &std::process::id().to_string()]).status();
            std::process::exit(130)
        });

        let mut notice = None;
        let forwards: Vec<Forward> = crate::store::load("forwards.json").unwrap_or_else(|e| {
            notice = Some(format!("forwards.json not loaded: {e:#}"));
            vec![]
        });
        let groups: Vec<Group> = crate::store::load("groups.json").unwrap_or_else(|e| {
            notice = Some(format!("groups.json not loaded: {e:#}"));
            vec![]
        });
        let settings: Settings = crate::store::load("settings.json").unwrap_or_else(|e| {
            notice = Some(format!("settings.json not loaded: {e:#}"));
            Settings::default()
        });
        hosts::spawn_watcher(bus.clone(), tunnel::ssh_path(&settings));

        let tray = Tray::new(bus.clone());
        // Started hidden (main.rs) but no tray to bring it back: show it.
        let window_visible = !settings.start_minimized || tray.is_none();
        ports::WINDOW_VISIBLE.store(window_visible, Ordering::Relaxed);
        if !window_visible {
            tracing::info!("starting minimized to tray");
        } else if settings.start_minimized {
            cc.egui_ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        }

        let mut app = Self {
            rx,
            manager: TunnelManager::new(bus),
            status: forwards.iter().map(|f| (f.id, Status::Stopped)).collect(),
            forwards,
            groups,
            settings,
            ports: vec![],
            hosts: vec![],
            history: history::load(),
            sessions: HashMap::new(),
            attempts: HashMap::new(),
            due: HashMap::new(),
            tray,
            window_visible,
            tab: Tab::Forwards,
            draft: None,
            group_draft: None,
            confirm_delete: None,
            confirm_delete_group: None,
            confirm_clear_history: false,
            show_hosts: false,
            host_filter: String::new(),
            history_filter: String::new(),
            port_filter: String::new(),
            only_mine: false,
            notice,
        };
        let auto: Vec<Forward> = app
            .forwards
            .iter()
            .filter(|f| f.auto_start)
            .cloned()
            .chain(app.groups.iter().filter(|g| g.auto_start).flat_map(Group::forwards))
            .collect();
        for f in auto {
            app.start_forward(&f, false);
        }
        app
    }

    // ---- lookups -------------------------------------------------------------------------

    fn forward(&self, id: Uuid) -> Option<&Forward> {
        self.forwards.iter().find(|f| f.id == id)
    }

    /// A standalone forward or one port of a group.
    fn find_forward(&self, id: Uuid) -> Option<Forward> {
        self.forward(id).cloned().or_else(|| self.groups.iter().flat_map(Group::forwards).find(|f| f.id == id))
    }

    fn tunnel_name(&self, id: Uuid) -> String {
        self.find_forward(id).map(|f| f.name).unwrap_or_else(|| "?".into())
    }

    fn active(&self, id: Uuid) -> bool {
        self.status.get(&id).is_some_and(is_active)
    }

    fn group_active(&self, id: Uuid) -> bool {
        self.groups.iter().filter(|g| g.id == id).flat_map(Group::forwards).any(|f| self.active(f.id))
    }

    fn all_hosts(&self) -> Vec<KnownHost> {
        let mut v = self.hosts.clone();
        let known = self.history.iter().map(|e| &e.forward).chain(&self.forwards);
        for f in known {
            if !v.iter().any(|h| h.alias == f.ssh_host) {
                v.push(KnownHost { alias: f.ssh_host.clone(), user: None, hostname: f.ssh_host.clone(), port: f.ssh_port.unwrap_or(22), identity: None, source: HostSource::History });
            }
        }
        v
    }

    /// Distinct (host, port) targets seen in history and saved forwards.
    fn targets(&self) -> Vec<(String, u16)> {
        let mut v: Vec<(String, u16)> = vec![];
        for f in self.history.iter().rev().map(|e| &e.forward).chain(&self.forwards) {
            let t = (f.target_host.clone(), f.target_port);
            if f.kind != Kind::Dynamic && !v.contains(&t) {
                v.push(t);
            }
        }
        v
    }

    /// Warnings for local ports already held by something that isn't one of `own`'s tunnels.
    fn conflicts(&self, ports: &[u16], own: &[Uuid]) -> Vec<String> {
        let mine = self.manager.pids();
        ports
            .iter()
            .filter_map(|port| {
                let p = self.ports.iter().find(|p| p.port == *port && !mine.get(&p.pid).is_some_and(|id| own.contains(id)))?;
                let who = if p.process.is_empty() { "unknown" } else { &p.process };
                Some(format!("⚠ {} in use by {who} (pid {})", p.port, p.pid))
            })
            .collect()
    }

    // ---- persistence ---------------------------------------------------------------------

    fn save(&mut self) {
        if let Err(e) = crate::store::save("forwards.json", &self.forwards) {
            self.notice = Some(format!("could not save forwards: {e:#}"));
        }
    }

    fn save_groups(&mut self) {
        if let Err(e) = crate::store::save("groups.json", &self.groups) {
            self.notice = Some(format!("could not save groups: {e:#}"));
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = crate::store::save("settings.json", &self.settings) {
            self.notice = Some(format!("could not save settings: {e:#}"));
        }
    }

    // ---- tunnel control ------------------------------------------------------------------

    fn start(&mut self, id: Uuid) {
        if let Some(f) = self.find_forward(id) {
            self.start_forward(&f, false);
        }
    }

    /// `retry`: called by the backoff timer, so a failure reschedules instead of surfacing and history stays quiet.
    fn start_forward(&mut self, f: &Forward, retry: bool) {
        let id = f.id;
        if self.manager.pid_of(id).is_some() {
            return;
        }
        if !retry {
            self.attempts.remove(&id);
            self.due.remove(&id);
        }
        match self.manager.start(f, &self.settings) {
            Ok(()) => {
                tracing::info!("started {}", f.summary());
                self.record_session(f, None);
                self.status.insert(id, Status::Starting);
            }
            Err(e) => {
                tracing::warn!("{} failed to start: {e}", f.name);
                if retry && f.auto_reconnect {
                    self.schedule_retry(id, false);
                } else {
                    if !retry {
                        self.record_session(f, Some(e.clone()));
                    }
                    self.status.insert(id, Status::Failed(e));
                }
            }
        }
    }

    /// `failed`: log a session that never ran (start error), otherwise open one.
    fn record_session(&mut self, f: &Forward, failed: Option<String>) {
        let now = Local::now();
        let e = HistoryEntry { session: Uuid::new_v4(), forward: f.clone(), started: now, ended: failed.as_ref().map(|_| now), exit_code: None, error: failed.clone() };
        history::append(&e);
        self.history.push(e.clone());
        if failed.is_none() {
            self.sessions.insert(f.id, e);
        }
    }

    fn close_session(&mut self, id: Uuid, code: Option<i32>, error: Option<String>) {
        if let Some(mut e) = self.sessions.remove(&id) {
            e.ended = Some(Local::now());
            e.exit_code = code;
            e.error = error;
            history::append(&e);
            if let Some(slot) = self.history.iter_mut().rev().find(|h| h.session == e.session) {
                *slot = e;
            }
        }
    }

    /// Manual stop: also cancels a pending reconnect.
    fn stop_tunnel(&mut self, id: Uuid) {
        self.attempts.remove(&id);
        if self.due.remove(&id).is_some() {
            self.status.insert(id, Status::Stopped);
        }
        self.manager.stop(id);
    }

    fn start_group(&mut self, id: Uuid) {
        let todo: Vec<Forward> = self.groups.iter().filter(|g| g.id == id).flat_map(Group::forwards).filter(|f| !self.active(f.id)).collect();
        for f in todo {
            self.start_forward(&f, false);
        }
    }

    fn stop_group(&mut self, id: Uuid) {
        let ids: Vec<Uuid> = self.groups.iter().filter(|g| g.id == id).flat_map(Group::forwards).map(|f| f.id).collect();
        for i in ids {
            self.stop_tunnel(i);
        }
    }

    fn rerun(&mut self, f: Forward) {
        if let Some(cur) = self.find_forward(f.id) {
            return self.start_forward(&cur, false);
        }
        // Source was deleted: recreate it from the snapshot.
        let mut f = f;
        if self.forwards.iter().any(|o| o.name == f.name) {
            f.name = format!("{} (re-run)", f.name);
        }
        self.status.insert(f.id, Status::Stopped);
        self.forwards.push(f.clone());
        self.save();
        self.start_forward(&f, false);
    }

    fn schedule_retry(&mut self, id: Uuid, stable: bool) {
        let n = if stable { 0 } else { self.attempts.get(&id).copied().unwrap_or(0) } + 1;
        self.attempts.insert(id, n);
        self.due.insert(id, Instant::now() + Duration::from_secs(backoff_secs(n)));
        self.status.insert(id, Status::Reconnecting { attempt: n });
    }

    fn run_due_retries(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let ready: Vec<Uuid> = self.due.iter().filter(|(_, t)| **t <= now).map(|(i, _)| *i).collect();
        for id in ready {
            self.due.remove(&id);
            match self.find_forward(id) {
                Some(f) => self.start_forward(&f, true),
                None => {
                    self.attempts.remove(&id);
                    self.status.insert(id, Status::Stopped);
                }
            }
        }
        if let Some(t) = self.due.values().min() {
            ctx.request_repaint_after(t.saturating_duration_since(now));
        }
    }

    // ---- window / tray -------------------------------------------------------------------

    fn set_visible(&mut self, ctx: &egui::Context, visible: bool) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(visible));
        if visible {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        self.window_visible = visible;
        ports::WINDOW_VISIBLE.store(visible, Ordering::Relaxed);
    }

    fn quit(&mut self) -> ! {
        self.manager.stop_all();
        // Exit directly: a hidden window may not process a Close command.
        std::process::exit(0)
    }

    fn tray_state(&self) -> TrayState {
        let mut entries: Vec<tray::Entry> =
            self.forwards.iter().map(|f| tray::Entry { id: format!("f:{}", f.id), label: f.name.clone(), checked: self.active(f.id) }).collect();
        entries.extend(self.groups.iter().map(|g| tray::Entry {
            id: format!("g:{}", g.id),
            label: format!("★ {} ({} ports)", g.name, g.ports.len()),
            checked: self.group_active(g.id),
        }));
        let active = self.status.values().filter(|s| matches!(s, Status::Running { .. })).count();
        tray::TrayState { entries, active }
    }

    // ---- events --------------------------------------------------------------------------

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Ports(p) => self.ports = p,
                AppEvent::Hosts(h) => self.hosts = h,
                AppEvent::Tray(a) => match a {
                    TrayAction::ToggleWindow => self.set_visible(ctx, !self.window_visible),
                    TrayAction::Show => self.set_visible(ctx, true),
                    TrayAction::ToggleForward(id) => {
                        if self.active(id) {
                            self.stop_tunnel(id)
                        } else {
                            self.start(id)
                        }
                    }
                    TrayAction::ToggleGroup(id) => {
                        if self.group_active(id) {
                            self.stop_group(id)
                        } else {
                            self.start_group(id)
                        }
                    }
                    TrayAction::Quit => self.quit(),
                },
                AppEvent::Tunnel(TunnelEvent::Running { id, pid }) => {
                    if self.manager.pid_of(id) == Some(pid) {
                        self.status.insert(id, Status::Running { pid, since: Local::now() });
                    }
                }
                AppEvent::Tunnel(TunnelEvent::Exited { id, pid, code, stopped, stderr }) => {
                    if !self.manager.reap(id, pid) {
                        continue;
                    }
                    let msg = if stderr.trim().is_empty() { "ssh exited unexpectedly".to_string() } else { stderr };
                    self.close_session(id, code, (!stopped).then(|| msg.clone()));
                    if stopped {
                        self.status.insert(id, Status::Stopped);
                        continue;
                    }
                    let name = self.tunnel_name(id);
                    tracing::warn!("{name} exited ({code:?}): {msg}");
                    notify(format!("Tunnel failed: {name}"), msg.lines().last().unwrap_or("").to_string());
                    if self.find_forward(id).is_some_and(|f| f.auto_reconnect) {
                        let stable = matches!(self.status.get(&id), Some(Status::Running { since, .. }) if (Local::now() - *since).num_seconds() >= STABLE_AFTER_SECS);
                        self.schedule_retry(id, stable);
                    } else {
                        self.status.insert(id, Status::Failed(msg));
                    }
                }
            }
        }
    }

    fn pick_host(&mut self, alias: String) {
        if let Some(d) = &mut self.group_draft {
            d.ssh_host = alias;
        } else if let Some(d) = &mut self.draft {
            d.ssh_host = alias;
        } else {
            let mut d = Draft::new(self.settings.reconnect_default);
            d.ssh_host = alias;
            self.draft = Some(d);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.run_due_retries(&ctx);
        let tray_state = self.tray_state();
        if let Some(t) = &mut self.tray {
            t.update(tray_state);
        }
        // Close button hides to the tray (only if there is a tray to restore from).
        if self.tray.is_some() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.set_visible(&ctx, false);
        }

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Forwards, "Forwards");
                ui.selectable_value(&mut self.tab, Tab::Ports, "Ports");
                ui.selectable_value(&mut self.tab, Tab::History, "History");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
        });
        if self.tab == Tab::Forwards && self.show_hosts {
            egui::Panel::right("hosts").default_size(240.0).show(ui, |ui| self.hosts_panel(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(n) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::LIGHT_RED, n);
                    if ui.small_button("✕").clicked() {
                        self.notice = None;
                    }
                });
            }
            match self.tab {
                Tab::Forwards => self.forwards_tab(ui),
                Tab::Ports => self.ports_tab(ui),
                Tab::History => self.history_tab(ui),
                Tab::Settings => self.settings_tab(ui),
            }
        });
        self.draft_window(&ctx);
        self.delete_window(&ctx);
        self.group_window(&ctx);
        self.delete_group_window(&ctx);
    }

    fn on_exit(&mut self) {
        self.manager.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_cap() {
        let v: Vec<u64> = (1..=9).map(backoff_secs).collect();
        assert_eq!(v, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    }
}
