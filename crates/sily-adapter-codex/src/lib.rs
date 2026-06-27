//! # sily-adapter-codex
//!
//! Read-only listing of OpenAI Codex CLI sessions. Codex stores each session as
//! a rollout `.jsonl` at `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`.
//! The first record is a `session_meta` (with `id` and `cwd`); conversation turns
//! are `response_item` records of `type: "message"` with a role and a `content`
//! array of typed blocks.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use sily_core::error::Result;
use sily_core::model::SessionMeta;
use sily_core::store::{ProjectSessions, SessionRef};

pub const PROVIDER: &str = "codex-cli";

/// Enumerate every Codex session under `<codex_home>/sessions/`, grouped by cwd.
pub fn list_all_projects(codex_home: &Path) -> Result<Vec<ProjectSessions>> {
    let sessions_dir = codex_home.join("sessions");
    let mut files = Vec::new();
    collect_jsonl(&sessions_dir, &mut files);

    let mut by_cwd: std::collections::BTreeMap<String, Vec<SessionRef>> = std::collections::BTreeMap::new();
    for path in files {
        if let Some((cwd, session)) = scan_session(&path) {
            by_cwd.entry(cwd).or_default().push(session);
        }
    }

    Ok(by_cwd
        .into_iter()
        .map(|(cwd, sessions)| ProjectSessions { cwd, sessions })
        .collect())
}

/// Recursively collect `*.jsonl` files under `dir`.
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Parse one rollout file into (cwd, SessionRef). Returns None if it has no
/// `session_meta` (not a real session file).
fn scan_session(path: &Path) -> Option<(String, SessionRef)> {
    let text = fs::read_to_string(path).ok()?;
    let mut id = None;
    let mut cwd = None;
    let mut count = 0usize;
    let mut summary = String::new();

    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let p = v.get("payload");
                id = p.and_then(|p| p.get("id")).and_then(Value::as_str).map(str::to_string);
                cwd = p.and_then(|p| p.get("cwd")).and_then(Value::as_str).map(str::to_string);
            }
            Some("response_item") => {
                let p = v.get("payload");
                if p.and_then(|p| p.get("type")).and_then(Value::as_str) != Some("message") {
                    continue;
                }
                let role = p.and_then(|p| p.get("role")).and_then(Value::as_str).unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue; // skip developer/system injected context
                }
                count += 1;
                if summary.is_empty() && role == "user" {
                    if let Some(text) = p.and_then(|p| p.get("content")).map(extract_text) {
                        let t = text.trim();
                        if !t.is_empty() && !t.starts_with('<') {
                            summary = t.chars().take(80).collect();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let cwd = cwd?;
    let id = id.or_else(|| path.file_stem().and_then(|s| s.to_str()).map(str::to_string))?;
    let modified = fs::metadata(path).ok().and_then(|m| m.modified().ok());
    Some((
        cwd.clone(),
        SessionRef {
            id,
            summary,
            message_count: count,
            modified,
            meta: SessionMeta { cwd: Some(cwd), provider: Some(PROVIDER.to_string()) },
        },
    ))
}

/// Join the `text` fields of a Codex `content` block array.
fn extract_text(content: &Value) -> String {
    match content {
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}
