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

use sily_core::model::{BranchRecord, Commit};
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
    session_id: Option<String>,
    children: Vec<usize>,
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
            session_id: None,
            children: Vec::new(),
        });
        tree.roots.push(adapter);

        let mut top: Vec<usize> = Vec::new();
        for (seg, child) in &root.children {
            top.push(convert_dir(&mut tree, format!("/{seg}"), child, commits, branches));
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
        children.push(convert_session(tree, s, commits, branches));
    }
    for (k, child) in &cur.children {
        children.push(convert_dir(tree, format!("{label}/{k}"), child, commits, branches));
    }

    let id = tree.push(Node {
        kind: Kind::Dir,
        primary: label,
        secondary: String::new(),
        meta: format!("{total_sessions} sessions"),
        session_id: None,
        children,
    });
    id
}

fn count_sessions(t: &Trie) -> usize {
    t.sessions.len() + t.children.values().map(count_sessions).sum::<usize>()
}

fn convert_session(
    tree: &mut Tree,
    s: &SessionRef,
    commits: &[Commit],
    branches: &[BranchRecord],
) -> usize {
    let mut children = Vec::new();
    for c in commits.iter().filter(|c| c.session_id == s.id) {
        let note = c.note.clone().unwrap_or_default();
        children.push(tree.push(Node {
            kind: Kind::Commit,
            primary: c.name.clone(),
            secondary: if note.is_empty() { String::new() } else { format!("\"{note}\"") },
            meta: String::new(),
            session_id: None,
            children: Vec::new(),
        }));
    }
    for b in branches.iter().filter(|b| b.from_session == s.id) {
        children.push(tree.push(Node {
            kind: Kind::Branch,
            primary: short(&b.session_id).to_string(),
            secondary: b.origin.clone(),
            meta: String::new(),
            session_id: Some(b.session_id.clone()),
            children: Vec::new(),
        }));
    }
    tree.push(Node {
        kind: Kind::Session,
        primary: short(&s.id).to_string(),
        secondary: truncate(&s.summary, 60),
        meta: meta_line(s.message_count, s.modified),
        session_id: Some(s.id.clone()),
        children,
    })
}

// ---------------------------------------------------------------- app / loop

struct App {
    tree: Tree,
    expanded: HashSet<usize>,
    visible: Vec<(usize, usize)>, // (node id, depth)
    sel: usize,
    status: String,
    picked: Option<String>,
}

impl App {
    fn new(tree: Tree) -> Self {
        let mut app = App {
            tree,
            expanded: HashSet::new(),
            visible: Vec::new(),
            sel: 0,
            status: String::new(),
            picked: None,
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
            if let Some(sid) = self.tree.nodes[id].session_id.clone() {
                let cmd = format!("claude --resume {sid}");
                if copy_to_clipboard(&cmd) {
                    self.status = format!("copied: {cmd}");
                } else {
                    self.status = format!("{cmd}  (no clipboard tool; shown on exit)");
                }
                self.picked = Some(cmd);
            }
        }
    }
}

pub fn run(
    providers: &[(String, Vec<ProjectSessions>)],
    commits: &[Commit],
    branches: &[BranchRecord],
) -> std::io::Result<Option<String>> {
    let mut app = App::new(build_tree(providers, commits, branches));
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result.map(|_| app.picked)
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
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

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());

    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&(id, depth)| row(app, id, depth))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::BOTTOM).title(" sily "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    state.select(Some(app.sel));
    f.render_stateful_widget(list, chunks[0], &mut state);

    let hint = if app.status.is_empty() {
        "↑↓ move   →/Enter expand   ← collapse   y copy resume   q quit".to_string()
    } else {
        app.status.clone()
    };
    f.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}

fn row<'a>(app: &App, id: usize, depth: usize) -> ListItem<'a> {
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
