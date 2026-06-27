//! Human-readable rendering of sessions for `log` and `tree`.

use sily_core::model::{Message, Role, Session};

fn role_tag(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "asst",
        Role::System => "sys ",
        Role::Other => "????",
    }
}

fn snippet(text: &str, width: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= width {
        one_line
    } else {
        let mut s: String = one_line.chars().take(width.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

fn short(uuid: &str) -> &str {
    &uuid[..uuid.len().min(8)]
}

/// Linear history in append-log (file) order — how Claude itself reads the
/// session. This is robust to the phantom `parentUuid` links real files carry.
pub fn log(session: &Session) -> String {
    if session.messages.is_empty() {
        return "(empty session)\n".to_string();
    }
    let mut out = String::new();
    for m in &session.messages {
        out.push_str(&format!(
            "{}  {}  {}\n",
            short(&m.uuid),
            role_tag(&m.role),
            snippet(&m.text, 80)
        ));
    }
    out
}

/// Branch tree: every message, indented under its parent, with branch points
/// (more than one child) and leaves marked.
pub fn tree(session: &Session) -> String {
    let mut out = String::new();
    let roots: Vec<&Message> = session
        .messages
        .iter()
        .filter(|m| {
            m.parent_uuid
                .as_ref()
                .map(|p| session.message(p).is_none())
                .unwrap_or(true)
        })
        .collect();
    for root in roots {
        render_node(session, root, 0, &mut out);
    }
    if out.is_empty() {
        out.push_str("(empty session)\n");
    }
    out
}

fn render_node(session: &Session, msg: &Message, depth: usize, out: &mut String) {
    let children = session.children(&msg.uuid);
    let marker = if children.len() > 1 {
        "┳" // branch point
    } else if children.is_empty() {
        "○" // leaf
    } else {
        "│"
    };
    out.push_str(&format!(
        "{}{} {}  {}  {}\n",
        "  ".repeat(depth),
        marker,
        short(&msg.uuid),
        role_tag(&msg.role),
        snippet(&msg.text, 60)
    ));
    for child in children {
        render_node(session, child, depth + 1, out);
    }
}
