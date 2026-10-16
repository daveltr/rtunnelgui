use std::net::{IpAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::app::Bus;

#[derive(Clone, Debug)]
pub struct PortInfo {
    pub port: u16,
    pub addr: IpAddr,
    pub pid: u32,
    pub process: String, // empty when the OS hides it
}

/// TCP listeners only: forwards are TCP.
pub fn scan() -> Vec<PortInfo> {
    let mut v: Vec<PortInfo> = listeners::get_all()
        .map(|set| {
            set.into_iter()
                .filter(|l| l.protocol == listeners::Protocol::TCP)
                .map(|l| PortInfo { port: l.socket.port(), addr: l.socket.ip(), pid: l.process.pid, process: l.process.name })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|p| (p.port, p.pid));
    v
}

/// Set by the UI; the scanner idles while the window is hidden in the tray.
pub static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

pub fn spawn_scanner(bus: Bus) {
    std::thread::spawn(move || loop {
        if WINDOW_VISIBLE.load(Ordering::Relaxed) {
            bus.send(crate::app::AppEvent::Ports(scan()));
        }
        std::thread::sleep(Duration::from_secs(4));
    });
}

/// Ask the OS for a free port.
pub fn pick_free() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok().map(|a| a.port())
}

/// Fail-fast check before spawning ssh for Local/Dynamic forwards.
pub fn check_bindable(addr: &str, port: u16) -> Result<(), String> {
    TcpListener::bind((addr, port)).map(drop).map_err(|e| format!("cannot bind {addr}:{port}: {e}"))
}
