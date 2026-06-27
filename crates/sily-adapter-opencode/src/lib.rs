//! # sily-adapter-opencode
//!
//! Read-only listing of OpenCode sessions. OpenCode stores everything in a
//! SQLite database (default `~/.local/share/opencode/opencode.db`). The `session`
//! table holds one row per session: `id`, `directory` (the cwd), `title` (a human
//! summary), and `time_updated` (epoch ms). Message counts come from the
//! `message` table.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

use sily_core::error::{Error, Result};
use sily_core::model::SessionMeta;
use sily_core::store::{ProjectSessions, SessionRef};

pub const PROVIDER: &str = "opencode";

fn io_err(e: impl std::fmt::Display) -> Error {
    Error::Io(std::io::Error::other(e.to_string()))
}

/// Enumerate every OpenCode session in the database, grouped by directory (cwd).
pub fn list_all_projects(db_path: &Path) -> Result<Vec<ProjectSessions>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    // Read-only; don't create or modify the DB.
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io_err)?;

    // message counts per session in one pass
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id")
            .map_err(io_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(io_err)?;
        for row in rows {
            let (sid, n) = row.map_err(io_err)?;
            counts.insert(sid, n as usize);
        }
    }

    let mut by_cwd: std::collections::BTreeMap<String, Vec<SessionRef>> = std::collections::BTreeMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, directory, title, time_updated FROM session")
            .map_err(io_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (id, directory, title, time_updated) = row.map_err(io_err)?;
            if directory.is_empty() {
                continue;
            }
            let modified = time_updated
                .filter(|&t| t > 0)
                .map(|t| UNIX_EPOCH + Duration::from_millis(t as u64));
            let message_count = counts.get(&id).copied().unwrap_or(0);
            by_cwd.entry(directory.clone()).or_default().push(SessionRef {
                id,
                summary: title.chars().take(80).collect(),
                message_count,
                modified,
                meta: SessionMeta {
                    cwd: Some(directory),
                    provider: Some(PROVIDER.to_string()),
                },
            });
        }
    }

    // newest first within each project
    Ok(by_cwd
        .into_iter()
        .map(|(cwd, mut sessions)| {
            sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
            ProjectSessions { cwd, sessions }
        })
        .collect())
}

/// Default OpenCode database path: `~/.local/share/opencode/opencode.db`.
pub fn default_db_path(home: &Path) -> std::path::PathBuf {
    home.join(".local/share/opencode/opencode.db")
}

// ------------------------------------------------------------------ branching

use std::process::Command;
use serde_json::Value;

pub struct Branched {
    pub new_id: Option<String>,
    pub resume: Option<String>,
    pub kept_messages: usize,
}

/// (message id, role, snippet) for each message, so a caller can choose a branch
/// point. Uses `opencode export` (read-only).
pub fn message_points(session_id: &str) -> Result<Vec<(String, String, String)>> {
    let json = export(session_id)?;
    let mut out = Vec::new();
    if let Some(msgs) = json.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let info = m.get("info");
            let id = info.and_then(|i| i.get("id")).and_then(Value::as_str).unwrap_or("").to_string();
            let role = info.and_then(|i| i.get("role")).and_then(Value::as_str).unwrap_or("").to_string();
            out.push((id, role, message_text(m)));
        }
    }
    Ok(out)
}

/// Branch a session through OpenCode's own export/import (no direct DB writes).
/// Slices messages up to `at_msg` (inclusive; `None` = whole session), imports
/// the result as a new session, and returns its id.
pub fn branch(session_id: &str, at_msg: Option<&str>) -> Result<Branched> {
    let mut json = export(session_id)?;

    let kept = {
        let msgs = json
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| io_err("export has no messages array"))?;
        if let Some(at) = at_msg {
            if let Some(pos) = msgs.iter().position(|m| {
                m.get("info").and_then(|i| i.get("id")).and_then(Value::as_str) == Some(at)
            }) {
                msgs.truncate(pos + 1);
            } else {
                return Err(io_err(format!("message {at} not found in session")));
            }
        }
        msgs.len()
    };

    // Mark provenance; let import mint a fresh session id.
    if let Some(info) = json.get_mut("info").and_then(Value::as_object_mut) {
        info.insert("parentID".to_string(), Value::String(session_id.to_string()));
    }

    let tmp = std::env::temp_dir().join(format!("sily-oc-branch-{}.json", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(&json).map_err(io_err)?)?;

    let out = Command::new("opencode")
        .arg("import")
        .arg(&tmp)
        .output()
        .map_err(|e| io_err(format!("failed to run opencode import: {e}")))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(io_err(format!(
            "opencode import failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    // Find the new (different) session id in the import output.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let new_id = find_new_session_id(&combined, session_id);
    let resume = new_id.as_ref().map(|i| format!("opencode --session {i}"));
    Ok(Branched { new_id, resume, kept_messages: kept })
}

/// Create a new OpenCode session seeded with a single user message, via
/// `opencode import` (no direct DB writes). Returns the new id + resume command.
pub fn create_session(directory: &str, first_user_text: &str) -> Result<Branched> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let doc = serde_json::json!({
        "info": {
            "directory": directory,
            "title": "Ported by sily",
            "version": "sily",
            "time": { "created": now_ms, "updated": now_ms }
        },
        "messages": [
            {
                "info": { "role": "user", "time": { "created": now_ms } },
                "parts": [ { "type": "text", "text": first_user_text } ]
            }
        ]
    });

    let tmp = std::env::temp_dir().join(format!("sily-oc-port-{}.json", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(&doc).map_err(io_err)?)?;
    let out = Command::new("opencode")
        .arg("import")
        .arg(&tmp)
        .output()
        .map_err(|e| io_err(format!("failed to run opencode import: {e}")))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(io_err(format!(
            "opencode import failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let new_id = find_new_session_id(&combined, "");
    let resume = new_id.as_ref().map(|i| format!("opencode --session {i}"));
    Ok(Branched { new_id, resume, kept_messages: 1 })
}

fn export(session_id: &str) -> Result<Value> {
    let out = Command::new("opencode")
        .arg("export")
        .arg(session_id)
        .output()
        .map_err(|e| io_err(format!("failed to run opencode export: {e}")))?;
    if !out.status.success() {
        return Err(io_err(format!(
            "opencode export failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    serde_json::from_slice(&out.stdout).map_err(io_err)
}

fn message_text(m: &Value) -> String {
    m.get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// Scan text for the first `ses_…` token that isn't the source id.
fn find_new_session_id(text: &str, source: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &text[i..i + 4] == "ses_" {
            let start = i;
            let mut j = i + 4;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            let tok = &text[start..j];
            if tok.len() > 8 && tok != source {
                best = Some(tok.to_string());
                break;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    best
}
