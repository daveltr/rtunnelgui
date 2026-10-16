use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::app::{AppEvent, Bus};
use crate::model::{Forward, Kind, Settings};
use crate::platform;
use crate::ports;

const ALIVE_AFTER: Duration = Duration::from_secs(2);
const STDERR_LINES: usize = 20;

pub enum TunnelEvent {
    Running { id: Uuid, pid: u32 },
    Exited { id: Uuid, pid: u32, code: Option<i32>, stopped: bool, stderr: String },
}

pub fn build_args(f: &Forward) -> Vec<String> {
    let mut a: Vec<String> = ["-N", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=3", "-o", "BatchMode=yes"]
        .map(String::from)
        .into();
    if let Some(p) = f.ssh_port {
        a.extend(["-p".into(), p.to_string()]);
    }
    if let Some(i) = &f.identity {
        a.extend(["-i".into(), i.to_string_lossy().into_owned()]);
    }
    a.extend(f.extra_opts.iter().cloned());
    let bind = f.bind_addr.as_deref().map(|b| format!("{b}:")).unwrap_or_default();
    match f.kind {
        Kind::Local => a.extend(["-L".into(), format!("{bind}{}:{}:{}", f.bind_port, f.target_host, f.target_port)]),
        Kind::Remote => a.extend(["-R".into(), format!("{bind}{}:{}:{}", f.bind_port, f.target_host, f.target_port)]),
        Kind::Dynamic => a.extend(["-D".into(), format!("{bind}{}", f.bind_port)]),
    }
    a.push(f.ssh_host.clone());
    a
}

/// settings override → PATH → Windows built-in OpenSSH.
pub fn ssh_path(s: &Settings) -> PathBuf {
    if let Some(p) = &s.ssh_path {
        return p.clone();
    }
    let win = PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe");
    if cfg!(windows) && win.exists() && !on_path("ssh.exe") {
        return win;
    }
    "ssh".into()
}

fn on_path(exe: &str) -> bool {
    find_on_path(exe).is_some()
}

fn find_on_path(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.exists())
}

/// For display in Settings.
pub fn detect_ssh() -> Option<PathBuf> {
    find_on_path(if cfg!(windows) { "ssh.exe" } else { "ssh" }).or_else(|| Some(PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe")).filter(|p| cfg!(windows) && p.exists()))
}

/// macOS apps launched from Finder have no SSH_AUTH_SOCK; ask launchd for it.
fn launchd_auth_sock() -> Option<String> {
    if !cfg!(target_os = "macos") || std::env::var_os("SSH_AUTH_SOCK").is_some() {
        return None;
    }
    let out = Command::new("launchctl").args(["getenv", "SSH_AUTH_SOCK"]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

pub fn build_command(f: &Forward, s: &Settings) -> Command {
    let mut c = Command::new(ssh_path(s));
    c.args(build_args(f)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    if let Some(sock) = s.ssh_auth_sock.clone().or_else(launchd_auth_sock) {
        c.env("SSH_AUTH_SOCK", sock);
    }
    platform::configure(&mut c);
    c
}

struct Handle {
    pid: u32,
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
}

pub struct TunnelManager {
    bus: Bus,
    children: HashMap<Uuid, Handle>,
}

impl TunnelManager {
    pub fn new(bus: Bus) -> Self {
        Self { bus, children: HashMap::new() }
    }

    pub fn pid_of(&self, id: Uuid) -> Option<u32> {
        self.children.get(&id).map(|h| h.pid)
    }

    pub fn pids(&self) -> HashMap<u32, Uuid> {
        self.children.iter().map(|(id, h)| (h.pid, *id)).collect()
    }

    pub fn start(&mut self, f: &Forward, s: &Settings) -> Result<(), String> {
        if self.children.contains_key(&f.id) {
            return Ok(());
        }
        if f.kind != Kind::Remote {
            ports::check_bindable(f.bind_addr.as_deref().unwrap_or("127.0.0.1"), f.bind_port)?;
        }
        let mut child = build_command(f, s).spawn().map_err(|e| format!("cannot run ssh: {e}"))?;
        platform::adopt(&child);
        let pid = child.id();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let stopped = Arc::new(AtomicBool::new(false));
        self.children.insert(f.id, Handle { pid, child: child.clone(), stopped: stopped.clone() });

        let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let reader = stderr.map(|e| {
            let tail = tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(e).lines().map_while(Result::ok) {
                    let mut t = tail.lock().unwrap();
                    if t.len() == STDERR_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            })
        });

        let (bus, id) = (self.bus.clone(), f.id);
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let mut announced = false;
            let status = loop {
                // ponytail: polling try_wait (not blocking wait) so stop() can share the Child; 100ms latency is fine.
                if let Some(st) = child.lock().unwrap().try_wait().ok().flatten() {
                    break st;
                }
                if !announced && t0.elapsed() >= ALIVE_AFTER {
                    announced = true;
                    bus.send(AppEvent::Tunnel(TunnelEvent::Running { id, pid }));
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            // Let the reader drain, but a grandchild (e.g. ProxyCommand) can hold the pipe open: cap the wait.
            if let Some(r) = reader {
                for _ in 0..5 {
                    if r.is_finished() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let stderr = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
            bus.send(AppEvent::Tunnel(TunnelEvent::Exited { id, pid, code: status.code(), stopped: stopped.load(Ordering::SeqCst), stderr }));
        });
        Ok(())
    }

    /// Kills the child; the watcher then emits `Exited { stopped: true }`.
    pub fn stop(&mut self, id: Uuid) {
        if let Some(h) = self.children.get(&id) {
            h.stopped.store(true, Ordering::SeqCst);
            let _ = h.child.lock().unwrap().kill();
        }
    }

    /// Drop the handle for an exited child. False if the event was stale (id restarted with another pid).
    pub fn reap(&mut self, id: Uuid, pid: u32) -> bool {
        let current = self.pid_of(id) == Some(pid);
        if current {
            self.children.remove(&id);
        }
        current
    }

    pub fn stop_all(&mut self) {
        for (_, h) in self.children.drain() {
            let mut c = h.child.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fwd(kind: Kind) -> Forward {
        Forward {
            id: Uuid::nil(),
            name: "t".into(),
            kind,
            bind_addr: None,
            bind_port: 8080,
            target_host: "db".into(),
            target_port: 5432,
            ssh_host: "bastion".into(),
            ssh_port: None,
            identity: None,
            extra_opts: vec![],
            auto_start: false,
            auto_reconnect: false,
        }
    }

    #[test]
    fn args_per_kind() {
        let tail = |k| build_args(&fwd(k)).split_off(9); // after the 9 fixed leading args
        assert_eq!(tail(Kind::Local), ["-L", "8080:db:5432", "bastion"]);
        assert_eq!(tail(Kind::Remote), ["-R", "8080:db:5432", "bastion"]);
        assert_eq!(tail(Kind::Dynamic), ["-D", "8080", "bastion"]);
    }

    #[test]
    fn args_optionals() {
        let mut f = fwd(Kind::Local);
        f.bind_addr = Some("0.0.0.0".into());
        f.ssh_port = Some(2222);
        f.identity = Some("/k/id".into());
        f.extra_opts = vec!["-J".into(), "jump".into()];
        assert_eq!(build_args(&f).split_off(9), ["-p", "2222", "-i", "/k/id", "-J", "jump", "-L", "0.0.0.0:8080:db:5432", "bastion"]);
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_with_fake_ssh() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("stm-fake-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            Settings { ssh_path: Some(p), ..Default::default() }
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut m = TunnelManager::new(Bus { tx, ctx: eframe::egui::Context::default() });
        let mut f = fwd(Kind::Remote); // Remote skips the local bind check
        let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // dies immediately => Exited with stderr, never Running
        m.start(&f, &script("bad", "echo 'bind failed' >&2; exit 255")).unwrap();
        match next() {
            AppEvent::Tunnel(TunnelEvent::Exited { stopped, stderr, .. }) => assert!(!stopped && stderr == "bind failed"),
            _ => panic!("expected Exited"),
        }

        // survives => Running after 2s, then stop => Exited{stopped}
        f.id = Uuid::new_v4();
        m.start(&f, &script("good", "sleep 30")).unwrap();
        assert!(matches!(next(), AppEvent::Tunnel(TunnelEvent::Running { .. })));
        m.stop(f.id);
        assert!(matches!(next(), AppEvent::Tunnel(TunnelEvent::Exited { stopped: true, .. })));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
