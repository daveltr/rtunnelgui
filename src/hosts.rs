use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::app::{AppEvent, Bus};

const MAX_INCLUDE_DEPTH: usize = 16;
const RESOLVE_THREADS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HostSource {
    Config,
    KnownHosts,
    History,
}

#[derive(Clone, Debug)]
pub struct KnownHost {
    pub alias: String,
    pub user: Option<String>,
    pub hostname: String,
    pub port: u16,
    pub identity: Option<PathBuf>,
    pub source: HostSource,
}

fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// "Keyword value" or "Keyword=value"; comments stripped, keyword lowercased.
fn keyword_value(line: &str) -> Option<(String, &str)> {
    let line = line.split('#').next()?.trim();
    let i = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let value = line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim();
    Some((line[..i].to_lowercase(), value)).filter(|(_, v)| !v.is_empty())
}

/// Collect concrete `Host` aliases, following `Include`. `files` gets every file read (for mtime watching).
pub fn parse_config(path: &Path, ssh_dir: &Path, depth: usize, visited: &mut HashSet<PathBuf>, aliases: &mut Vec<String>, files: &mut Vec<PathBuf>) {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if depth > MAX_INCLUDE_DEPTH || !visited.insert(canon) {
        return;
    }
    files.push(path.to_path_buf());
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let Some((key, value)) = keyword_value(line) else { continue };
        match key.as_str() {
            "host" => {
                for a in value.split_whitespace().map(|a| a.trim_matches('"')) {
                    if !a.contains(['*', '?']) && !a.starts_with('!') && !aliases.iter().any(|x| x == a) {
                        aliases.push(a.to_string());
                    }
                }
            }
            "include" => {
                for pat in value.split_whitespace().map(|p| p.trim_matches('"')) {
                    let full = match (pat.strip_prefix("~/"), home()) {
                        (Some(rest), Some(h)) => h.join(rest),
                        _ if Path::new(pat).is_absolute() => PathBuf::from(pat),
                        _ => ssh_dir.join(pat),
                    };
                    let Ok(paths) = glob::glob(&full.to_string_lossy()) else { continue };
                    for p in paths.flatten() {
                        parse_config(&p, ssh_dir, depth + 1, visited, aliases, files);
                    }
                }
            }
            _ => {} // Match blocks are ignored; `ssh -G` resolves them correctly anyway.
        }
    }
}

/// Plain (non-hashed) host names from known_hosts, `[host]:port` unwrapped.
pub fn parse_known_hosts(text: &str) -> Vec<(String, u16)> {
    let mut out: Vec<(String, u16)> = vec![];
    for line in text.lines() {
        let Some(first) = line.split_whitespace().next() else { continue };
        if first.starts_with(['|', '@', '#']) {
            continue;
        }
        for h in first.split(',') {
            if h.contains(['*', '?', '!']) {
                continue;
            }
            let (host, port) = match h.strip_prefix('[').and_then(|r| r.split_once("]:")) {
                Some((host, port)) => (host, port.parse().unwrap_or(22)),
                None => (h, 22),
            };
            if !out.iter().any(|(x, p)| x == host && *p == port) {
                out.push((host.to_string(), port));
            }
        }
    }
    out
}

/// `ssh -G alias` → first user/hostname/port/identityfile.
fn resolve(ssh: &Path, alias: &str) -> KnownHost {
    let mut h = KnownHost { alias: alias.into(), user: None, hostname: alias.into(), port: 22, identity: None, source: HostSource::Config };
    let Ok(out) = Command::new(ssh).args(["-G", alias]).output() else { return h };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        match line.split_once(' ') {
            Some(("user", v)) => h.user = Some(v.into()),
            Some(("hostname", v)) => h.hostname = v.into(),
            Some(("port", v)) => h.port = v.parse().unwrap_or(22),
            Some(("identityfile", v)) if h.identity.is_none() => h.identity = Some(v.into()),
            _ => {}
        }
    }
    h
}

fn scan(ssh: &Path) -> (Vec<KnownHost>, Vec<PathBuf>) {
    let Some(dir) = home().map(|h| h.join(".ssh")) else { return (vec![], vec![]) };
    let (mut aliases, mut files) = (vec![], vec![]);
    parse_config(&dir.join("config"), &dir, 0, &mut HashSet::new(), &mut aliases, &mut files);

    // Bounded parallel `ssh -G`.
    let next = AtomicUsize::new(0);
    let resolved = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..RESOLVE_THREADS.min(aliases.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(a) = aliases.get(i) else { break };
                let h = resolve(ssh, a);
                resolved.lock().unwrap().push((i, h));
            });
        }
    });
    let mut resolved = resolved.into_inner().unwrap();
    resolved.sort_by_key(|(i, _)| *i);
    let mut hosts: Vec<KnownHost> = resolved.into_iter().map(|(_, h)| h).collect();

    let kh = dir.join("known_hosts");
    if let Ok(text) = std::fs::read_to_string(&kh) {
        for (host, port) in parse_known_hosts(&text) {
            if !hosts.iter().any(|h| h.alias == host) {
                hosts.push(KnownHost { alias: host.clone(), user: None, hostname: host, port, identity: None, source: HostSource::KnownHosts });
            }
        }
    }
    files.push(kh);
    (hosts, files)
}

fn stamp(files: &[PathBuf]) -> Vec<Option<SystemTime>> {
    files.iter().map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()).collect()
}

/// Scans at startup and again whenever a parsed file's mtime changes (polled every 5s; no `notify` dependency).
pub fn spawn_watcher(bus: Bus, ssh: PathBuf) {
    std::thread::spawn(move || {
        let (mut files, mut seen) = (vec![], vec![]);
        let mut first = true;
        loop {
            if first || stamp(&files) != seen {
                first = false;
                let (hosts, f) = scan(&ssh);
                seen = stamp(&f);
                files = f;
                bus.send(AppEvent::Hosts(hosts));
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn config_include_globs_wildcards_and_recursion() {
        let dir = std::env::temp_dir().join(format!("stm-hosts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("conf.d")).unwrap();
        fs::write(
            dir.join("config"),
            "Include conf.d/*.conf\nHost web db # comment\n  HostName=x\nHost *\nHost !bad ok? *.corp\nHOST   Caps\nInclude config\n",
        )
        .unwrap();
        fs::write(dir.join("conf.d/a.conf"), "Host from-include\n").unwrap();
        let (mut aliases, mut files) = (vec![], vec![]);
        parse_config(&dir.join("config"), &dir, 0, &mut HashSet::new(), &mut aliases, &mut files);
        assert_eq!(aliases, ["from-include", "web", "db", "Caps"]);
        assert_eq!(files.len(), 2); // self-include is skipped by the visited set
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn known_hosts_skips_hashed_and_unwraps_ports() {
        let t = "|1|abc=|def= ssh-rsa AAA\nhost1,10.0.0.1 ssh-ed25519 AAA\n[host2]:2222 ssh-rsa AAA\n*.wild ssh-rsa A\n@cert-authority x ssh-rsa A\nhost1 ssh-rsa dup\n";
        assert_eq!(parse_known_hosts(t), [("host1".to_string(), 22), ("10.0.0.1".to_string(), 22), ("host2".to_string(), 2222)]);
    }
}
