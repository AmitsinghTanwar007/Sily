//! Translation between Claude `.jsonl` records and the canonical model.
//!
//! These helpers are the only place that understands Claude's record shape
//! (`message.content` blocks, role strings, `sessionId`/`parentUuid` fields).
//! `store` uses them; nothing here touches the filesystem.

use serde_json::{Map, Value};

use sily_core::model::{Message, Role, Session};

pub const PROVIDER: &str = "claude-code";

/// Pull best-effort display text out of a Claude `message.content` value, which
/// is either a plain string or an array of typed blocks.
pub(crate) fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The role string Claude writes for a canonical [`Role`]. `Other` has no native
/// Claude representation, so it serializes as `user` (the safe default).
fn claude_role_str(role: Role) -> &'static str {
    match role {
        Role::User | Role::Other => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

/// Set `sessionId` on an object record if the key is present (or if `force`).
pub(crate) fn rewrite_session_id(obj: &mut Map<String, Value>, new_id: &str, force: bool) {
    if force || obj.contains_key("sessionId") {
        obj.insert("sessionId".into(), Value::String(new_id.to_string()));
    }
}

/// Turn a raw `user`/`assistant` record into a canonical [`Message`], keeping the
/// original JSON in `extra` for faithful round-trip.
pub(crate) fn record_to_message(val: Value) -> Message {
    let msg = val.get("message");
    let role = msg
        .and_then(|m| m.get("role"))
        .and_then(Value::as_str)
        .map(Role::from)
        .unwrap_or(Role::Other);
    let text = msg
        .and_then(|m| m.get("content"))
        .map(extract_text)
        .unwrap_or_default();
    let uuid = val
        .get("uuid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parent_uuid = val
        .get("parentUuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    let timestamp = val
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    Message {
        uuid,
        parent_uuid,
        role,
        text,
        timestamp,
        extra: val,
    }
}

/// Serialize a canonical [`Message`] to a Claude record for the given session.
/// Prefers the faithful original (`extra`) patched to the session id; falls back
/// to a minimal synthesized record when the message has no provider payload.
pub(crate) fn message_to_record(m: &Message, session: &Session) -> Value {
    match &m.extra {
        Value::Object(_) => {
            let mut v = m.extra.clone();
            if let Some(obj) = v.as_object_mut() {
                rewrite_session_id(obj, &session.id, true);
            }
            v
        }
        _ => synth_record(m, session),
    }
}

/// Build a minimal valid Claude record for a message that has no `extra`
/// (e.g. a fully synthesized session). Mirrors the fields Claude writes.
fn synth_record(m: &Message, session: &Session) -> Value {
    let role = claude_role_str(m.role);
    let mut obj = Map::new();
    obj.insert(
        "parentUuid".into(),
        m.parent_uuid
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    obj.insert("isSidechain".into(), Value::Bool(false));
    obj.insert("type".into(), Value::String(role.into()));
    obj.insert(
        "message".into(),
        serde_json::json!({ "role": role, "content": m.text }),
    );
    obj.insert("uuid".into(), Value::String(m.uuid.clone()));
    if let Some(ts) = &m.timestamp {
        obj.insert("timestamp".into(), Value::String(ts.clone()));
    }
    obj.insert("userType".into(), Value::String("external".into()));
    obj.insert("entrypoint".into(), Value::String("cli".into()));
    if let Some(cwd) = &session.meta.cwd {
        obj.insert("cwd".into(), Value::String(cwd.clone()));
    }
    obj.insert("sessionId".into(), Value::String(session.id.clone()));
    Value::Object(obj)
}
