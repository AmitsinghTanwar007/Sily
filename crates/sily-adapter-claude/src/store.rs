//! The filesystem-facing [`SessionStore`] implementation for Claude Code.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use sily_core::error::{Error, Result};
use sily_core::model::{Session, SessionMeta};
use sily_core::store::{SessionRef, SessionStore};

use crate::convert::{extract_text, message_to_record, record_to_message, rewrite_session_id, PROVIDER};
use crate::encode::encode_cwd;

pub use sily_core::store::ProjectSessions;

/// Enumerate every project under `<claude_home>/projects/`, with its sessions.
/// The real cwd is read from inside a session file (folder-name encoding is
/// lossy and can't be reliably reversed).
pub fn list_all_projects(claude_home: &Path) -> Result<Vec<ProjectSessions>> {
    let projects_dir = claude_home.join("projects");
    let mut out = Vec::new();
    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(Error::Io(e)),
    };
    for entry in entries {
        let dir = entry?.path();
        if !dir.is_dir() {
            continue;
        }
        let mut cwd: Option<String> = None;
        let mut sessions = Vec::new();
        let files = match fs::read_dir(&dir) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for f in files {
            let path = f?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if cwd.is_none() {
                cwd = read_cwd(&path);
            }
            let modified = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            let (summary, message_count) = scan_session(&path);
            sessions.push(SessionRef {
                id: id.to_string(),
                summary,
                message_count,
                modified,
                meta: SessionMeta {
                    cwd: cwd.clone(),
                    provider: Some(PROVIDER.to_string()),
                },
            });
        }
        if sessions.is_empty() {
            continue;
        }
        let cwd = cwd.unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
        out.push(ProjectSessions { cwd, sessions });
    }
    Ok(out)
}

/// Find a session by id across ALL projects under the Claude home, returning the
/// project dir it lives in and its recorded cwd. Lets commands work regardless of
/// the current working directory.
pub fn locate(claude_home: &Path, id: &str) -> Option<(PathBuf, String)> {
    let projects = claude_home.join("projects");
    let entries = fs::read_dir(&projects).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let file = dir.join(format!("{id}.jsonl"));
        if file.exists() {
            let cwd = read_cwd(&file).unwrap_or_default();
            return Some((dir, cwd));
        }
    }
    None
}

/// Read the `cwd` field from the first record that has one.
fn read_cwd(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                return Some(c.to_string());
            }
        }
    }
    None
}

/// A store scoped to one project (one `cwd`) under a Claude home directory.
pub struct ClaudeStore {
    /// The `projects/<encoded-cwd>` directory this store reads and writes.
    project_dir: PathBuf,
    cwd: String,
}

impl ClaudeStore {
    /// `claude_home` is typically `~/.claude`. The store targets the project
    /// folder for `cwd`.
    pub fn new(claude_home: impl AsRef<Path>, cwd: impl Into<String>) -> Self {
        let cwd = cwd.into();
        let project_dir = claude_home.as_ref().join("projects").join(encode_cwd(&cwd));
        Self { project_dir, cwd }
    }

    /// Build a store pointed directly at a known project dir (used when a session
    /// is located by id in a project other than the current cwd).
    pub fn from_project_dir(project_dir: PathBuf, cwd: String) -> Self {
        Self { project_dir, cwd }
    }

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.project_dir.join(format!("{id}.jsonl"))
    }

    fn meta(&self) -> SessionMeta {
        SessionMeta {
            cwd: Some(self.cwd.clone()),
            provider: Some(PROVIDER.to_string()),
        }
    }
}

impl SessionStore for ClaudeStore {
    fn load(&self, id: &str) -> Result<Session> {
        let path = self.session_path(id);
        let text = fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::SessionNotFound(id.to_string()),
            _ => Error::Io(e),
        })?;

        let mut session = Session::new(id);
        session.meta = self.meta();

        let mut skipped = 0usize;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Real session files can contain a malformed line from an
            // interrupted/crashed write (e.g. two records concatenated with no
            // newline). Claude tolerates these on resume; so do we — skip the
            // bad line rather than failing the whole load. A dropped record just
            // becomes a continuation boundary, which `lineage` handles.
            let val: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            match val.get("type").and_then(Value::as_str) {
                Some("user") | Some("assistant") => {
                    session.messages.push(record_to_message(val));
                }
                // Everything else is a header record — keep verbatim.
                _ => session.headers.push(val),
            }
        }
        if skipped > 0 {
            eprintln!("sily: skipped {skipped} malformed line(s) while loading {id}");
        }
        Ok(session)
    }

    fn save(&self, session: &Session) -> Result<()> {
        fs::create_dir_all(&self.project_dir)?;
        let path = self.session_path(&session.id);
        let mut out = String::new();

        // Headers first, with sessionId rewritten where present.
        for header in &session.headers {
            let mut h = header.clone();
            if let Some(obj) = h.as_object_mut() {
                rewrite_session_id(obj, &session.id, false);
            }
            out.push_str(&serde_json::to_string(&h)?);
            out.push('\n');
        }

        // Then messages, each patched to this session id (see `convert`).
        for m in &session.messages {
            let record = message_to_record(m, session);
            out.push_str(&serde_json::to_string(&record)?);
            out.push('\n');
        }

        fs::write(&path, out)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionRef>> {
        let mut refs = Vec::new();
        let entries = match fs::read_dir(&self.project_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(refs),
            Err(e) => return Err(Error::Io(e)),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let modified = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            let (summary, message_count) = scan_session(&path);
            refs.push(SessionRef {
                id: id.to_string(),
                summary,
                message_count,
                modified,
                meta: self.meta(),
            });
        }
        Ok(refs)
    }
}

/// One pass over a session file: the first user message (truncated summary) and
/// the total user/assistant message count.
fn scan_session(path: &Path) -> (String, usize) {
    let Ok(text) = fs::read_to_string(path) else {
        return (String::new(), 0);
    };
    let mut summary = String::new();
    let mut count = 0usize;
    for line in text.lines() {
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match val.get("type").and_then(Value::as_str) {
            Some("user") | Some("assistant") => count += 1,
            _ => continue,
        }
        if summary.is_empty() && val.get("type").and_then(Value::as_str) == Some("user") {
            let t = val
                .get("message")
                .and_then(|m| m.get("content"))
                .map(extract_text)
                .unwrap_or_default();
            let t = t.trim();
            if !t.is_empty() {
                summary = t.chars().take(80).collect();
            }
        }
    }
    (summary, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sily_core::model::{Message, Role};
    use sily_core::ops::branch_at;

    /// Two-message fixture in real Claude shape.
    fn fixture(sid: &str) -> String {
        format!(
            "{}\n{}\n{}\n{}\n",
            serde_json::json!({"type":"mode","mode":"normal","sessionId":sid}),
            serde_json::json!({"type":"permission-mode","permissionMode":"default","sessionId":sid}),
            serde_json::json!({
                "parentUuid":null,"isSidechain":false,"type":"user",
                "message":{"role":"user","content":"hi there"},
                "uuid":"u1","timestamp":"2026-01-01T00:00:00.000Z",
                "cwd":"/home/x","sessionId":sid,"version":"2.1.191"
            }),
            serde_json::json!({
                "parentUuid":"u1","isSidechain":false,"type":"assistant",
                "message":{"role":"assistant","model":"claude-opus-4-8",
                           "content":[{"type":"text","text":"hello!"}]},
                "uuid":"a1","timestamp":"2026-01-01T00:00:01.000Z",
                "cwd":"/home/x","sessionId":sid,"version":"2.1.191"
            }),
        )
    }

    fn store_with(dir: &Path, cwd: &str) -> ClaudeStore {
        let store = ClaudeStore::new(dir, cwd);
        fs::create_dir_all(store.project_dir()).unwrap();
        store
    }

    #[test]
    fn load_parses_messages_and_headers() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with(tmp.path(), "/home/x");
        fs::write(store.project_dir().join("s1.jsonl"), fixture("s1")).unwrap();

        let s = store.load("s1").unwrap();
        assert_eq!(s.id, "s1");
        assert_eq!(s.headers.len(), 2);
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.messages[0].text, "hi there");
        assert_eq!(s.messages[1].text, "hello!");
        assert_eq!(s.messages[1].parent_uuid.as_deref(), Some("u1"));
    }

    #[test]
    fn round_trip_preserves_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with(tmp.path(), "/home/x");
        fs::write(store.project_dir().join("s1.jsonl"), fixture("s1")).unwrap();

        let loaded = store.load("s1").unwrap();
        store.save(&loaded).unwrap();
        let again = store.load("s1").unwrap();

        assert_eq!(loaded.headers.len(), again.headers.len());
        assert_eq!(loaded.messages.len(), again.messages.len());
        for (a, b) in loaded.messages.iter().zip(&again.messages) {
            assert_eq!(a.uuid, b.uuid);
            assert_eq!(a.parent_uuid, b.parent_uuid);
            assert_eq!(a.text, b.text);
        }
    }

    #[test]
    fn branch_then_save_rewrites_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with(tmp.path(), "/home/x");
        fs::write(store.project_dir().join("s1.jsonl"), fixture("s1")).unwrap();

        let src = store.load("s1").unwrap();
        let branched = branch_at(&src, "u1", "s2").unwrap();
        store.save(&branched).unwrap();

        let raw = fs::read_to_string(store.project_dir().join("s2.jsonl")).unwrap();
        assert!(!raw.is_empty());
        for line in raw.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            if let Some(sid) = v.get("sessionId").and_then(Value::as_str) {
                assert_eq!(sid, "s2", "stale sessionId in: {line}");
            }
        }
        let reloaded = store.load("s2").unwrap();
        assert_eq!(reloaded.messages.len(), 1);
        assert_eq!(reloaded.messages[0].uuid, "u1");
    }

    #[test]
    fn list_returns_summaries() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with(tmp.path(), "/home/x");
        fs::write(store.project_dir().join("s1.jsonl"), fixture("s1")).unwrap();
        let refs = store.list().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].id, "s1");
        assert_eq!(refs[0].summary, "hi there");
    }

    #[test]
    fn synthesized_message_saves_without_extra() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with(tmp.path(), "/home/x");
        let mut s = Session::new("syn1");
        s.meta.cwd = Some("/home/x".into());
        s.messages
            .push(Message::new("m1", None, Role::User, "from scratch"));
        store.save(&s).unwrap();
        let back = store.load("syn1").unwrap();
        assert_eq!(back.messages.len(), 1);
        assert_eq!(back.messages[0].text, "from scratch");
        assert_eq!(back.messages[0].parent_uuid, None);
    }
}
