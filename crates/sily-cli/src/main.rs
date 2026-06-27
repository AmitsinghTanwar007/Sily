//! sily — a git-like commit / branch / revert system for AI sessions.
//!
//! The CLI is a thin driver over a registry of [`Provider`]s (one per tool:
//! Claude Code, Codex, OpenCode, Gemini). Every command resolves the right
//! provider by id and calls the trait — so behaviour is uniform and adding a
//! tool is just one more `impl Provider`.

mod branchstore;
mod commitstore;
mod graph;
mod render;
mod tui;
mod update;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use sily_core::model::{BranchRecord, Commit, Role};
use sily_core::ops::diff;
use sily_core::provider::{MsgPoint, Provider};

use sily_adapter_claude::ClaudeProvider;
use sily_adapter_codex::CodexProvider;
use sily_adapter_gemini::GeminiProvider;
use sily_adapter_opencode::OpenCodeProvider;
use sily_adapter_pi::PiProvider;

use branchstore::BranchStore;
use commitstore::CommitStore;

/// Default number of recent entries shown by `log`/`tree` (override with --full).
const DEFAULT_LIMIT: usize = 8;

/// Providers that can be a `port` target (they can write a new session).
const PORT_TARGETS: [&str; 3] = ["claude-code", "codex-cli", "opencode"];

#[derive(Debug)]
enum CliError {
    Core(sily_core::Error),
    Io(std::io::Error),
    Msg(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Core(e) => write!(f, "{e}"),
            CliError::Io(e) => write!(f, "{e}"),
            CliError::Msg(m) => write!(f, "{m}"),
        }
    }
}

impl From<sily_core::Error> for CliError {
    fn from(e: sily_core::Error) -> Self {
        CliError::Core(e)
    }
}
impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Io(e)
    }
}
impl From<String> for CliError {
    fn from(m: String) -> Self {
        CliError::Msg(m)
    }
}
impl From<&str> for CliError {
    fn from(m: &str) -> Self {
        CliError::Msg(m.to_string())
    }
}

#[derive(Parser)]
#[command(name = "sily", version, about = "git-like commit / branch / revert for AI sessions")]
struct Cli {
    /// Directory `list` scopes to (defaults to the current directory).
    #[arg(long, global = true)]
    cwd: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List sessions under the current directory as a tree (use --all for every project).
    List {
        #[arg(long)]
        all: bool,
    },
    /// Print a session's history (last 8 by default).
    Log {
        session: String,
        /// Show only the user's prompts.
        #[arg(short, long)]
        prompts: bool,
        /// Show the full history.
        #[arg(short, long)]
        full: bool,
    },
    /// Show a session's branch tree (last 8 by default).
    Tree {
        session: String,
        #[arg(short, long)]
        full: bool,
    },
    /// GitHub-style rail graph: the session's timeline with branches/commits
    /// splitting off at the exact point they were made.
    Graph {
        session: String,
        #[arg(short, long)]
        full: bool,
    },
    /// Save a commit (a named pointer) at a session's HEAD or a chosen point.
    Commit {
        session: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(short, long)]
        message: Option<String>,
        /// Point to pin at (defaults to the session's last message).
        #[arg(long)]
        at: Option<String>,
    },
    /// List saved commits.
    Commits,
    /// Create a new session branched from a point (defaults to HEAD).
    Branch {
        session: String,
        #[arg(long)]
        at: Option<String>,
    },
    /// Restore a commit. Default: fork into a NEW session (non-destructive).
    Revert {
        commit: String,
        #[arg(long)]
        hard: bool,
    },
    /// Show where two sessions diverge (claude-code only).
    Diff { a: String, b: String },
    /// Merge a branch back into its main (new session = main + the branch's work).
    Merge {
        /// The branch session to merge.
        branch: String,
        /// Main session to merge into (defaults to the branch's recorded origin).
        #[arg(long)]
        into: Option<String>,
    },
    /// Copy a session's content into a NEW session in another AI tool.
    Port {
        session: String,
        /// Target provider. Prompts if omitted.
        #[arg(long)]
        to: Option<String>,
    },
    /// Update sily to the latest release.
    Update,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sily: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Ctx {
    commits: CommitStore,
    branches: BranchStore,
    cwd: String,
    providers: Vec<Box<dyn Provider>>,
}

impl Ctx {
    /// The provider that owns a session id.
    fn provider_for(&self, id: &str) -> Result<&dyn Provider, CliError> {
        self.providers
            .iter()
            .map(|b| b.as_ref())
            .find(|p| p.owns(id))
            .ok_or_else(|| CliError::Core(sily_core::Error::SessionNotFound(id.to_string())))
    }

    fn provider_by_name(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.iter().map(|b| b.as_ref()).find(|p| p.name() == name)
    }

    /// Session listings from every provider (skips empty/uninstalled).
    fn listings(&self) -> Vec<(String, Vec<sily_core::store::ProjectSessions>)> {
        let mut out = Vec::new();
        for p in &self.providers {
            match p.list_projects() {
                Ok(ps) if !ps.is_empty() => out.push((p.name().to_string(), ps)),
                Ok(_) => {}
                Err(e) => eprintln!("sily: {} adapter error: {e}", p.name()),
            }
        }
        out
    }
}

fn build_ctx(cwd_override: Option<String>) -> Result<Ctx, CliError> {
    let home = dirs::home_dir().ok_or(CliError::from("cannot locate home directory"))?;
    let env = |k: &str, default: std::path::PathBuf| {
        std::env::var_os(k).map(std::path::PathBuf::from).unwrap_or(default)
    };
    let claude_home = env("SILY_CLAUDE_HOME", home.join(".claude"));
    let sily_home = env("SILY_HOME", home.join(".sily"));
    let codex_home = env("SILY_CODEX_HOME", home.join(".codex"));
    let opencode_db = env("SILY_OPENCODE_DB", sily_adapter_opencode::default_db_path(&home));
    let gemini_home = env("SILY_GEMINI_HOME", home.join(".gemini"));
    let pi_dir = env("SILY_PI_DIR", home.join(".pi/agent/sessions"));
    let cwd = match cwd_override {
        Some(c) => c,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(ClaudeProvider::new(claude_home)),
        Box::new(CodexProvider::new(codex_home)),
        Box::new(OpenCodeProvider::new(opencode_db)),
        Box::new(GeminiProvider::new(gemini_home)),
        Box::new(PiProvider::new(pi_dir)),
    ];
    Ok(Ctx {
        commits: CommitStore::new(&sily_home),
        branches: BranchStore::new(&sily_home),
        cwd,
        providers,
    })
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn run(cli: Cli) -> Result<(), CliError> {
    if let Cmd::Update = cli.cmd {
        return update::run().map_err(CliError::Msg);
    }
    let ctx = build_ctx(cli.cwd)?;
    match cli.cmd {
        Cmd::List { all } => cmd_list(&ctx, all)?,
        Cmd::Log { session, prompts, full } => {
            let limit = if full { None } else { Some(DEFAULT_LIMIT) };
            let msgs = ctx.provider_for(&session)?.messages(&session)?;
            if prompts {
                print_prompts(msgs, limit);
            } else {
                print_messages(msgs, limit);
            }
        }
        Cmd::Tree { session, full } => {
            let limit = if full { None } else { Some(DEFAULT_LIMIT) };
            let p = ctx.provider_for(&session)?;
            match p.structured(&session)? {
                Some(s) => print!("{}", render::tree(&s, limit)),
                None => print_messages(p.messages(&session)?, limit),
            }
        }
        Cmd::Graph { session, full } => {
            let limit = if full { None } else { Some(DEFAULT_LIMIT) };
            let msgs = ctx.provider_for(&session)?.messages(&session)?;
            let commits: Vec<Commit> =
                ctx.commits.all()?.into_iter().filter(|c| c.session_id == session).collect();
            let branches: Vec<BranchRecord> =
                ctx.branches.all()?.into_iter().filter(|b| b.from_session == session).collect();
            print!("{}", graph::session_graph(&session, &msgs, &commits, &branches, limit));
        }
        Cmd::Commit { session, name, message, at } => cmd_commit(&ctx, session, name, message, at)?,
        Cmd::Commits => cmd_commits(&ctx)?,
        Cmd::Branch { session, at } => cmd_branch(&ctx, session, at)?,
        Cmd::Revert { commit, hard } => cmd_revert(&ctx, commit, hard)?,
        Cmd::Diff { a, b } => cmd_diff(&ctx, a, b)?,
        Cmd::Merge { branch, into } => cmd_merge(&ctx, branch, into)?,
        Cmd::Port { session, to } => cmd_port(&ctx, session, to)?,
        Cmd::Update => unreachable!(),
    }
    Ok(())
}

// ------------------------------------------------------------------ commands

fn cmd_list(ctx: &Ctx, all: bool) -> Result<(), CliError> {
    let commits = ctx.commits.all()?;
    let branches = ctx.branches.all()?;
    let mut providers = ctx.listings();
    if !all {
        providers = scope_to_dir(providers, &ctx.cwd);
    }
    if providers.is_empty() {
        if all {
            println!("(no sessions found)");
        } else {
            println!("(no sessions under {} — use 'sily list --all' to see every project)", ctx.cwd);
        }
    } else if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
        if let Some(cmd) = tui::run(&providers, &ctx.providers, &commits, &branches)
            .map_err(|e| CliError::Msg(e.to_string()))?
        {
            println!("{cmd}");
        }
    } else {
        print!("{}", graph::render_all(&providers, &commits, &branches));
    }
    Ok(())
}

fn cmd_commit(
    ctx: &Ctx,
    session: String,
    name: Option<String>,
    message: Option<String>,
    at: Option<String>,
) -> Result<(), CliError> {
    let p = ctx.provider_for(&session)?;
    let point = match at {
        Some(a) => {
            let pts = p.messages(&session)?;
            if !pts.iter().any(|m| m.point == a) {
                return Err(CliError::Msg(format!("point '{a}' not found in session")));
            }
            a
        }
        None => p
            .messages(&session)?
            .last()
            .map(|m| m.point.clone())
            .unwrap_or_default(),
    };
    let existing = ctx.commits.all()?;
    let name = name.unwrap_or_else(|| format!("c{}", existing.len() + 1));
    ctx.commits.add(Commit {
        name: name.clone(),
        session_id: session,
        message_uuid: point.clone(),
        created_at: now_iso(),
        note: message,
    })?;
    let label = if point.is_empty() { "HEAD".to_string() } else { short(&point) };
    println!("committed '{name}' at {label}");
    Ok(())
}

fn cmd_commits(ctx: &Ctx) -> Result<(), CliError> {
    let all = ctx.commits.all()?;
    if all.is_empty() {
        println!("(no commits yet)");
    }
    // Newest first.
    for c in all.into_iter().rev() {
        println!(
            "{:<14} {} @ {}  {}",
            c.name,
            short(&c.session_id),
            if c.message_uuid.is_empty() { "HEAD".into() } else { short(&c.message_uuid) },
            c.note.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn cmd_branch(ctx: &Ctx, session: String, at: Option<String>) -> Result<(), CliError> {
    let new = ctx.provider_for(&session)?.branch(&session, at.as_deref())?;
    ctx.branches.add(BranchRecord {
        session_id: new.id.clone(),
        from_session: session,
        at_message: at.unwrap_or_default(),
        origin: "branch".to_string(),
        created_at: now_iso(),
    })?;
    println!("created session {} ({} messages)", new.id, new.messages);
    println!("  resume with:  {}", new.resume);
    Ok(())
}

fn cmd_revert(ctx: &Ctx, commit: String, hard: bool) -> Result<(), CliError> {
    let c = ctx.commits.find(&commit)?.ok_or_else(|| format!("no such commit: {commit}"))?;
    let point = if c.message_uuid.is_empty() { None } else { Some(c.message_uuid.as_str()) };
    let p = ctx.provider_for(&c.session_id)?;
    if hard {
        let pt = point.ok_or("commit has no point for --hard")?;
        let kept = p.truncate(&c.session_id, pt)?;
        println!("hard-reset {} to commit '{}' ({kept} messages kept)", short(&c.session_id), c.name);
    } else {
        let new = p.branch(&c.session_id, point)?;
        ctx.branches.add(BranchRecord {
            session_id: new.id.clone(),
            from_session: c.session_id.clone(),
            at_message: c.message_uuid.clone(),
            origin: c.name.clone(),
            created_at: now_iso(),
        })?;
        println!("reverted commit '{}' into session {} ({} messages)", c.name, new.id, new.messages);
        println!("  resume with:  {}", new.resume);
    }
    Ok(())
}

fn cmd_diff(ctx: &Ctx, a: String, b: String) -> Result<(), CliError> {
    let unsupported = || CliError::Msg("diff is only supported for claude-code sessions".into());
    let sa = ctx.provider_for(&a)?.structured(&a)?.ok_or_else(unsupported)?;
    let sb = ctx.provider_for(&b)?.structured(&b)?.ok_or_else(unsupported)?;
    let ha = sa.messages.last().map(|m| m.uuid.clone()).ok_or("session has no messages")?;
    let hb = sb.messages.last().map(|m| m.uuid.clone()).ok_or("session has no messages")?;
    let d = diff(&sa, &ha, &sb, &hb)?;
    match &d.common_ancestor {
        Some(anc) => println!("common ancestor: {} ({} shared messages)", short(anc), d.common_len),
        None => println!("no common ancestor"),
    }
    println!("only in {}: {} messages", short(&a), d.only_a.len());
    println!("only in {}: {} messages", short(&b), d.only_b.len());
    Ok(())
}

fn cmd_merge(ctx: &Ctx, branch: String, into: Option<String>) -> Result<(), CliError> {
    // Target: explicit --into (any session, incl. another branch), else the
    // branch's recorded origin.
    let main = match into {
        Some(m) => m,
        None => ctx
            .branches
            .all()?
            .into_iter()
            .find(|b| b.session_id == branch)
            .map(|r| r.from_session)
            .ok_or_else(|| {
                CliError::Msg(format!(
                    "no recorded origin for {} — pass --into <session> to choose what to merge into",
                    short(&branch)
                ))
            })?,
    };
    let new = ctx.provider_for(&branch)?.merge(&main, &branch)?;
    ctx.branches.add(BranchRecord {
        session_id: new.id.clone(),
        from_session: main.clone(),
        at_message: String::new(),
        origin: format!("merge of {}", short(&branch)),
        created_at: now_iso(),
    })?;
    println!(
        "merged {} into {} → session {} ({} messages)",
        short(&branch),
        short(&main),
        new.id,
        new.messages
    );
    println!("  resume with:  {}", new.resume);
    Ok(())
}

fn cmd_port(ctx: &Ctx, session: String, to: Option<String>) -> Result<(), CliError> {
    let p = ctx.provider_for(&session)?;
    let transcript = transcript_of(p, &session)?;
    if transcript.is_empty() {
        return Err(CliError::Msg("no content to port from that session".into()));
    }
    let source = p.name();
    let context = build_context(source, &session, &transcript);
    let target_name = match to {
        Some(t) => normalize_provider(&t)?,
        None => choose_provider()?,
    };
    if target_name == source {
        eprintln!("sily: note — porting into the same provider ({source}); use 'branch' for same-tool forks.");
    }
    let target = ctx
        .provider_by_name(target_name)
        .ok_or_else(|| CliError::Msg(format!("unknown provider: {target_name}")))?;
    let new = target.create_session(&ctx.cwd, &context)?;
    println!("ported {} messages → {target_name} session {}", transcript.len(), new.id);
    println!("  resume with:  {}", new.resume);
    Ok(())
}

// ------------------------------------------------------------------ helpers

/// Keep only projects whose cwd is at or under `base`; drop now-empty providers.
fn scope_to_dir(
    providers: Vec<(String, Vec<sily_core::store::ProjectSessions>)>,
    base: &str,
) -> Vec<(String, Vec<sily_core::store::ProjectSessions>)> {
    let prefix = format!("{}/", base.trim_end_matches('/'));
    providers
        .into_iter()
        .filter_map(|(name, projects)| {
            let kept: Vec<_> = projects
                .into_iter()
                .filter(|p| p.cwd == base || p.cwd.starts_with(&prefix))
                .collect();
            (!kept.is_empty()).then_some((name, kept))
        })
        .collect()
}

fn role_label(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "asst",
        Role::System => "sys ",
        Role::Other => "????",
    }
}

/// Print messages (last N) for `log`/non-structured `tree`.
fn print_messages(msgs: Vec<MsgPoint>, limit: Option<usize>) {
    if msgs.is_empty() {
        println!("(no messages)");
        return;
    }
    let start = match limit {
        Some(n) if msgs.len() > n => msgs.len() - n,
        _ => 0,
    };
    // Newest first.
    for m in msgs[start..].iter().rev() {
        println!("{:<8}  {}  {}", short(&m.point), role_label(m.role), clip(&m.text, 80));
    }
    if start > 0 {
        println!("… {start} earlier messages (use --full to see all)");
    }
}

/// Print only the user's real prompts (numbered, last N).
fn print_prompts(msgs: Vec<MsgPoint>, limit: Option<usize>) {
    let prompts: Vec<String> = msgs
        .into_iter()
        .filter(|m| matches!(m.role, Role::User) && render::is_real_prompt(&m.text))
        .map(|m| m.text)
        .collect();
    if prompts.is_empty() {
        println!("(no user prompts)");
        return;
    }
    let start = match limit {
        Some(n) if prompts.len() > n => prompts.len() - n,
        _ => 0,
    };
    // Newest first (numbers keep their original order).
    for (i, p) in prompts[start..].iter().enumerate().rev() {
        println!("{:>3}. {}", start + i + 1, clip(p, 100));
    }
    if start > 0 {
        println!("… {start} earlier prompts (use --full to see all)");
    }
}

/// A session's conversation as (role, text), dropping empties and user-side noise.
fn transcript_of(p: &dyn Provider, id: &str) -> Result<Vec<(String, String)>, CliError> {
    Ok(p.messages(id)?
        .into_iter()
        .map(|m| (role_label(m.role).trim().to_string(), m.text))
        .filter(|(role, text)| {
            let t = text.trim();
            !t.is_empty() && (role != "user" || render::is_real_prompt(t))
        })
        .collect())
}

/// Build the context message that seeds a ported (cross-provider) session.
fn build_context(source: &str, id: &str, transcript: &[(String, String)]) -> String {
    let mut s = format!(
        "[Context ported by sily from a {source} session ({})]\n\n\
         The following is a prior AI coding conversation. Continue the work from where it left off.\n\n\
         --- transcript ---\n",
        short(id)
    );
    let mut total = 0usize;
    for (role, text) in transcript {
        let body: String = if role == "asst" && text.chars().count() > 800 {
            let mut t: String = text.chars().take(800).collect();
            t.push('…');
            t
        } else {
            text.clone()
        };
        let line = format!("{}: {}\n\n", role.to_uppercase(), body.trim());
        if total + line.len() > 40_000 {
            s.push_str("[… earlier transcript truncated by sily …]\n");
            break;
        }
        total += line.len();
        s.push_str(&line);
    }
    s.push_str("--- end transcript ---\n");
    s
}

fn choose_provider() -> Result<&'static str, CliError> {
    use std::io::Write;
    if !std::io::stdin().is_terminal() {
        return Err(CliError::Msg(format!(
            "not a terminal — pass --to <{}>",
            PORT_TARGETS.join("|")
        )));
    }
    println!("Port to which provider?");
    for (i, name) in PORT_TARGETS.iter().enumerate() {
        println!("  {}) {name}", i + 1);
    }
    print!("> ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| CliError::Msg(e.to_string()))?;
    normalize_provider(line.trim())
}

fn normalize_provider(s: &str) -> Result<&'static str, CliError> {
    match s.trim() {
        "1" | "claude" | "claude-code" => Ok("claude-code"),
        "2" | "codex" | "codex-cli" => Ok("codex-cli"),
        "3" | "opencode" => Ok("opencode"),
        other => Err(CliError::Msg(format!(
            "unknown provider '{other}' (use {})",
            PORT_TARGETS.join(" | ")
        ))),
    }
}

/// First 8 chars of an id/point for compact display.
fn short(s: &str) -> String {
    s.chars().take(8).collect()
}

/// Collapse whitespace and truncate to `n` chars for one-line display.
fn clip(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        let mut t: String = one.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
