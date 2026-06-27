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

/// A real, human-typed prompt — not a tool result, command output, or injected
/// context (those are empty here or wrapped in `<…>` tags).
pub fn is_real_prompt(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("[Request interrupted")
}

/// Just the user's prompts, in order — skips assistant turns, tool calls,
/// sidechains, and command/caveat noise. `limit` shows only the last N.
pub fn prompts(session: &Session, limit: Option<usize>) -> String {
    let all: Vec<&Message> = session
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User) && is_real_prompt(&m.text))
        .collect();
    if all.is_empty() {
        return "(no user prompts)\n".to_string();
    }
    let start = match limit {
        Some(n) if all.len() > n => all.len() - n,
        _ => 0,
    };
    let mut out = String::new();
    if start > 0 {
        out.push_str(&format!("… {start} earlier prompts (use --full to see all)\n"));
    }
    for (i, m) in all[start..].iter().enumerate() {
        out.push_str(&format!("{:>3}. {}\n", start + i + 1, snippet(&m.text, 100)));
    }
    out
}

/// Linear history in append-log (file) order — how Claude itself reads the
/// session. `limit` shows only the last N messages (None = all).
pub fn log(session: &Session, limit: Option<usize>) -> String {
    let msgs = &session.messages;
    if msgs.is_empty() {
        return "(empty session)\n".to_string();
    }
    let start = match limit {
        Some(n) if msgs.len() > n => msgs.len() - n,
        _ => 0,
    };
    let mut out = String::new();
    if start > 0 {
        out.push_str(&format!("… {start} earlier messages (use --full to see all)\n"));
    }
    for m in &msgs[start..] {
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
pub fn tree(session: &Session, limit: Option<usize>) -> String {
    // Optionally restrict to the last N messages; parents outside the window
    // become roots, so you see recent threads as fresh fragments.
    let restricted;
    let (s, skipped) = match limit {
        Some(n) if session.messages.len() > n => {
            let cut = session.messages.len() - n;
            restricted = Session {
                id: session.id.clone(),
                headers: Vec::new(),
                messages: session.messages[cut..].to_vec(),
                meta: session.meta.clone(),
            };
            (&restricted, cut)
        }
        _ => (session, 0),
    };

    let roots: Vec<&Message> = s
        .messages
        .iter()
        .filter(|m| {
            m.parent_uuid
                .as_ref()
                .map(|p| s.message(p).is_none())
                .unwrap_or(true)
        })
        .collect();

    if roots.is_empty() {
        return "(empty session)\n".to_string();
    }

    let mut out = String::new();
    out.push_str("legend: ● start   │ reply   ┳ fork   ○ leaf   [sub] sub-agent\n");
    if skipped > 0 {
        out.push_str(&format!("… {skipped} earlier messages (use --full to see all)\n"));
    }
    out.push('\n');
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            out.push('\n'); // separate fragments
        }
        render_node(s, root, 0, true, &mut out);
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
