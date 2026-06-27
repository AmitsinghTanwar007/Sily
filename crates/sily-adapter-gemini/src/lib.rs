//! # sily-adapter-gemini
//!
//! Read-only listing of Gemini CLI sessions. Gemini stores conversation logs at
//! `~/.gemini/tmp/<project_hash>/logs.json` — a JSON **array** of entries:
//! `{ sessionId, messageId: number, timestamp, type: "user", message }`.
//!
//! Two consequences shape this adapter:
//! 1. `logs.json` records **only user prompts** (Gemini's log type enum is
//!    user-only), so we surface the prompts, not assistant replies.
//! 2. `<project_hash>` is a one-way hash of the project dir, so the real cwd
//!    can't be recovered — we label projects `gemini:<hash8>`.
//!
//! Because the full conversation and a stable resume-by-id aren't available,
//! branch/revert/port are intentionally unsupported (the trait defaults apply).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use sily_core::error::{Error, Result};
use sily_core::model::{Role, SessionMeta};
use sily_core::provider::{MsgPoint, Provider};
use sily_core::store::{ProjectSessions, SessionRef};

pub const PROVIDER: &str = "gemini-cli";

pub struct GeminiProvider {
    home: PathBuf, // ~/.gemini
}

impl GeminiProvider {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }
}

/// One parsed log entry (only `type: "user"` exists today).
struct Entry {
    session_id: String,
    message: String,
}

/// Read every `tmp/*/logs.json`, returning (hash-label, file-mtime, entries).
fn read_all(home: &Path) -> Vec<(String, Option<SystemTime>, Vec<Entry>)> {
    let mut out = Vec::new();
    let tmp = home.join("tmp");
    let Ok(dirs) = fs::read_dir(&tmp) else { return out };
    for d in dirs.flatten() {
        let logs = d.path().join("logs.json");
        if !logs.exists() {
            continue;
        }
        let hash = d.file_name().to_string_lossy().chars().take(8).collect::<String>();
        let modified = fs::metadata(&logs).ok().and_then(|m| m.modified().ok());
        let Ok(text) = fs::read_to_string(&logs) else { continue };
        let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) else { continue };
        let entries: Vec<Entry> = arr
            .into_iter()
            .filter_map(|v| {
                Some(Entry {
                    session_id: v.get("sessionId")?.as_str()?.to_string(),
                    message: v.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                })
            })
            .collect();
        if !entries.is_empty() {
            out.push((format!("gemini:{hash}"), modified, entries));
        }
    }
    out
}

impl Provider for GeminiProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn owns(&self, id: &str) -> bool {
        read_all(&self.home)
            .iter()
            .any(|(_, _, es)| es.iter().any(|e| e.session_id == id))
    }

    fn list_projects(&self) -> Result<Vec<ProjectSessions>> {
        let mut projects = Vec::new();
        for (label, modified, entries) in read_all(&self.home) {
            // group entries by sessionId, preserving first-seen order
            let mut order: Vec<String> = Vec::new();
            let mut sessions: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for e in entries {
                sessions
                    .entry(e.session_id.clone())
                    .or_insert_with(|| {
                        order.push(e.session_id.clone());
                        Vec::new()
                    })
                    .push(e.message);
            }
            let refs: Vec<SessionRef> = order
                .into_iter()
                .map(|sid| {
                    let msgs = &sessions[&sid];
                    let summary = msgs
                        .iter()
                        .find(|m| !m.trim().is_empty())
                        .map(|m| m.chars().take(80).collect())
                        .unwrap_or_default();
                    SessionRef {
                        id: sid,
                        summary,
                        message_count: msgs.len(),
                        modified,
                        meta: SessionMeta {
                            cwd: Some(label.clone()),
                            provider: Some(PROVIDER.to_string()),
                        },
                    }
                })
                .collect();
            projects.push(ProjectSessions { cwd: label, sessions: refs });
        }
        Ok(projects)
    }

    fn messages(&self, id: &str) -> Result<Vec<MsgPoint>> {
        for (_, _, entries) in read_all(&self.home) {
            let mut points = Vec::new();
            let mut n = 0;
            for e in entries.into_iter().filter(|e| e.session_id == id) {
                n += 1;
                points.push(MsgPoint { point: n.to_string(), role: Role::User, text: e.message });
            }
            if !points.is_empty() {
                return Ok(points);
            }
        }
        Err(Error::SessionNotFound(id.to_string()))
    }

    fn resume_command(&self, _id: &str) -> String {
        // Gemini resumes per-project (gemini --resume), not by session id.
        "gemini --resume".to_string()
    }
}
