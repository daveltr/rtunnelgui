use std::path::PathBuf;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Forward {
    pub id: Uuid,
    pub name: String,
    pub kind: Kind,
    pub bind_addr: Option<String>, // default 127.0.0.1
    pub bind_port: u16,
    pub target_host: String, // unused for Dynamic
    pub target_port: u16,    // unused for Dynamic
    pub ssh_host: String,    // alias or user@host
    pub ssh_port: Option<u16>,
    pub identity: Option<PathBuf>,
    pub extra_opts: Vec<String>, // e.g. ["-J", "bastion"]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_reconnect: bool,
}

impl Forward {
    pub fn summary(&self) -> String {
        let bind = self.bind_addr.as_deref().map(|a| format!("{a}:")).unwrap_or_default();
        match self.kind {
            Kind::Local => format!("L {bind}{} → {}:{} via {}", self.bind_port, self.target_host, self.target_port, self.ssh_host),
            Kind::Remote => format!("R {bind}{} → {}:{} via {}", self.bind_port, self.target_host, self.target_port, self.ssh_host),
            Kind::Dynamic => format!("D {bind}{} via {}", self.bind_port, self.ssh_host),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Stopped,
    Starting,
    Reconnecting { attempt: u32 },
    Running { pid: u32, since: DateTime<Local> },
    Failed(String),
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Settings {
    pub ssh_path: Option<PathBuf>,
    pub ssh_auth_sock: Option<String>,
    pub start_minimized: bool,
    pub launch_at_login: bool,
    pub reconnect_default: bool,
}

/// A favorite server plus several ports; starting it launches one Local forward per port.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub ssh_host: String,
    pub ssh_port: Option<u16>,
    pub identity: Option<PathBuf>,
    pub extra_opts: Vec<String>,
    pub ports: Vec<PortMap>,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_reconnect: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct PortMap {
    pub local: u16,
    pub remote: u16,
}

impl Group {
    /// Forward ids are derived from the group id + local port, so they are stable across runs and edits.
    pub fn forwards(&self) -> Vec<Forward> {
        self.ports
            .iter()
            .map(|p| Forward {
                id: Uuid::new_v5(&self.id, &p.local.to_be_bytes()),
                name: format!("{} :{}", self.name, p.local),
                kind: Kind::Local,
                bind_addr: None,
                bind_port: p.local,
                target_host: "localhost".into(),
                target_port: p.remote,
                ssh_host: self.ssh_host.clone(),
                ssh_port: self.ssh_port,
                identity: self.identity.clone(),
                extra_opts: self.extra_opts.clone(),
                auto_start: self.auto_start,
                auto_reconnect: self.auto_reconnect,
            })
            .collect()
    }
}

/// "3000 9002, 8080:80" → local[:remote] pairs; remote defaults to local.
pub fn parse_ports(s: &str) -> Result<Vec<PortMap>, String> {
    let port = |t: &str| match t.parse::<u16>() {
        Ok(p) if p >= 1 => Ok(p),
        _ => Err(format!("\"{t}\" is not a port 1–65535")),
    };
    let mut out: Vec<PortMap> = vec![];
    for tok in s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()) {
        let (l, r) = tok.split_once(':').unwrap_or((tok, tok));
        let m = PortMap { local: port(l)?, remote: port(r)? };
        if out.iter().any(|o| o.local == m.local) {
            return Err(format!("local port {} listed twice", m.local));
        }
        out.push(m);
    }
    if out.is_empty() {
        return Err("add at least one port".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_parse() {
        let p = parse_ports("3000, 9002 8080:80").unwrap();
        assert_eq!(p, [PortMap { local: 3000, remote: 3000 }, PortMap { local: 9002, remote: 9002 }, PortMap { local: 8080, remote: 80 }]);
        assert!(parse_ports("").is_err() && parse_ports("0").is_err() && parse_ports("80 80:81").is_err() && parse_ports("x").is_err());
    }

    #[test]
    fn group_forward_ids_are_stable() {
        let g = Group { id: Uuid::new_v4(), name: "Pluto".into(), ssh_host: "pluto".into(), ssh_port: None, identity: None, extra_opts: vec![], ports: parse_ports("3000 80").unwrap(), auto_start: false, auto_reconnect: false };
        let (a, b) = (g.forwards(), g.forwards());
        assert_eq!(a[0].id, b[0].id);
        assert_ne!(a[0].id, a[1].id);
        assert_eq!(a[1].summary(), "L 80 → localhost:80 via pluto");
    }
}
