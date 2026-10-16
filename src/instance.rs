//! Single-instance guard: the first instance listens on a loopback port; a second launch pokes it and exits.
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use crate::app::{AppEvent, Bus};
use crate::tray::TrayAction;

const ADDR: &str = "127.0.0.1:47653";
const MAGIC: &str = "ssh-tunnel-manager:show";

pub enum Acquired {
    First(TcpListener),
    /// Port taken by something that isn't us: run without a guard.
    Unguarded,
    /// An existing instance was told to show its window.
    AlreadyRunning,
}

pub fn acquire() -> Acquired {
    if let Ok(l) = TcpListener::bind(ADDR) {
        return Acquired::First(l);
    }
    let poked = (|| {
        let mut s = TcpStream::connect_timeout(&ADDR.parse().ok()?, Duration::from_secs(1)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
        writeln!(s, "{MAGIC}").ok()?;
        let mut reply = String::new();
        BufReader::new(s).read_line(&mut reply).ok()?;
        Some(reply.trim() == "ok")
    })();
    if poked == Some(true) {
        Acquired::AlreadyRunning
    } else {
        Acquired::Unguarded
    }
}

pub fn listen(l: TcpListener, bus: Bus) {
    std::thread::spawn(move || {
        for mut conn in l.incoming().flatten() {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            if BufReader::new(&conn).read_line(&mut line).is_ok() && line.trim() == MAGIC {
                let _ = writeln!(conn, "ok");
                bus.send(AppEvent::Tray(TrayAction::Show));
            }
        }
    });
}
