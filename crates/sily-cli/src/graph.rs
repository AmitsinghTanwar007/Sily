//! Colored, git-log-style rendering of sessions, commits, and branches.
//!
//! `render_list` shows one project's sessions. `render_all` shows the whole tree:
//! provider → project → sessions, with commits (`◆`) and branched sessions (`○`)
//! nested under each. Colors degrade gracefully — `owo-colors` strips them when
//! output isn't a TTY or `NO_COLOR` is set.

use std::collections::HashMap;
use std::time::SystemTime;

use owo_colors::{OwoColorize, Stream::Stdout, Style};

use sily_core::model::{BranchRecord, Commit};
use sily_core::store::{ProjectSessions, SessionRef};

/// Render one project's session/branch graph (`sily list`).
pub fn render_list(sessions: &[SessionRef], commits: &[Commit], branches: &[BranchRecord]) -> String {
    if sessions.is_empty() {
        return "(no sessions in this project)\n".to_string();
    }
    let ctx = Ctx::build(sessions, commits, branches);
    let mut out = String::new();
    for root in ctx.roots_sorted() {
        render_session(&ctx, &root.id, "", &mut out);
        out.push('\n');
    }
    out
}

/// Render the full tree across all providers and projects (`sily list`).
pub fn render_all(
    providers: &[(String, Vec<ProjectSessions>)],
    commits: &[Commit],
    branches: &[BranchRecord],
) -> String {
    if providers.is_empty() {
        return "(no sessions found)\n".to_string();
    }
    let mut out = String::new();
    for (name, projects) in providers {
        out.push_str(&format!(
            "{} {}\n",
            "●".if_supports_color(Stdout, |t| t.magenta()),
            name.if_supports_color(Stdout, |t| t.style(Style::new().magenta().bold())),
        ));
        render_provider(projects, commits, branches, &mut out);
        out.push('\n');
    }
    out
}

fn render_provider(
    projects: &[ProjectSessions],
    commits: &[Commit],
    branches: &[BranchRecord],
    out: &mut String,
) {
    // newest-active project first
    let mut order: Vec<usize> = (0..projects.len()).collect();
    order.sort_by(|&a, &b| latest(&projects[b].sessions).cmp(&latest(&projects[a].sessions)));

    let n = order.len();
    for (k, &pi) in order.iter().enumerate() {
        let proj = &projects[pi];
        let last = k == n - 1;
        let connector = if last { "└─ " } else { "├─ " };
        let child_prefix = if last { "   " } else { "│  " };
        out.push_str(&format!(
            "{}{}  {}\n",
            connector.if_supports_color(Stdout, |t| t.dimmed()),
            proj.cwd.if_supports_color(Stdout, |t| t.style(Style::new().blue().bold())),
            format!("{} sessions", proj.sessions.len()).if_supports_color(Stdout, |t| t.dimmed()),
        ));
        let ctx = Ctx::build(&proj.sessions, commits, branches);
        for root in ctx.roots_sorted() {
            render_session(&ctx, &root.id, child_prefix, out);
        }
        if !last {
            out.push_str(&"│\n".if_supports_color(Stdout, |t| t.dimmed()).to_string());
        }
    }
}

fn latest(sessions: &[SessionRef]) -> Option<SystemTime> {
    sessions.iter().filter_map(|s| s.modified).max()
}

struct Ctx<'a> {
    by_id: HashMap<&'a str, &'a SessionRef>,
    commits_of: HashMap<&'a str, Vec<&'a Commit>>,
    children_of: HashMap<&'a str, Vec<&'a BranchRecord>>,
    is_child: HashMap<&'a str, bool>,
    sessions: &'a [SessionRef],
}

impl<'a> Ctx<'a> {
    fn build(sessions: &'a [SessionRef], commits: &'a [Commit], branches: &'a [BranchRecord]) -> Self {
        let by_id: HashMap<&str, &SessionRef> =
            sessions.iter().map(|s| (s.id.as_str(), s)).collect();

        let mut commits_of: HashMap<&str, Vec<&Commit>> = HashMap::new();
        for c in commits {
            if by_id.contains_key(c.session_id.as_str()) {
                commits_of.entry(&c.session_id).or_default().push(c);
            }
        }

        let mut children_of: HashMap<&str, Vec<&BranchRecord>> = HashMap::new();
        let mut is_child: HashMap<&str, bool> = HashMap::new();
        for b in branches {
            if by_id.contains_key(b.session_id.as_str())
                && by_id.contains_key(b.from_session.as_str())
            {
                children_of.entry(&b.from_session).or_default().push(b);
                is_child.insert(&b.session_id, true);
            }
        }

        Ctx { by_id, commits_of, children_of, is_child, sessions }
    }

    /// Top-level sessions (not a known branch-child), newest first.
    fn roots_sorted(&self) -> Vec<&'a SessionRef> {
        let mut roots: Vec<&SessionRef> = self
            .sessions
            .iter()
            .filter(|s| !self.is_child.get(s.id.as_str()).copied().unwrap_or(false))
            .collect();
        roots.sort_by(|a, b| b.modified.cmp(&a.modified));
        roots
    }
}

fn render_session(ctx: &Ctx, id: &str, prefix: &str, out: &mut String) {
    if let Some(s) = ctx.by_id.get(id) {
        out.push_str(prefix);
        out.push_str(&session_line(s));
    }
    print_children(ctx, id, prefix, out);
}

fn print_children(ctx: &Ctx, session_id: &str, prefix: &str, out: &mut String) {
    let commits = ctx.commits_of.get(session_id);
    let children = ctx.children_of.get(session_id);
    let total = commits.map_or(0, |v| v.len()) + children.map_or(0, |v| v.len());
    let mut i = 0;

    if let Some(commits) = commits {
        for c in commits {
            i += 1;
            let connector = if i == total { "└─" } else { "├─" };
            out.push_str(&format!(
                "{}{} {}\n",
                prefix.if_supports_color(Stdout, |t| t.dimmed()),
                connector.if_supports_color(Stdout, |t| t.dimmed()),
                commit_label(c),
            ));
        }
    }

    if let Some(children) = children {
        for b in children {
            i += 1;
            let last = i == total;
            let connector = if last { "└─" } else { "├─" };
            out.push_str(&format!(
                "{}{} {}\n",
                prefix.if_supports_color(Stdout, |t| t.dimmed()),
                connector.if_supports_color(Stdout, |t| t.dimmed()),
                branch_label(ctx.by_id.get(b.session_id.as_str()), b),
            ));
            let deeper = format!("{prefix}{}", if last { "   " } else { "│  " });
            print_children(ctx, &b.session_id, &deeper, out);
        }
    }
}

fn session_line(s: &SessionRef) -> String {
    format!(
        "{} {}  {}   {}\n",
        "●".if_supports_color(Stdout, |t| t.bright_yellow()),
        short(&s.id).if_supports_color(Stdout, |t| t.style(Style::new().bright_yellow().bold())),
        truncate(&s.summary, 50),
        meta(s.message_count, s.modified).if_supports_color(Stdout, |t| t.dimmed()),
    )
}

fn commit_label(c: &Commit) -> String {
    let note = c.note.as_deref().unwrap_or("");
    format!(
        "{} {}  {}",
        "◆".if_supports_color(Stdout, |t| t.green()),
        c.name.if_supports_color(Stdout, |t| t.style(Style::new().green().bold())),
        format!("\"{note}\"").if_supports_color(Stdout, |t| t.dimmed()),
    )
}

fn branch_label(child: Option<&&SessionRef>, b: &BranchRecord) -> String {
    let detail = match child {
        Some(s) => meta(s.message_count, s.modified),
        None => "missing".to_string(),
    };
    format!(
        "{} {}  {} · {}",
        "○".if_supports_color(Stdout, |t| t.cyan()),
        short(&b.session_id).if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
        b.origin.if_supports_color(Stdout, |t| t.cyan()),
        detail.if_supports_color(Stdout, |t| t.dimmed()),
    )
}

fn meta(count: usize, modified: Option<SystemTime>) -> String {
    let when = rel_time(modified);
    if when.is_empty() {
        format!("{count} msgs")
    } else {
        format!("{count} msgs · {when}")
    }
}

fn rel_time(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let Ok(d) = SystemTime::now().duration_since(t) else {
        return "just now".to_string();
    };
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn truncate(text: &str, width: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= width {
        one_line
    } else {
        let mut s: String = one_line.chars().take(width.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}
