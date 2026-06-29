//! Colored, git-log-style rendering of sessions, commits, and branches.
//!
//! `render_list` shows one project's sessions. `render_all` shows the whole tree:
//! provider → project → sessions, with commits (`◆`) and branched sessions (`○`)
//! nested under each. Colors degrade gracefully — `owo-colors` strips them when
//! output isn't a TTY or `NO_COLOR` is set.

use std::collections::HashMap;
use std::time::SystemTime;

use owo_colors::{OwoColorize, Stream::Stdout, Style};

use sily_core::model::{BranchRecord, Commit, Role};
use sily_core::provider::MsgPoint;
use sily_core::store::{ProjectSessions, SessionRef};

use crate::idfmt::{compact_label, unique_labels};

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

fn role_lbl(r: Role) -> &'static str {
    match r {
        Role::User => "you",
        Role::Assistant => "ai ",
        Role::System => "sys",
        Role::Other => "·  ",
    }
}

/// A real, human turn — not command/tool/system plumbing or empty filler.
fn meaningful(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && !t.starts_with('<')
        && !t.starts_with("[Request interrupted")
        && t != "No response requested."
        && t != "No response requested"
}

/// A branch to draw as a parallel lane: its id, how it was made, the main point
/// it forked at, and its (raw) divergent messages.
pub struct GraphBranch {
    pub id: String,
    pub origin: String,
    /// The main message point this branch forked from (last shared message).
    pub fork_point: String,
    pub tail: Vec<MsgPoint>,
}

/// A node placed on the lane graph.
#[derive(Clone)]
struct GRow {
    time: String,
    lane: usize,
    role: Role,
    text: String,
    point: String,
    is_main: bool,
}

/// One rail-column glyph: a node on the trunk, a node on a branch, or a dim
/// structural character (`│ ╯ ─ ┼` or space).
pub enum RailCell {
    MainNode,
    BranchNode,
    Dim(char),
}

/// One laid-out graph row: its rail prefix plus the labels to its right.
pub struct RowOut {
    pub cells: Vec<RailCell>,
    pub is_main: bool,
    pub role: Role,
    pub text: String,
    pub commits: Vec<String>,
    pub stubs: Vec<String>, // just-created branches forking at this node
    /// Set on a branch lane's tip node: the branch id (so you know which lane is which).
    pub branch_tip: Option<String>,
}

/// A fully laid-out graph, ready to render to ANSI (`sily graph`) or ratatui
/// (the interactive detail pane).
pub struct GraphLayout {
    pub id: String,
    pub rows: Vec<RowOut>,
    pub bottom_empties: Vec<(String, String)>, // (id, origin) whose fork is off-screen
    pub earlier: usize,
    pub empty_note: Option<String>,
}

/// True multi-lane rail layout (git/gitgraph.nvim style): the main timeline is
/// lane 0; each branch runs in its own parallel lane, opening at the message it
/// forked from. Rows are time-ordered (newest first) so lanes interleave. This
/// is shared by `session_graph` (ANSI) and the TUI detail pane.
pub fn session_layout(
    id: &str,
    main_raw: &[MsgPoint],
    commits: &[Commit],
    branches: &[GraphBranch],
    limit: Option<usize>,
) -> GraphLayout {
    let main: Vec<&MsgPoint> = main_raw.iter().filter(|m| meaningful(&m.text)).collect();
    if main.is_empty() {
        return GraphLayout {
            id: short(id).to_string(),
            rows: Vec::new(),
            bottom_empties: Vec::new(),
            earlier: 0,
            empty_note: Some("(no conversation — only system/command messages)".to_string()),
        };
    }

    // commits anchored to a meaningful main message
    let first_meaningful = main_raw.iter().find(|m| meaningful(&m.text)).map(|m| m.point.clone());
    let mut anchor: HashMap<String, String> = HashMap::new();
    let mut last: Option<String> = None;
    for m in main_raw {
        if meaningful(&m.text) {
            last = Some(m.point.clone());
        }
        anchor.insert(
            m.point.clone(),
            last.clone().or_else(|| first_meaningful.clone()).unwrap_or_else(|| m.point.clone()),
        );
    }
    let last_raw = main_raw.last().map(|m| m.point.clone()).unwrap_or_default();
    let anch = |raw: &str| anchor.get(raw).cloned().unwrap_or_else(|| raw.to_string());
    let mut commit_at: HashMap<String, Vec<&Commit>> = HashMap::new();
    for c in commits {
        let raw = if c.message_uuid.is_empty() { last_raw.clone() } else { c.message_uuid.clone() };
        commit_at.entry(anch(&raw)).or_default().push(c);
    }

    // rows = main (lane 0) + each branch's divergent messages (lane i+1)
    let mut rows: Vec<GRow> = main
        .iter()
        .map(|m| GRow { time: m.time.clone(), lane: 0, role: m.role, text: m.text.clone(), point: m.point.clone(), is_main: true })
        .collect();
    let mut empty_branches: Vec<&GraphBranch> = Vec::new();
    let mut fork_pts: Vec<String> = vec![String::new()]; // lane-indexed fork point
    let mut lane_ids: Vec<String> = vec![String::new()]; // lane-indexed branch id
    for b in branches {
        let tail: Vec<&MsgPoint> = b.tail.iter().filter(|m| meaningful(&m.text)).collect();
        if tail.is_empty() {
            empty_branches.push(b);
            continue;
        }
        let lane = fork_pts.len();
        fork_pts.push(anch(&b.fork_point));
        lane_ids.push(b.id.clone());
        for m in &tail {
            rows.push(GRow { time: m.time.clone(), lane, role: m.role, text: m.text.clone(), point: String::new(), is_main: false });
        }
    }
    rows.sort_by(|a, b| b.time.cmp(&a.time)); // newest first (rows[0] = newest)
    let total = rows.len();
    let maxlane = rows.iter().map(|r| r.lane).max().unwrap_or(0);
    let all_main: Vec<usize> = (0..total).filter(|&i| rows[i].lane == 0).collect();

    let mut head = vec![usize::MAX; maxlane + 1];
    let mut fork = vec![usize::MAX; maxlane + 1];
    let mut deepest = 0usize;
    for l in 1..=maxlane {
        let idxs: Vec<usize> = (0..total).filter(|&i| rows[i].lane == l).collect();
        if idxs.is_empty() {
            continue;
        }
        head[l] = *idxs.iter().min().unwrap();
        // fork = the main row at this branch's fork point (last shared message);
        // fall back to the newest main not newer than the branch's oldest msg.
        let fp = fork_pts.get(l).cloned().unwrap_or_default();
        fork[l] = all_main
            .iter()
            .copied()
            .find(|&mi| rows[mi].point == fp)
            .or_else(|| {
                let oldest = rows[*idxs.iter().max().unwrap()].time.clone();
                all_main.iter().copied().find(|&mi| rows[mi].time <= oldest)
            })
            .unwrap_or_else(|| all_main.last().copied().unwrap_or(0));
        deepest = deepest.max(fork[l]);
    }
    // Show the newest `limit` rows, but always extend down to the deepest fork so
    // branches (and the point they split from) stay visible.
    let end = match limit {
        Some(n) => n.max(deepest + 1).min(total),
        None => total,
    };
    let shown = &rows[0..end];
    let main_idxs: Vec<usize> = all_main.iter().copied().filter(|&i| i < end).collect();
    let (m_first, m_last) = (main_idxs.first().copied().unwrap_or(0), main_idxs.last().copied().unwrap_or(0));

    // Branches with no new conversation: show a stub `╰○` on their fork message
    // so you can see the branch exists, even without messages of its own.
    let mut empty_at: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for b in &empty_branches {
        empty_at.entry(anch(&b.fork_point)).or_default().push(b.id.clone());
    }
    let shown_points: std::collections::HashSet<&str> =
        shown.iter().filter(|r| r.is_main).map(|r| r.point.as_str()).collect();

    // tip row index → branch id, so each lane gets labelled at its newest node
    let mut tip_branch: HashMap<usize, String> = HashMap::new();
    for (l, &h) in head.iter().enumerate().skip(1) {
        if h != usize::MAX {
            tip_branch.insert(h, lane_ids.get(l).cloned().unwrap_or_default());
        }
    }

    let mut out_rows: Vec<RowOut> = Vec::new();
    for (r, row) in shown.iter().enumerate() {
        let mut glyph = vec![' '; maxlane + 1];
        let mut sep = vec![' '; maxlane + 1];
        for (c, g) in glyph.iter_mut().enumerate() {
            if row.lane == c {
                *g = '●';
            } else if c == 0 {
                if r >= m_first && r <= m_last {
                    *g = '│';
                }
            } else if head[c] != usize::MAX && r >= head[c] && r <= fork[c] {
                *g = '│';
            }
        }
        // fork connectors: a branch lane closes into the trunk on its fork row
        for l in 1..=maxlane {
            if head[l] != usize::MAX && fork[l] == r {
                glyph[l] = '╯';
                for g in glyph.iter_mut().take(l).skip(1) {
                    *g = if *g == '│' { '┼' } else { '─' };
                }
                for s in sep.iter_mut().take(l) {
                    *s = '─';
                }
            }
        }
        let mut cells: Vec<RailCell> = Vec::new();
        for c in 0..=maxlane {
            let g = glyph[c];
            cells.push(if g == '●' && row.lane == c {
                if c == 0 { RailCell::MainNode } else { RailCell::BranchNode }
            } else {
                RailCell::Dim(g)
            });
            cells.push(RailCell::Dim(sep[c]));
        }
        let commits_here: Vec<String> = if row.is_main {
            commit_at
                .get(row.point.as_str())
                .into_iter()
                .flatten()
                .map(|c| match c.note.as_deref() {
                    Some(n) if !n.is_empty() => format!("{}  \"{}\"", c.name, n),
                    _ => c.name.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let stubs_here: Vec<String> = if row.is_main {
            empty_at.get(row.point.as_str()).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        out_rows.push(RowOut {
            cells,
            is_main: row.is_main,
            role: row.role,
            text: row.text.clone(),
            commits: commits_here,
            stubs: stubs_here,
            branch_tip: tip_branch.get(&r).cloned(),
        });
    }

    let bottom_empties: Vec<(String, String)> = empty_branches
        .iter()
        .filter(|b| !shown_points.contains(anch(&b.fork_point).as_str()))
        .map(|b| (b.id.clone(), b.origin.clone()))
        .collect();

    GraphLayout { id: short(id).to_string(), rows: out_rows, bottom_empties, earlier: total - end, empty_note: None }
}

/// Render the lane layout to a colored string (`sily graph`).
pub fn session_graph(
    id: &str,
    main_raw: &[MsgPoint],
    commits: &[Commit],
    branches: &[GraphBranch],
    limit: Option<usize>,
) -> String {
    let lay = session_layout(id, main_raw, commits, branches, limit);
    let dim = |s: &str| s.if_supports_color(Stdout, |t| t.dimmed()).to_string();
    let mut out = format!(
        "{} {}\n",
        "▲".if_supports_color(Stdout, |t| t.magenta()),
        format!("{} (newest first)", lay.id).if_supports_color(Stdout, |t| t.style(Style::new().magenta().bold())),
    );
    if let Some(note) = &lay.empty_note {
        out.push_str(&format!("{note}\n"));
        return out;
    }
    for row in &lay.rows {
        let mut rail = String::new();
        for cell in &row.cells {
            match cell {
                RailCell::MainNode => rail.push_str(&"●".if_supports_color(Stdout, |t| t.bright_yellow()).to_string()),
                RailCell::BranchNode => rail.push_str(&"●".if_supports_color(Stdout, |t| t.cyan()).to_string()),
                RailCell::Dim(c) => rail.push_str(&dim(&c.to_string())),
            }
        }
        let role = if row.is_main {
            role_lbl(row.role).if_supports_color(Stdout, |t| t.dimmed()).to_string()
        } else {
            role_lbl(row.role).if_supports_color(Stdout, |t| t.cyan()).to_string()
        };
        let mut lbl = format!("{role}  {}", truncate(&row.text, if row.is_main { 48 } else { 40 }));
        if let Some(bid) = &row.branch_tip {
            lbl.push_str(&format!(
                "  {}",
                format!("← branch {bid}").if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
            ));
        }
        for c in &row.commits {
            lbl.push_str(&format!(
                "  {} {}",
                "◆".if_supports_color(Stdout, |t| t.green()),
                c.if_supports_color(Stdout, |t| t.style(Style::new().green().bold())),
            ));
        }
        for eid in &row.stubs {
            lbl.push_str(&format!(
                "  {}{}",
                "╰○ ".if_supports_color(Stdout, |t| t.cyan()),
                eid.if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
            ));
        }
        out.push_str(&format!("{rail} {lbl}\n"));
    }
    for (eid, origin) in &lay.bottom_empties {
        out.push_str(&format!(
            "{} {}  {}\n",
            "○".if_supports_color(Stdout, |t| t.cyan()),
            eid.if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
            format!("{origin} · no new conversation").if_supports_color(Stdout, |t| t.dimmed()),
        ));
    }
    if lay.earlier > 0 {
        out.push_str(&format!(
            "{}\n",
            format!("┆ … {} earlier (use --full)", lay.earlier).if_supports_color(Stdout, |t| t.dimmed())
        ));
    }
    out
}

fn latest(sessions: &[SessionRef]) -> Option<SystemTime> {
    sessions.iter().filter_map(|s| s.modified).max()
}

struct Ctx<'a> {
    by_id: HashMap<&'a str, &'a SessionRef>,
    label_by_id: HashMap<String, String>,
    commits_of: HashMap<&'a str, Vec<&'a Commit>>,
    children_of: HashMap<&'a str, Vec<&'a BranchRecord>>,
    is_child: HashMap<&'a str, bool>,
    sessions: &'a [SessionRef],
}

impl<'a> Ctx<'a> {
    fn build(sessions: &'a [SessionRef], commits: &'a [Commit], branches: &'a [BranchRecord]) -> Self {
        let by_id: HashMap<&str, &SessionRef> =
            sessions.iter().map(|s| (s.id.as_str(), s)).collect();
        let label_by_id = unique_labels(sessions.iter().map(|s| s.id.as_str()), 8);

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

        Ctx { by_id, label_by_id, commits_of, children_of, is_child, sessions }
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

    fn label(&self, id: &str) -> String {
        self.label_by_id
            .get(id)
            .cloned()
            .unwrap_or_else(|| compact_label(id, 8))
    }
}

fn render_session(ctx: &Ctx, id: &str, prefix: &str, out: &mut String) {
    if let Some(s) = ctx.by_id.get(id) {
        out.push_str(prefix);
        out.push_str(&session_line(s, &ctx.label(&s.id)));
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
                branch_label(ctx.by_id.get(b.session_id.as_str()), b, &ctx.label(&b.session_id)),
            ));
            let deeper = format!("{prefix}{}", if last { "   " } else { "│  " });
            print_children(ctx, &b.session_id, &deeper, out);
        }
    }
}

fn session_line(s: &SessionRef, label: &str) -> String {
    format!(
        "{} {}  {}   {}\n",
        "●".if_supports_color(Stdout, |t| t.bright_yellow()),
        label.if_supports_color(Stdout, |t| t.style(Style::new().bright_yellow().bold())),
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

fn branch_label(child: Option<&&SessionRef>, b: &BranchRecord, label: &str) -> String {
    let detail = match child {
        Some(s) => meta(s.message_count, s.modified),
        None => "missing".to_string(),
    };
    // Show where it forked from so the origin point is visible.
    let from = if b.at_message.is_empty() {
        "HEAD".to_string()
    } else {
        short(&b.at_message).to_string()
    };
    format!(
        "{} {}  {} {} · {}",
        "○".if_supports_color(Stdout, |t| t.cyan()),
        label.if_supports_color(Stdout, |t| t.style(Style::new().cyan().bold())),
        b.origin.if_supports_color(Stdout, |t| t.cyan()),
        format!("(from {from})").if_supports_color(Stdout, |t| t.dimmed()),
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

fn short(id: &str) -> String {
    compact_label(id, 8)
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
