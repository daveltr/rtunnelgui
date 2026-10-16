use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::Forward;
use crate::store;

const MAX_ENTRIES: usize = 5000;

/// One tunnel session. Written once on start and again (same `session`) on end; the later line wins on load.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryEntry {
    pub session: Uuid,
    pub forward: Forward, // snapshot, so re-run works after edits/deletes
    pub started: DateTime<Local>,
    pub ended: Option<DateTime<Local>>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

fn path() -> Result<PathBuf> {
    Ok(store::data_dir()?.join("history.jsonl"))
}

pub fn append(e: &HistoryEntry) {
    let r: Result<()> = (|| {
        let p = path()?;
        fs::create_dir_all(p.parent().unwrap())?;
        let mut f = OpenOptions::new().create(true).append(true).open(p)?;
        writeln!(f, "{}", serde_json::to_string(e)?)?;
        Ok(())
    })();
    if let Err(e) = r {
        tracing::warn!("history append failed: {e:#}");
    }
}

pub fn clear() -> Result<()> {
    store::write_atomic(&path()?, b"")
}

/// Collapse start/end line pairs by session and keep the newest `max`. Unparseable lines are dropped.
fn fold(text: &str, max: usize) -> Vec<HistoryEntry> {
    let mut out: Vec<HistoryEntry> = vec![];
    let mut at: HashMap<Uuid, usize> = HashMap::new();
    for e in text.lines().filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok()) {
        match at.get(&e.session) {
            Some(&i) => out[i] = e,
            None => {
                at.insert(e.session, out.len());
                out.push(e);
            }
        }
    }
    let skip = out.len().saturating_sub(max);
    out.drain(..skip);
    out
}

/// Load, compacting the file when it holds duplicates or exceeds the cap.
pub fn load() -> Vec<HistoryEntry> {
    let Ok(p) = path() else { return vec![] };
    let Ok(text) = fs::read_to_string(&p) else { return vec![] };
    let entries = fold(&text, MAX_ENTRIES);
    if entries.len() != text.lines().count() {
        let body: String = entries.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        if let Err(e) = store::write_atomic(&p, body.as_bytes()) {
            tracing::warn!("history compaction failed: {e:#}");
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Forward, Kind};

    fn entry(session: Uuid, error: Option<&str>) -> String {
        let e = HistoryEntry {
            session,
            forward: Forward {
                id: Uuid::nil(),
                name: "n".into(),
                kind: Kind::Local,
                bind_addr: None,
                bind_port: 1,
                target_host: "h".into(),
                target_port: 2,
                ssh_host: "s".into(),
                ssh_port: None,
                identity: None,
                extra_opts: vec![],
                auto_start: false,
                auto_reconnect: false,
            },
            started: Local::now(),
            ended: error.map(|_| Local::now()),
            exit_code: None,
            error: error.map(String::from),
        };
        serde_json::to_string(&e).unwrap() + "\n"
    }

    #[test]
    fn fold_merges_sessions_skips_garbage_and_trims() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let text = entry(a, None) + &entry(b, None) + "not json\n" + &entry(a, Some("boom")) + &entry(c, None);
        let all = fold(&text, 10);
        assert_eq!(all.iter().map(|e| e.session).collect::<Vec<_>>(), [a, b, c]);
        assert_eq!(all[0].error.as_deref(), Some("boom"));
        let last2 = fold(&text, 2);
        assert_eq!(last2.iter().map(|e| e.session).collect::<Vec<_>>(), [b, c]);
    }
}
