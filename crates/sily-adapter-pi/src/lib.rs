//! # sily-adapter-pi
//!
//! Read-only adapter for the Pi coding agent. Pi stores each session as a JSONL
//! file at `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`. The first
//! record is a header `{type:"session", version, id, timestamp, cwd}`; the rest
//! are entries `{type:"message", id, parentId, timestamp, message:{role,content}}`
//! forming a DAG (Pi branches in place).
//!
//! sily reads the cwd from the header (so grouping is by real directory) and maps
//! the id/parentId DAG to the canonical model, giving Pi list/log/tree/prompts.
//! Writes (branch/port) are left unsupported pending verification against a real
//! Pi install.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use sily_core::error::{Error, Result};
use sily_core::model::{Message, Role, Session, SessionMeta};
use sily_core::provider::{MsgPoint, Provider};
use sily_core::store::{ProjectSessions, SessionRef};

pub const PROVIDER: &str = "pi";

pub struct PiProvider {
    sessions_dir: PathBuf, // ~/.pi/agent/sessions
}

impl PiProvider {
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self { sessions_dir: sessions_dir.into() }
    }
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_jsonl(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn role_of(s: &str) -> Role {
    match s {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::Other,
    }
}

/// Parsed session file: header id + cwd, plus message entries.
struct Parsed {
    cwd: String,
    messages: Vec<Message>,
}

fn parse(path: &Path) -> Option<Parsed> {
    let text = fs::read_to_string(path).ok()?;
    let mut cwd = None;
    let mut messages = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("session") => {
                cwd = v.get("cwd").and_then(Value::as_str).map(str::to_string);
            }
            Some("message") => {
                let m = v.get("message");
                let role = role_of(m.and_then(|m| m.get("role")).and_then(Value::as_str).unwrap_or(""));
                let text = m.and_then(|m| m.get("content")).map(extract_text).unwrap_or_default();
                messages.push(Message {
                    uuid: v.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    parent_uuid: v.get("parentId").and_then(Value::as_str).map(str::to_string),
                    role,
                    text,
                    timestamp: v.get("timestamp").and_then(Value::as_str).map(str::to_string),
                    extra: v,
                });
            }
            _ => {}
        }
    }
    Some(Parsed { cwd: cwd?, messages })
}

/// The session header id of a file (used to match a session id to a file).
fn header_id(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(Value::as_str) == Some("session") {
                return v.get("id").and_then(Value::as_str).map(str::to_string);
            }
        }
    }
    None
}

impl PiProvider {
    fn find_file(&self, id: &str) -> Option<PathBuf> {
        let mut files = Vec::new();
        collect_jsonl(&self.sessions_dir, &mut files);
        files.into_iter().find(|p| header_id(p).as_deref() == Some(id))
    }
}

impl Provider for PiProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn owns(&self, id: &str) -> bool {
        self.find_file(id).is_some()
    }

    fn list_projects(&self) -> Result<Vec<ProjectSessions>> {
        let mut files = Vec::new();
        collect_jsonl(&self.sessions_dir, &mut files);
        let mut by_cwd: std::collections::BTreeMap<String, Vec<SessionRef>> = Default::default();
        for path in files {
            let Some(parsed) = parse(&path) else { continue };
            let Some(id) = header_id(&path) else { continue };
            let count = parsed
                .messages
                .iter()
                .filter(|m| matches!(m.role, Role::User | Role::Assistant))
                .count();
            let summary = parsed
                .messages
                .iter()
                .find(|m| matches!(m.role, Role::User) && !m.text.trim().is_empty())
                .map(|m| m.text.chars().take(80).collect())
                .unwrap_or_default();
            let modified = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            by_cwd.entry(parsed.cwd.clone()).or_default().push(SessionRef {
                id,
                summary,
                message_count: count,
                modified,
                meta: SessionMeta { cwd: Some(parsed.cwd), provider: Some(PROVIDER.to_string()) },
            });
        }
        Ok(by_cwd
            .into_iter()
            .map(|(cwd, sessions)| ProjectSessions { cwd, sessions })
            .collect())
    }

    fn messages(&self, id: &str) -> Result<Vec<MsgPoint>> {
        let path = self.find_file(id).ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        let parsed = parse(&path).ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        Ok(parsed
            .messages
            .into_iter()
            .map(|m| MsgPoint {
                point: m.uuid,
                role: m.role,
                text: m.text,
                time: m.timestamp.unwrap_or_default(),
            })
            .collect())
    }

    fn resume_command(&self, id: &str) -> String {
        format!("pi --resume {id}")
    }

    fn structured(&self, id: &str) -> Result<Option<Session>> {
        let path = self.find_file(id).ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        let parsed = parse(&path).ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        let mut s = Session::new(id);
        s.meta = SessionMeta { cwd: Some(parsed.cwd), provider: Some(PROVIDER.to_string()) };
        s.messages = parsed.messages;
        Ok(Some(s))
    }
}
