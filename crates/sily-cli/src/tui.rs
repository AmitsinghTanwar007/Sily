//! Interactive, collapsible session browser for `sily list` (when run in a
//! terminal). Shows a tree: claude-code → directories (nested by path) →
//! sessions → commits/branches. Everything starts collapsed; nodes expand on
//! demand. Browse with the arrows; copy a session's resume command with `y`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::SystemTime;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use sily_core::model::{BranchRecord, Commit, Role};
use sily_core::provider::Provider;
use sily_core::store::{ProjectSessions, SessionRef};

use crate::graph;

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
        // (count, modified) per session id, for showing branch sizes.
        let info: HashMap<String, (usize, Option<SystemTime>)> = projects
            .iter()
            .flat_map(|p| p.sessions.iter())
            .map(|s| (s.id.clone(), (s.message_count, s.modified)))
            .collect();
        // Sessions that are a branch-child of another (present) session — these
        // are shown only nested under their origin, not also as top-level.
        let mut is_child: HashSet<String> = HashSet::new();
        for b in branches {
            if info.contains_key(&b.session_id) && info.contains_key(&b.from_session) {
                is_child.insert(b.session_id.clone());
            }
        }

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
            top.push(convert_dir(&mut tree, format!("/{seg}"), child, commits, branches, name, &is_child, &info));
        }
        tree.nodes[adapter].children = top;
    }
    tree
}

/// Convert a trie node into a Dir node, compressing single-child empty chains
/// (so `/home/x/Documents/y` shows as one label until it branches or has sessions).
#[allow(clippy::too_many_arguments)]
fn convert_dir(
    tree: &mut Tree,
    mut label: String,
    trie: &Trie,
    commits: &[Commit],
    branches: &[BranchRecord],
    provider: &str,
    is_child: &HashSet<String>,
    info: &HashMap<String, (usize, Option<SystemTime>)>,
) -> usize {
    let mut cur = trie;
    while cur.sessions.is_empty() && cur.children.len() == 1 {
        let (k, child) = cur.children.iter().next().unwrap();
        label = format!("{label}/{k}");
        cur = child;
    }

    let mut children: Vec<usize> = Vec::new();
    let total_sessions = count_sessions(cur);

    // top-level sessions (newest first), skipping branch-children (shown nested)
    let mut sessions = cur.sessions.clone();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    for s in &sessions {
        if is_child.contains(&s.id) {
            continue;
        }
        children.push(convert_session(tree, s, commits, branches, provider, info));
    }
    for (k, child) in &cur.children {
        children.push(convert_dir(tree, format!("{label}/{k}"), child, commits, branches, provider, is_child, info));
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
    info: &HashMap<String, (usize, Option<SystemTime>)>,
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
        let (cnt, modified) = info.get(&b.session_id).copied().unwrap_or((0, None));
        children.push(tree.push(Node {
            kind: Kind::Branch,
            primary: short(&b.session_id).to_string(),
            secondary: format!("{} (from {})", b.origin, from),
            meta: meta_line(cnt, modified),
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

    /// Rebuild the tree from fresh listings and drop the graph cache, so changes
    /// made elsewhere (e.g. continuing a session) show up without restarting.
    fn reload(&mut self, listings: &[(String, Vec<ProjectSessions>)]) {
        self.tree = build_tree(listings, self.commits, self.branches);
        self.detail.clear();
        for &r in &self.tree.roots {
            self.expanded.insert(r);
        }
        self.recompute();
        self.status = "reloaded".to_string();
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

    /// Compute (and cache) the swimlane graph lines for a session id: the main
    /// timeline with each branch's divergent messages shown in a parallel lane.
    fn ensure_detail(&mut self, id: &str) {
        if self.detail.contains_key(id) {
            return;
        }
        let provider = self.providers.iter().map(|b| b.as_ref()).find(|p| p.owns(id));
        let lines = match provider.and_then(|p| p.messages(id).ok()) {
            Some(main_msgs) => {
                // Same multi-lane layout as `sily graph`, rendered to ratatui lines.
                let commits: Vec<Commit> =
                    self.commits.iter().filter(|c| c.session_id == id).cloned().collect();
                let mut gbs: Vec<graph::GraphBranch> = Vec::new();
                for b in self.branches.iter().filter(|b| b.from_session == id) {
                    let bm = self
                        .providers
                        .iter()
                        .map(|x| x.as_ref())
                        .find(|x| x.owns(&b.session_id))
                        .and_then(|x| x.messages(&b.session_id).ok())
                        .unwrap_or_default();
                    let common = main_msgs
                        .iter()
                        .zip(bm.iter())
                        .take_while(|(a, c)| a.role == c.role && a.text == c.text)
                        .count();
                    let fork_point =
                        if common > 0 { main_msgs[common - 1].point.clone() } else { String::new() };
                    let tail = bm.into_iter().skip(common).collect();
                    gbs.push(graph::GraphBranch {
                        id: short(&b.session_id).to_string(),
                        origin: b.origin.clone(),
                        fork_point,
                        tail,
                    });
                }
                let lay = graph::session_layout(id, &main_msgs, &commits, &gbs, Some(60));
                layout_lines(&lay)
            }
            None => vec![Line::from("(no messages)")],
        };
        self.detail.insert(id.to_string(), lines);
    }
}

/// Render a shared [`graph::GraphLayout`] to ratatui lines for the detail pane.
fn layout_lines(lay: &graph::GraphLayout) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let green = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let cyan = Style::default().fg(Color::Cyan);
    let cyanb = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mainnode = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let userc = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let aic = Style::default().fg(Color::White);

    let mut out = vec![Line::from(vec![
        Span::styled("▌ ", Style::default().fg(Color::Magenta)),
        Span::styled(lay.id.clone(), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
        Span::styled("  · newest first", dim),
    ])];
    out.push(Line::from(""));
    if let Some(note) = &lay.empty_note {
        out.push(Line::styled(note.clone(), dim));
        return out;
    }
    for row in &lay.rows {
        let mut spans: Vec<Span> = Vec::new();
        for cell in &row.cells {
            match cell {
                graph::RailCell::MainNode => spans.push(Span::styled("●", mainnode)),
                graph::RailCell::BranchNode => spans.push(Span::styled("●", cyan)),
                graph::RailCell::Dim(c) => spans.push(Span::styled(c.to_string(), dim)),
            }
        }
        spans.push(Span::raw(" "));
        let rolestyle = if row.is_main {
            match row.role {
                Role::User => userc,
                Role::Assistant => aic,
                _ => dim,
            }
        } else {
            cyan
        };
        spans.push(Span::styled(format!("{}  ", who(row.role)), rolestyle));
        spans.push(Span::raw(truncate(&row.text, 44)));
        if let Some(bid) = &row.branch_tip {
            spans.push(Span::styled(format!("  ← branch {bid}"), cyanb));
        }
        for c in &row.commits {
            spans.push(Span::styled(format!("  ◆ {c}"), green));
        }
        for eid in &row.stubs {
            spans.push(Span::styled(format!("  ╰○ {eid}"), cyanb));
        }
        out.push(Line::from(spans));
    }
    for (eid, origin) in &lay.bottom_empties {
        out.push(Line::styled(format!("○ {eid}  {origin} · no new conversation"), dim));
    }
    if lay.earlier > 0 {
        out.push(Line::styled(format!("┆ … {} earlier", lay.earlier), dim));
    }
    out
}

/// Human-friendly speaker label for the detail pane.
fn who(r: Role) -> &'static str {
    match r {
        Role::User => "you",
        Role::Assistant => "ai ",
        Role::System => "sys",
        Role::Other => "·  ",
    }
}


pub fn run<F>(
    relist: F,
    registry: &[Box<dyn Provider>],
    commits: &[Commit],
    branches: &[BranchRecord],
) -> std::io::Result<Option<String>>
where
    F: Fn() -> Vec<(String, Vec<ProjectSessions>)>,
{
    let listings = relist();
    let mut app = App::new(build_tree(&listings, commits, branches), registry, commits, branches);
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &relist);
    ratatui::restore();
    result.map(|_| app.picked)
}

fn event_loop<F>(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App<'_>,
    relist: &F,
) -> std::io::Result<()>
where
    F: Fn() -> Vec<(String, Vec<ProjectSessions>)>,
{
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
                KeyCode::Char('r') => {
                    let listings = relist();
                    app.reload(&listings);
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
        "↑↓ move · →/Enter expand · ← collapse · y copy resume · r reload · q quit".to_string()
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
