//! Interactive, collapsible session browser for `sily list` (when run in a
//! terminal). Shows a tree: claude-code → directories (nested by path) →
//! sessions → commits/branches. Everything starts collapsed; nodes expand on
//! demand. Browse with the arrows; copy a session's resume command with `y`.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::SystemTime;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use sily_core::model::{BranchRecord, Commit, Role};
use sily_core::provider::{MsgPoint, Provider};
use sily_core::store::{ProjectSessions, SessionRef};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Adapter,
    Dir,
    Session,
    Commit,
    Branch,
}

struct Node {
    kind: Kind,
    primary: String,
    secondary: String,
    meta: String,
    /// The full resume command for this node, if it's a resumable session.
    resume: Option<String>,
    /// Full session id (for Session nodes) — used to render the detail rail.
    session_id: Option<String>,
    children: Vec<usize>,
}

/// The provider-specific command to resume a session.
fn resume_command(provider: &str, id: &str) -> String {
    match provider {
        "codex-cli" => format!("codex resume {id}"),
        "opencode" => format!("opencode --session {id}"),
        _ => format!("claude --resume {id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::resume_command;

    #[test]
    fn resume_command_is_provider_specific() {
        assert_eq!(resume_command("claude-code", "abc"), "claude --resume abc");
        assert_eq!(resume_command("codex-cli", "abc"), "codex resume abc");
        assert_eq!(resume_command("opencode", "ses_abc"), "opencode --session ses_abc");
    }
}

struct Tree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

impl Tree {
    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }
}

// ---------------------------------------------------------------- tree build

#[derive(Default)]
struct Trie {
    sessions: Vec<SessionRef>,
    children: BTreeMap<String, Trie>,
}

fn trie_insert(t: &mut Trie, comps: &[&str], sessions: Vec<SessionRef>) {
    match comps.first() {
        None => t.sessions = sessions,
        Some(head) => trie_insert(t.children.entry((*head).to_string()).or_default(), &comps[1..], sessions),
    }
}

fn build_tree(
    providers: &[(String, Vec<ProjectSessions>)],
    commits: &[Commit],
    branches: &[BranchRecord],
) -> Tree {
    let mut tree = Tree { nodes: Vec::new(), roots: Vec::new() };
    for (name, projects) in providers {
        // path trie of this provider's project cwds
        let mut root = Trie::default();
        for p in projects {
            let comps: Vec<&str> = p.cwd.split('/').filter(|s| !s.is_empty()).collect();
            trie_insert(&mut root, &comps, p.sessions.clone());
        }

        let total: usize = projects.iter().map(|p| p.sessions.len()).sum();
        let adapter = tree.push(Node {
            kind: Kind::Adapter,
            primary: name.clone(),
            secondary: String::new(),
            meta: format!("{total} sessions"),
            resume: None,
            session_id: None,
            children: Vec::new(),
        });
        tree.roots.push(adapter);

        let mut top: Vec<usize> = Vec::new();
        for (seg, child) in &root.children {
            top.push(convert_dir(&mut tree, format!("/{seg}"), child, commits, branches, name));
        }
        tree.nodes[adapter].children = top;
    }
    tree
}

/// Convert a trie node into a Dir node, compressing single-child empty chains
/// (so `/home/x/Documents/y` shows as one label until it branches or has sessions).
fn convert_dir(
    tree: &mut Tree,
    mut label: String,
    trie: &Trie,
    commits: &[Commit],
    branches: &[BranchRecord],
    provider: &str,
) -> usize {
    let mut cur = trie;
    while cur.sessions.is_empty() && cur.children.len() == 1 {
        let (k, child) = cur.children.iter().next().unwrap();
        label = format!("{label}/{k}");
        cur = child;
    }

    let mut children: Vec<usize> = Vec::new();
    let total_sessions = count_sessions(cur);

    // sessions first (newest first), then sub-directories
    let mut sessions = cur.sessions.clone();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    for s in &sessions {
        children.push(convert_session(tree, s, commits, branches, provider));
    }
    for (k, child) in &cur.children {
        children.push(convert_dir(tree, format!("{label}/{k}"), child, commits, branches, provider));
    }

    tree.push(Node {
        kind: Kind::Dir,
        primary: label,
        secondary: String::new(),
        meta: format!("{total_sessions} sessions"),
        resume: None,
        session_id: None,
        children,
    })
}

fn count_sessions(t: &Trie) -> usize {
    t.sessions.len() + t.children.values().map(count_sessions).sum::<usize>()
}

fn convert_session(
    tree: &mut Tree,
    s: &SessionRef,
    commits: &[Commit],
    branches: &[BranchRecord],
    provider: &str,
) -> usize {
    let mut children = Vec::new();
    for c in commits.iter().filter(|c| c.session_id == s.id) {
        let note = c.note.clone().unwrap_or_default();
        children.push(tree.push(Node {
            kind: Kind::Commit,
            primary: c.name.clone(),
            secondary: if note.is_empty() { String::new() } else { format!("\"{note}\"") },
            meta: String::new(),
            resume: None,
            session_id: None,
            children: Vec::new(),
        }));
    }
    // sily branches are always Claude Code sessions.
    for b in branches.iter().filter(|b| b.from_session == s.id) {
        let from = if b.at_message.is_empty() { "HEAD".to_string() } else { short(&b.at_message).to_string() };
        children.push(tree.push(Node {
            kind: Kind::Branch,
            primary: short(&b.session_id).to_string(),
            secondary: format!("{} (from {})", b.origin, from),
            meta: String::new(),
            resume: Some(resume_command("claude-code", &b.session_id)),
            session_id: Some(b.session_id.clone()),
            children: Vec::new(),
        }));
    }
    tree.push(Node {
        kind: Kind::Session,
        primary: short(&s.id).to_string(),
        secondary: truncate(&s.summary, 60),
        meta: meta_line(s.message_count, s.modified),
        resume: Some(resume_command(provider, &s.id)),
        session_id: Some(s.id.clone()),
        children,
    })
}

// ---------------------------------------------------------------- app / loop

struct App<'a> {
    tree: Tree,
    expanded: HashSet<usize>,
    visible: Vec<(usize, usize)>, // (node id, depth)
    sel: usize,
    status: String,
    picked: Option<String>,
    providers: &'a [Box<dyn Provider>],
    commits: &'a [Commit],
    branches: &'a [BranchRecord],
    detail: std::collections::HashMap<String, Vec<Line<'static>>>,
}

impl<'a> App<'a> {
    fn new(
        tree: Tree,
        providers: &'a [Box<dyn Provider>],
        commits: &'a [Commit],
        branches: &'a [BranchRecord],
    ) -> Self {
        let mut app = App {
            tree,
            expanded: HashSet::new(),
            visible: Vec::new(),
            sel: 0,
            status: String::new(),
            picked: None,
            providers,
            commits,
            branches,
            detail: std::collections::HashMap::new(),
        };
        // start with the adapter expanded so top-level dirs are visible
        for &r in &app.tree.roots {
            app.expanded.insert(r);
        }
        app.recompute();
        app
    }

    fn recompute(&mut self) {
        let mut out = Vec::new();
        let roots = self.tree.roots.clone();
        for r in roots {
            self.walk(r, 0, &mut out);
        }
        self.visible = out;
        if self.sel >= self.visible.len() {
            self.sel = self.visible.len().saturating_sub(1);
        }
    }

    fn walk(&self, id: usize, depth: usize, out: &mut Vec<(usize, usize)>) {
        out.push((id, depth));
        if self.expanded.contains(&id) {
            for &c in &self.tree.nodes[id].children {
                self.walk(c, depth + 1, out);
            }
        }
    }

    fn selected_node(&self) -> Option<usize> {
        self.visible.get(self.sel).map(|&(id, _)| id)
    }

    fn toggle(&mut self) {
        if let Some(id) = self.selected_node() {
            if self.tree.nodes[id].children.is_empty() {
                self.copy_resume();
            } else if !self.expanded.insert(id) {
                self.expanded.remove(&id);
            }
            self.recompute();
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(id) = self.selected_node() else { return };
        if self.expanded.contains(&id) && !self.tree.nodes[id].children.is_empty() {
            self.expanded.remove(&id);
            self.recompute();
            return;
        }
        // move selection to parent (first row above with smaller depth)
        let depth = self.visible[self.sel].1;
        for i in (0..self.sel).rev() {
            if self.visible[i].1 < depth {
                self.sel = i;
                break;
            }
        }
    }

    fn copy_resume(&mut self) {
        if let Some(id) = self.selected_node() {
            if let Some(cmd) = self.tree.nodes[id].resume.clone() {
                if copy_to_clipboard(&cmd) {
                    self.status = format!("copied: {cmd}");
                } else {
                    self.status = format!("{cmd}  (no clipboard tool; shown on exit)");
                }
                self.picked = Some(cmd);
            }
        }
    }

    fn selected_session_id(&self) -> Option<String> {
        self.selected_node().and_then(|id| self.tree.nodes[id].session_id.clone())
    }

    /// Compute (and cache) the rail graph lines for a session id.
    fn ensure_detail(&mut self, id: &str) {
        if self.detail.contains_key(id) {
            return;
        }
        let lines = match self.providers.iter().map(|b| b.as_ref()).find(|p| p.owns(id)) {
            Some(p) => match p.messages(id) {
                Ok(msgs) => rail_lines(&msgs, self.commits, self.branches, id),
                Err(e) => vec![Line::from(format!("error: {e}"))],
            },
            None => vec![Line::from("(no provider for this session)")],
        };
        self.detail.insert(id.to_string(), lines);
    }
}

fn role_lbl(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "asst",
        Role::System => "sys ",
        Role::Other => "····",
    }
}

/// Build the rail-graph lines (●─│ trunk with ◆ commits / ○ branches splitting
/// off at their point) for the detail pane.
fn rail_lines(
    msgs: &[MsgPoint],
    commits: &[Commit],
    branches: &[BranchRecord],
    id: &str,
) -> Vec<Line<'static>> {
    let yellow = Style::default().fg(Color::Yellow);
    let ybold = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    let green = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let cyan = Style::default().fg(Color::Cyan);

    if msgs.is_empty() {
        return vec![Line::from("(no messages)")];
    }
    let last = msgs.last().map(|m| m.point.clone()).unwrap_or_default();

    let mut commit_at: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in commits.iter().filter(|c| c.session_id == id) {
        let key = if c.message_uuid.is_empty() { last.clone() } else { c.message_uuid.clone() };
        commit_at.entry(key).or_default().push(format!("{}  \"{}\"", c.name, c.note.clone().unwrap_or_default()));
    }
    let mut branch_at: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for b in branches.iter().filter(|b| b.from_session == id) {
        let key = if b.at_message.is_empty() { last.clone() } else { b.at_message.clone() };
        branch_at.entry(key).or_default().push(format!("{}  {}", short(&b.session_id), b.origin));
    }

    let start = msgs.len().saturating_sub(40);
    let mut out = Vec::new();
    if start > 0 {
        out.push(Line::styled(format!("┆ … {start} earlier"), dim));
    }
    for (i, m) in msgs[start..].iter().enumerate() {
        let is_last = start + i == msgs.len() - 1;
        out.push(Line::from(vec![
            Span::styled("● ", yellow),
            Span::styled(short(&m.point).to_string(), ybold),
            Span::raw("  "),
            Span::styled(role_lbl(m.role), dim),
            Span::raw("  "),
            Span::raw(truncate(&m.text, 44)),
        ]));
        for c in commit_at.get(&m.point).into_iter().flatten() {
            out.push(Line::from(vec![Span::styled("├─◆ ", green), Span::styled(c.clone(), green)]));
        }
        for b in branch_at.get(&m.point).into_iter().flatten() {
            out.push(Line::from(vec![Span::styled("├─○ ", cyan), Span::styled(b.clone(), cyan)]));
        }
        if !is_last {
            out.push(Line::styled("│", dim));
        }
    }
    out
}

pub fn run(
    listings: &[(String, Vec<ProjectSessions>)],
    registry: &[Box<dyn Provider>],
    commits: &[Commit],
    branches: &[BranchRecord],
) -> std::io::Result<Option<String>> {
    let tree = build_tree(listings, commits, branches);
    let mut app = App::new(tree, registry, commits, branches);
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result.map(|_| app.picked)
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App<'_>) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    if app.sel + 1 < app.visible.len() {
                        app.sel += 1;
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.sel = app.sel.saturating_sub(1);
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                    app.toggle();
                }
                KeyCode::Left | KeyCode::Char('h') => app.collapse_or_parent(),
                KeyCode::Char('y') => {
                    app.copy_resume();
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn draw(f: &mut ratatui::Frame, app: &mut App<'_>) {
    let outer = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());
    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(outer[0]);

    // Left: the collapsible tree.
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&(id, depth)| row(app, id, depth))
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT).title(" sily "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    state.select(Some(app.sel));
    f.render_stateful_widget(list, body[0], &mut state);

    // Right: the selected session's rail graph.
    let sid = app.selected_session_id();
    let (title, detail): (String, Vec<Line>) = match &sid {
        Some(id) => {
            app.ensure_detail(id);
            (
                format!(" graph: {} ", &id[..id.len().min(8)]),
                app.detail.get(id).cloned().unwrap_or_default(),
            )
        }
        None => (
            " graph ".to_string(),
            vec![Line::styled(
                "select a session to see its graph",
                Style::default().fg(Color::DarkGray),
            )],
        ),
    };
    f.render_widget(
        Paragraph::new(detail).block(Block::default().borders(Borders::LEFT).title(title)),
        body[1],
    );

    let hint = if app.status.is_empty() {
        "↑↓ move · →/Enter expand · ← collapse · y copy resume · q quit".to_string()
    } else {
        app.status.clone()
    };
    f.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        outer[1],
    );
}

fn row<'a>(app: &App<'_>, id: usize, depth: usize) -> ListItem<'a> {
    let node = &app.tree.nodes[id];
    let has_children = !node.children.is_empty();
    let marker = if has_children {
        if app.expanded.contains(&id) { "▾" } else { "▸" }
    } else {
        match node.kind {
            Kind::Session => "●",
            Kind::Commit => "◆",
            Kind::Branch => "○",
            _ => " ",
        }
    };
    let color = match node.kind {
        Kind::Adapter => Color::Magenta,
        Kind::Dir => Color::Blue,
        Kind::Session => Color::Yellow,
        Kind::Commit => Color::Green,
        Kind::Branch => Color::Cyan,
    };

    let mut spans = vec![
        Span::raw("  ".repeat(depth)),
        Span::styled(format!("{marker} "), Style::default().fg(color)),
        Span::styled(
            node.primary.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ];
    if !node.secondary.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::raw(node.secondary.clone()));
    }
    if !node.meta.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(node.meta.clone(), Style::default().fg(Color::DarkGray)));
    }
    ListItem::new(Line::from(spans))
}

// ---------------------------------------------------------------- helpers

fn copy_to_clipboard(text: &str) -> bool {
    let candidates: [(&str, &[&str]); 4] = [
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (cmd, args) in candidates {
        if let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn meta_line(count: usize, modified: Option<SystemTime>) -> String {
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
