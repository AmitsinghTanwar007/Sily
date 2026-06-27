//! Canonical, provider-agnostic conversation model.
//!
//! A [`Session`] is a list of [`Message`]s forming a DAG via `parent_uuid`.
//! Provider-specific fields the core doesn't understand are preserved opaquely
//! in [`Message::extra`] and [`Session::headers`] so adapters can round-trip
//! faithfully. The core never interprets those blobs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who authored a message. `Other` is a catch-all so adapters never lose data
/// on roles the core doesn't model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Other,
}

/// Infallible by design: an unrecognized role string maps to [`Role::Other`]
/// rather than failing, so adapters never drop a message over an unknown role.
/// (Hence `From`, not `TryFrom`.)
impl From<&str> for Role {
    fn from(s: &str) -> Self {
        match s {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            _ => Role::Other,
        }
    }
}

/// One node in the conversation DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Unique id of this message within its provider namespace.
    pub uuid: String,
    /// The message this one follows. `None` marks a root.
    pub parent_uuid: Option<String>,
    pub role: Role,
    /// Best-effort human-readable text (for `log`/`tree`/`diff` display).
    pub text: String,
    /// ISO-8601 timestamp if the provider supplied one.
    pub timestamp: Option<String>,
    /// Opaque original provider record. Core never reads this; adapters use it
    /// to reconstruct the native format on save. Defaults to `null`.
    #[serde(default)]
    pub extra: Value,
}

impl Message {
    /// Minimal constructor for tests/synthesis; `extra` is left null.
    pub fn new(
        uuid: impl Into<String>,
        parent_uuid: Option<String>,
        role: Role,
        text: impl Into<String>,
    ) -> Self {
        Self {
            uuid: uuid.into(),
            parent_uuid,
            role,
            text: text.into(),
            timestamp: None,
            extra: Value::Null,
        }
    }
}

/// Provider-agnostic metadata about where a session lives.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Working directory the session belongs to (drives Claude's project folder).
    pub cwd: Option<String>,
    /// Adapter/provider name, e.g. "claude-code".
    pub provider: Option<String>,
}

/// A whole conversation: header blobs + ordered messages + metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// Opaque provider header records (e.g. Claude's `mode`/`permission-mode`).
    #[serde(default)]
    pub headers: Vec<Value>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub meta: SessionMeta,
}

impl Session {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            headers: Vec::new(),
            messages: Vec::new(),
            meta: SessionMeta::default(),
        }
    }

    /// Look up a message by uuid.
    pub fn message(&self, uuid: &str) -> Option<&Message> {
        self.messages.iter().find(|m| m.uuid == uuid)
    }

    /// The root message (first one with no parent), if any.
    pub fn root(&self) -> Option<&Message> {
        self.messages.iter().find(|m| m.parent_uuid.is_none())
    }

    /// Direct children of the given message uuid.
    pub fn children(&self, uuid: &str) -> Vec<&Message> {
        self.messages
            .iter()
            .filter(|m| m.parent_uuid.as_deref() == Some(uuid))
            .collect()
    }

    /// Leaf messages (no children) — the tips of every branch in this session.
    pub fn leaves(&self) -> Vec<&Message> {
        self.messages
            .iter()
            .filter(|m| self.children(&m.uuid).is_empty())
            .collect()
    }
}

/// A named, persisted pointer into the DAG — sily's "commit".
///
/// Stored in `~/.sily/`. It records *where* a good point is, not a copy of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    /// Human-friendly name/ref.
    pub name: String,
    /// Session the commit points into.
    pub session_id: String,
    /// The exact message that is "HEAD" at commit time.
    pub message_uuid: String,
    /// When the commit was made (ISO-8601; stamped by the caller, not core).
    pub created_at: String,
    pub note: Option<String>,
}

/// Provenance for a session created by `branch`/`revert`: records which session
/// it came from and where, so the graph can show branch relationships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchRecord {
    /// The newly created session.
    pub session_id: String,
    /// The session it was branched from.
    pub from_session: String,
    /// The message it was branched at.
    pub at_message: String,
    /// How it was created: a commit name, or "branch".
    pub origin: String,
    pub created_at: String,
}
