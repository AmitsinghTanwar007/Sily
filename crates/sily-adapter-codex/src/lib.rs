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

use sily_core::error::{Error, Result};
use sily_core::model::SessionMeta;
use sily_core::store::{ProjectSessions, SessionRef};

pub const PROVIDER: &str = "codex-cli";

mod provider;
pub use provider::CodexProvider;

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

/// Outcome of creating a branched Codex session.
pub struct Branched {
    pub new_id: String,
    pub resume: String,
    pub kept_messages: usize,
}

/// Locate the rollout file for a session id (matches `session_meta.payload.id`).
pub fn find_session_file(codex_home: &Path, id: &str) -> Option<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl(&codex_home.join("sessions"), &mut files);
    files
        .into_iter()
        .find(|path| file_session_id(path).as_deref() == Some(id))
}

fn file_session_id(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(Value::as_str) == Some("session_meta") {
                return v
                    .get("payload")
                    .and_then(|p| p.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
    }
    None
}

/// Index (1-based) → display snippet for each user/assistant message, so a caller
/// can pick a branch point (Codex messages have no ids).
pub fn message_points(codex_home: &Path, id: &str) -> Result<Vec<(usize, String, String, String)>> {
    let path = find_session_file(codex_home, id)
        .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
    let text = fs::read_to_string(&path)?;
    let mut points = Vec::new();
    let mut idx = 0;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if is_message(&v) {
            idx += 1;
            let p = v.get("payload");
            let role = p.and_then(|p| p.get("role")).and_then(Value::as_str).unwrap_or("").to_string();
            let text = p
                .and_then(|p| p.get("content"))
                .map(extract_text)
                .unwrap_or_default();
            let time = v.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string();
            points.push((idx, role, text, time));
        }
    }
    Ok(points)
}

fn is_message(v: &Value) -> bool {
    v.get("type").and_then(Value::as_str) == Some("response_item")
        && v.get("payload").and_then(|p| p.get("type")).and_then(Value::as_str) == Some("message")
        && matches!(
            v.get("payload").and_then(|p| p.get("role")).and_then(Value::as_str),
            Some("user") | Some("assistant")
        )
}

/// Slice the raw record stream up to (and including) the `at`-th message (1-based;
/// `None` = the whole session). Returns the kept raw lines and message count.
fn slice_lines(text: &str, at: Option<usize>) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut msgs = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(line.to_string());
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if is_message(&v) {
                msgs += 1;
                if Some(msgs) == at {
                    break;
                }
            }
        }
    }
    (out, msgs)
}

/// Create a NEW Codex session branched from `id` at the `at`-th message
/// (`None` = copy the whole session). Writes a fresh rollout file with a new id.
pub fn branch(codex_home: &Path, id: &str, at: Option<usize>) -> Result<Branched> {
    let path = find_session_file(codex_home, id)
        .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
    let text = fs::read_to_string(&path)?;
    let (mut lines, kept) = slice_lines(&text, at);

    let new_id = uuid::Uuid::new_v4().to_string();
    rewrite_meta_id(&mut lines, &new_id);

    let now = chrono::Utc::now();
    let dir = codex_home
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&dir)?;
    let fname = format!("rollout-{}-{}.jsonl", now.format("%Y-%m-%dT%H-%M-%S"), new_id);
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(dir.join(fname), body)?;

    Ok(Branched {
        new_id: new_id.clone(),
        resume: format!("codex resume {new_id}"),
        kept_messages: kept,
    })
}

/// Merge `branch` into `main`: new rollout = main's full records + the branch's
/// message records after their common (role,text) prefix. Experimental.
pub fn merge(codex_home: &Path, main_id: &str, branch_id: &str) -> Result<Branched> {
    let main_path =
        find_session_file(codex_home, main_id).ok_or_else(|| Error::SessionNotFound(main_id.to_string()))?;
    let branch_path = find_session_file(codex_home, branch_id)
        .ok_or_else(|| Error::SessionNotFound(branch_id.to_string()))?;
    let main_text = fs::read_to_string(&main_path)?;
    let branch_text = fs::read_to_string(&branch_path)?;

    let main_msgs = message_role_text(&main_text);
    // branch message (role,text) paired with their raw lines, in order.
    let mut branch_msgs: Vec<(String, String)> = Vec::new();
    let mut branch_msg_lines: Vec<String> = Vec::new();
    for line in branch_text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if is_message(&v) {
                let p = v.get("payload");
                let role = p.and_then(|p| p.get("role")).and_then(Value::as_str).unwrap_or("").to_string();
                let text = p.and_then(|p| p.get("content")).map(extract_text).unwrap_or_default();
                branch_msgs.push((role, text));
                branch_msg_lines.push(line.to_string());
            }
        }
    }
    let common = main_msgs
        .iter()
        .zip(branch_msgs.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut lines: Vec<String> = main_text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    let new_id = uuid::Uuid::new_v4().to_string();
    rewrite_meta_id(&mut lines, &new_id);
    let tail = &branch_msg_lines[common..];
    lines.extend(tail.iter().cloned());

    let now = chrono::Utc::now();
    let dir = codex_home
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&dir)?;
    let fname = format!("rollout-{}-{}.jsonl", now.format("%Y-%m-%dT%H-%M-%S"), new_id);
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(dir.join(fname), body)?;

    Ok(Branched {
        new_id: new_id.clone(),
        resume: format!("codex resume {new_id}"),
        kept_messages: main_msgs.len() + tail.len(),
    })
}

/// (role, text) for each user/assistant message record, in file order.
fn message_role_text(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if is_message(&v) {
                let p = v.get("payload");
                let role = p.and_then(|p| p.get("role")).and_then(Value::as_str).unwrap_or("").to_string();
                let t = p.and_then(|p| p.get("content")).map(extract_text).unwrap_or_default();
                out.push((role, t));
            }
        }
    }
    out
}

/// Create a brand-new Codex session seeded with a single user message (used by
/// cross-provider porting). Returns (new_id, resume command).
pub fn create_session(codex_home: &Path, cwd: &str, first_user_text: &str) -> Result<(String, String)> {
    let new_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let ts = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let meta = serde_json::json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": { "id": new_id, "timestamp": ts, "cwd": cwd,
                     "originator": "sily", "cli_version": "sily", "source": "sily" }
    });
    let msg = serde_json::json!({
        "timestamp": ts,
        "type": "response_item",
        "payload": { "type": "message", "role": "user",
                     "content": [ { "type": "input_text", "text": first_user_text } ] }
    });

    let dir = codex_home
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&dir)?;
    let fname = format!("rollout-{}-{}.jsonl", now.format("%Y-%m-%dT%H-%M-%S"), new_id);
    let body = format!("{}\n{}\n", serde_json::to_string(&meta)?, serde_json::to_string(&msg)?);
    fs::write(dir.join(fname), body)?;

    Ok((new_id.clone(), format!("codex resume {new_id}")))
}

/// Destructive: truncate the original session file at the `at`-th message.
pub fn truncate(codex_home: &Path, id: &str, at: usize) -> Result<usize> {
    let path = find_session_file(codex_home, id)
        .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
    let text = fs::read_to_string(&path)?;
    let (lines, kept) = slice_lines(&text, Some(at));
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(&path, body)?;
    Ok(kept)
}

/// Set `session_meta.payload.id` to `new_id` in the first session_meta line.
fn rewrite_meta_id(lines: &mut [String], new_id: &str) {
    for line in lines.iter_mut() {
        if let Ok(mut v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(Value::as_str) == Some("session_meta") {
                if let Some(p) = v.get_mut("payload").and_then(|p| p.as_object_mut()) {
                    p.insert("id".to_string(), Value::String(new_id.to_string()));
                }
                if let Ok(s) = serde_json::to_string(&v) {
                    *line = s;
                }
                break;
            }
        }
    }
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
