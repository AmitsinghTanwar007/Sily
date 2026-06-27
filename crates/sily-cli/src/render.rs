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

/// Branch tree. Linear stretches stay flat (no growing indent); indentation only
/// increases at real forks. Each fragment (a session start or a compaction
/// boundary) begins with `●` and a blank-line separator.
pub fn tree(session: &Session) -> String {
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

    if roots.is_empty() {
        return "(empty session)\n".to_string();
    }

    let mut out = String::new();
    out.push_str("legend: ● start   │ reply   ┳ fork   ○ leaf   [sub] sub-agent\n\n");
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            out.push('\n'); // separate fragments
        }
        render_node(session, root, 0, true, &mut out);
    }
    out
}

fn render_node(session: &Session, msg: &Message, depth: usize, is_root: bool, out: &mut String) {
    let children = session.children(&msg.uuid);
    let marker = if is_root {
        "●" // fragment start
    } else if children.len() > 1 {
        "┳" // fork
    } else if children.is_empty() {
        "○" // leaf
    } else {
        "│" // linear reply
    };
    let tag = if is_sidechain(msg) { " [sub]" } else { "" };
    out.push_str(&format!(
        "{}{} {}  {}{}  {}\n",
        "  ".repeat(depth),
        marker,
        short(&msg.uuid),
        role_tag(&msg.role),
        tag,
        snippet(&msg.text, 60)
    ));
    match children.as_slice() {
        [] => {}
        // Linear: keep the same indent so a straight chain reads as a flat list.
        [only] => render_node(session, only, depth, false, out),
        // Real fork: indent each branch.
        many => {
            for child in many {
                render_node(session, child, depth + 1, false, out);
            }
        }
    }
}

/// Claude marks sub-agent (Task) messages with `isSidechain: true` in the record.
fn is_sidechain(msg: &Message) -> bool {
    msg.extra
        .get("isSidechain")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
