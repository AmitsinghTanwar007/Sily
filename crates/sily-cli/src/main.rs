//! sily — a git-like commit / branch / revert system for AI sessions.
//!
//! The CLI is provider-agnostic in spirit: it talks to a `SessionStore`. Today
//! the only backend is Claude Code; a `--provider` flag / registry can select
//! others later without touching command logic.

mod branchstore;
mod commitstore;
mod graph;
mod render;
mod tui;
mod update;

use std::io::IsTerminal;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use sily_core::model::{BranchRecord, Commit};
use sily_core::ops::{branch_at, diff, truncate_at};
use sily_core::store::SessionStore;
use sily_adapter_claude::ClaudeStore;

use branchstore::BranchStore;
use commitstore::CommitStore;

/// Default number of recent entries shown by `log`/`tree` (override with --full).
const DEFAULT_LIMIT: usize = 8;

/// One error type for the CLI. `From` impls let command code use `?` directly
/// instead of stringifying every fallible call.
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
    /// Project working directory whose sessions to operate on
    /// (defaults to the current directory).
    #[arg(long, global = true)]
    cwd: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List sessions under the current directory as a tree (use --all for every project).
    List {
        /// Show every project on the machine, not just the current directory subtree.
        #[arg(long)]
        all: bool,
    },
    /// Print the linear history of a session (last 8 entries by default).
    Log {
        session: String,
        /// Show only the user's prompts (skip assistant turns, tools, noise).
        #[arg(short, long)]
        prompts: bool,
        /// Show the full history instead of just the last few entries.
        #[arg(short, long)]
        full: bool,
    },
    /// Show the branch tree of a session (last 8 entries by default).
    Tree {
        session: String,
        /// Show the full tree instead of just the last few entries.
        #[arg(short, long)]
        full: bool,
    },
    /// Save a commit (a named pointer) at a session's HEAD or a chosen message.
    Commit {
        session: String,
        /// Name for the commit (defaults to c1, c2, …).
        #[arg(long)]
        name: Option<String>,
        /// Note to attach.
        #[arg(short, long)]
        message: Option<String>,
        /// Message uuid to point at (defaults to the session's last message).
        #[arg(long)]
        at: Option<String>,
    },
    /// List saved commits.
    Commits,
    /// Create a new session branched from a message (defaults to HEAD).
    Branch {
        session: String,
        #[arg(long)]
        at: Option<String>,
    },
    /// Restore a commit. Default: fork into a NEW session (non-destructive).
    Revert {
        commit: String,
        /// Destructive: truncate the original session back to the commit.
        #[arg(long)]
        hard: bool,
    },
    /// Show where two sessions diverge.
    Diff { a: String, b: String },
    /// Copy a session's content into a NEW session in another AI tool.
    Port {
        session: String,
        /// Target provider (claude-code | codex-cli | opencode). Prompts if omitted.
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
    claude_home: std::path::PathBuf,
    codex_home: std::path::PathBuf,
    opencode_db: std::path::PathBuf,
    /// The working directory `list` scopes to by default.
    cwd: String,
}

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

/// Gather session listings from every supported provider, skipping ones that
/// aren't installed (empty) and warning on errors.
fn gather_providers(ctx: &Ctx) -> Vec<(String, Vec<sily_core::store::ProjectSessions>)> {
    let mut out = Vec::new();
    let mut add = |name: &str, res: sily_core::Result<Vec<sily_core::store::ProjectSessions>>| match res
    {
        Ok(projects) if !projects.is_empty() => out.push((name.to_string(), projects)),
        Ok(_) => {}
        Err(e) => eprintln!("sily: {name} adapter error: {e}"),
    };
    add("claude-code", sily_adapter_claude::list_all_projects(&ctx.claude_home));
    add("codex-cli", sily_adapter_codex::list_all_projects(&ctx.codex_home));
    add("opencode", sily_adapter_opencode::list_all_projects(&ctx.opencode_db));
    out
}

/// Which provider owns a session id. OpenCode ids start with `ses_`; a Codex id
/// matches a rollout file; otherwise it's treated as Claude Code.
fn detect_provider(ctx: &Ctx, id: &str) -> &'static str {
    if id.starts_with("ses_") {
        "opencode"
    } else if sily_adapter_codex::find_session_file(&ctx.codex_home, id).is_some() {
        "codex-cli"
    } else {
        "claude-code"
    }
}

/// A Claude store pointed at the project that actually contains `id` (anywhere
/// under the Claude home), so commands work regardless of the current directory.
fn claude_store_for(ctx: &Ctx, id: &str) -> Result<ClaudeStore, CliError> {
    match sily_adapter_claude::locate(&ctx.claude_home, id) {
        Some((dir, cwd)) => Ok(ClaudeStore::from_project_dir(dir, cwd)),
        None => Err(CliError::Core(sily_core::Error::SessionNotFound(id.to_string()))),
    }
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn build_ctx(cwd_override: Option<String>) -> Result<Ctx, CliError> {
    let home = dirs::home_dir().ok_or(CliError::from("cannot locate home directory"))?;
    // Env overrides make the homes relocatable (and let tests run isolated).
    let claude_home = std::env::var_os("SILY_CLAUDE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let sily_home = std::env::var_os("SILY_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".sily"));
    let codex_home = std::env::var_os("SILY_CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let opencode_db = std::env::var_os("SILY_OPENCODE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| sily_adapter_opencode::default_db_path(&home));
    let cwd = match cwd_override {
        Some(c) => c,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    Ok(Ctx {
        commits: CommitStore::new(&sily_home),
        branches: BranchStore::new(&sily_home),
        claude_home,
        codex_home,
        opencode_db,
        cwd,
    })
}

/// Print only the user's real prompts from (role, text) pairs, numbered, with an
/// optional last-N limit.
fn print_user_prompts(points: Vec<(String, String)>, limit: Option<usize>) {
    let prompts: Vec<String> = points
        .into_iter()
        .filter(|(role, text)| role == "user" && render::is_real_prompt(text))
        .map(|(_, text)| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    if prompts.is_empty() {
        println!("(no user prompts)");
        return;
    }
    let start = match limit {
        Some(n) if prompts.len() > n => prompts.len() - n,
        _ => 0,
    };
    if start > 0 {
        println!("… {start} earlier prompts (use --full to see all)");
    }
    for (i, p) in prompts[start..].iter().enumerate() {
        println!("{:>3}. {}", start + i + 1, clip(p, 100));
    }
}

/// Read a session's conversation as (role, full-text) pairs, from any provider,
/// dropping empties and user-side noise (env context, command/caveat wrappers).
fn read_transcript(ctx: &Ctx, id: &str) -> Result<Vec<(String, String)>, CliError> {
    let raw: Vec<(String, String)> = match detect_provider(ctx, id) {
        "codex-cli" => sily_adapter_codex::message_points(&ctx.codex_home, id)?
            .into_iter()
            .map(|(_, r, t)| (r, t))
            .collect(),
        "opencode" => sily_adapter_opencode::message_points(id)?
            .into_iter()
            .map(|(_, r, t)| (r, t))
            .collect(),
        _ => {
            use sily_core::model::Role;
            let s = claude_store_for(ctx, id)?.load(id)?;
            s.messages
                .iter()
                .filter(|m| matches!(m.role, Role::User | Role::Assistant))
                .map(|m| (role_str(m.role), m.text.clone()))
                .collect()
        }
    };
    Ok(raw
        .into_iter()
        .filter(|(role, text)| {
            let t = text.trim();
            if t.is_empty() {
                return false;
            }
            // Keep all assistant turns; drop user-side noise (env/command wrappers).
            role != "user" || render::is_real_prompt(t)
        })
        .collect())
}

fn role_str(r: sily_core::model::Role) -> String {
    use sily_core::model::Role::*;
    match r {
        User => "user",
        Assistant => "assistant",
        System => "system",
        Other => "other",
    }
    .to_string()
}

/// Build the context message that seeds the new (cross-provider) session.
fn build_context(source: &str, id: &str, transcript: &[(String, String)]) -> String {
    let mut s = format!(
        "[Context ported by sily from a {source} session ({})]\n\n\
         The following is a prior AI coding conversation. Continue the work from where it left off.\n\n\
         --- transcript ---\n",
        &id[..id.len().min(8)]
    );
    let mut total = 0usize;
    for (role, text) in transcript {
        let body: String = if role == "assistant" && text.chars().count() > 800 {
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

/// Interactively ask which provider to port into.
fn choose_provider() -> Result<&'static str, CliError> {
    use std::io::Write;
    if !std::io::stdin().is_terminal() {
        return Err(CliError::Msg(
            "not a terminal — pass --to <claude-code|codex-cli|opencode>".into(),
        ));
    }
    println!("Port to which provider?");
    println!("  1) claude-code");
    println!("  2) codex-cli");
    println!("  3) opencode");
    print!("> ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| CliError::Msg(e.to_string()))?;
    normalize_provider(line.trim())
}

fn normalize_provider(s: &str) -> Result<&'static str, CliError> {
    match s.trim() {
        "1" | "claude" | "claude-code" => Ok("claude-code"),
        "2" | "codex" | "codex-cli" => Ok("codex-cli"),
        "3" | "opencode" => Ok("opencode"),
        other => Err(CliError::Msg(format!(
            "unknown provider '{other}' (use claude-code | codex-cli | opencode)"
        ))),
    }
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

/// Print Codex message points: a numbered list (use the number with `--at`).
fn print_points(points: Vec<(usize, String, String)>, limit: Option<usize>) {
    if points.is_empty() {
        println!("(no messages)");
        return;
    }
    let start = match limit {
        Some(n) if points.len() > n => points.len() - n,
        _ => 0,
    };
    if start > 0 {
        println!("… {start} earlier messages (use --full to see all)");
    }
    for (n, role, snippet) in &points[start..] {
        println!("{n:>4}  {role:<9}  {}", clip(snippet, 80));
    }
}

/// Print OpenCode message points: message id + role + snippet.
fn print_oc_points(points: Vec<(String, String, String)>, limit: Option<usize>) {
    if points.is_empty() {
        println!("(no messages)");
        return;
    }
    let start = match limit {
        Some(n) if points.len() > n => points.len() - n,
        _ => 0,
    };
    if start > 0 {
        println!("… {start} earlier messages (use --full to see all)");
    }
    for (id, role, snippet) in &points[start..] {
        println!("{id}  {role:<9}  {}", clip(snippet, 80));
    }
}

/// Parse an optional Codex branch point ("3" → message #3; None → whole session).
fn parse_index(at: Option<&str>) -> Result<Option<usize>, CliError> {
    match at {
        None => Ok(None),
        Some(s) => s
            .parse::<usize>()
            .map(Some)
            .map_err(|_| CliError::Msg(format!("--at for codex must be a message number, got '{s}'"))),
    }
}

/// Print the result of an OpenCode branch (the new id may be undetectable).
fn print_opencode_branch(b: &sily_adapter_opencode::Branched) {
    match (&b.new_id, &b.resume) {
        (Some(id), Some(resume)) => {
            println!("created opencode session {id}");
            println!("  {} messages", b.kept_messages);
            println!("  resume with:  {resume}");
        }
        _ => {
            println!(
                "opencode import completed ({} messages), but the new session id wasn't detected.",
                b.kept_messages
            );
            println!("  run 'sily list' to find it, then: opencode --session <id>");
        }
    }
}

/// Resolve the "HEAD" of a session: its last message.
fn head_uuid(session: &sily_core::model::Session) -> Result<String, CliError> {
    session
        .messages
        .last()
        .map(|m| m.uuid.clone())
        .ok_or_else(|| CliError::from("session has no messages"))
}

fn run(cli: Cli) -> Result<(), CliError> {
    // `update` needs no session context.
    if let Cmd::Update = cli.cmd {
        return update::run().map_err(CliError::Msg);
    }
    let ctx = build_ctx(cli.cwd)?;
    match cli.cmd {
        Cmd::List { all } => {
            let commits = ctx.commits.all()?;
            let branches = ctx.branches.all()?;
            let mut providers = gather_providers(&ctx);
            if !all {
                providers = scope_to_dir(providers, &ctx.cwd);
            }
            if providers.is_empty() {
                if all {
                    println!("(no sessions found)");
                } else {
                    println!(
                        "(no sessions under {} — use 'sily list --all' to see every project)",
                        ctx.cwd
                    );
                }
            } else if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
                // Interactive when attached to a terminal; static tree when piped.
                if let Some(cmd) = tui::run(&providers, &commits, &branches)
                    .map_err(|e| CliError::Msg(e.to_string()))?
                {
                    println!("{cmd}");
                }
            } else {
                print!("{}", graph::render_all(&providers, &commits, &branches));
            }
        }

        Cmd::Log { session, prompts, full } => {
            let limit = if full { None } else { Some(DEFAULT_LIMIT) };
            match detect_provider(&ctx, &session) {
                "codex-cli" => {
                    let pts = sily_adapter_codex::message_points(&ctx.codex_home, &session)?;
                    if prompts {
                        print_user_prompts(pts.into_iter().map(|(_, r, t)| (r, t)).collect(), limit);
                    } else {
                        print_points(pts, limit);
                    }
                }
                "opencode" => {
                    let pts = sily_adapter_opencode::message_points(&session)?;
                    if prompts {
                        print_user_prompts(pts.into_iter().map(|(_, r, t)| (r, t)).collect(), limit);
                    } else {
                        print_oc_points(pts, limit);
                    }
                }
                _ => {
                    let s = claude_store_for(&ctx, &session)?.load(&session)?;
                    if prompts {
                        print!("{}", render::prompts(&s, limit));
                    } else {
                        print!("{}", render::log(&s, limit));
                    }
                }
            }
        }

        Cmd::Tree { session, full } => {
            let limit = if full { None } else { Some(DEFAULT_LIMIT) };
            match detect_provider(&ctx, &session) {
                // codex/opencode are linear append-logs (no in-file branch tree).
                "codex-cli" => {
                    print_points(sily_adapter_codex::message_points(&ctx.codex_home, &session)?, limit)
                }
                "opencode" => print_oc_points(sily_adapter_opencode::message_points(&session)?, limit),
                _ => {
                    let s = claude_store_for(&ctx, &session)?.load(&session)?;
                    print!("{}", render::tree(&s, limit));
                }
            }
        }

        Cmd::Commit {
            session,
            name,
            message,
            at,
        } => {
            // Claude validates the message against the loaded session; for codex
            // (index) / opencode (message id) the point is stored as given
            // (empty = HEAD / whole session).
            let msg_uuid = match detect_provider(&ctx, &session) {
                "claude-code" => {
                    let s = claude_store_for(&ctx, &session)?.load(&session)?;
                    match at {
                        Some(u) => {
                            if s.message(&u).is_none() {
                                return Err(CliError::Msg(format!(
                                    "message {u} not found in session"
                                )));
                            }
                            u
                        }
                        None => head_uuid(&s)?,
                    }
                }
                _ => at.unwrap_or_default(),
            };
            let existing = ctx.commits.all()?;
            let name = name.unwrap_or_else(|| format!("c{}", existing.len() + 1));
            let commit = Commit {
                name: name.clone(),
                session_id: session,
                message_uuid: msg_uuid.clone(),
                created_at: now_iso(),
                note: message,
            };
            ctx.commits.add(commit)?;
            let at_label = if msg_uuid.is_empty() {
                "HEAD".to_string()
            } else {
                msg_uuid.chars().take(8).collect()
            };
            println!("committed '{name}' at {at_label}");
        }

        Cmd::Commits => {
            let all = ctx.commits.all()?;
            if all.is_empty() {
                println!("(no commits yet)");
            }
            for c in all {
                let note = c.note.as_deref().unwrap_or("");
                println!(
                    "{:<12} {} @ {}  {}",
                    c.name,
                    &c.session_id[..c.session_id.len().min(8)],
                    &c.message_uuid[..c.message_uuid.len().min(8)],
                    note
                );
            }
        }

        Cmd::Branch { session, at } => match detect_provider(&ctx, &session) {
            "codex-cli" => {
                let at_idx = parse_index(at.as_deref())?;
                let b = sily_adapter_codex::branch(&ctx.codex_home, &session, at_idx)?;
                println!("created codex session {}", b.new_id);
                println!("  {} messages", b.kept_messages);
                println!("  resume with:  {}", b.resume);
            }
            "opencode" => {
                let b = sily_adapter_opencode::branch(&session, at.as_deref())?;
                print_opencode_branch(&b);
            }
            _ => {
                let store = claude_store_for(&ctx, &session)?;
                let s = store.load(&session)?;
                let at = match at {
                    Some(u) => u,
                    None => head_uuid(&s)?,
                };
                let new_id = new_session_id();
                let branched = branch_at(&s, &at, new_id.clone())?;
                store.save(&branched)?;
                ctx.branches.add(BranchRecord {
                    session_id: new_id.clone(),
                    from_session: session,
                    at_message: at.clone(),
                    origin: "branch".to_string(),
                    created_at: now_iso(),
                })?;
                println!("created session {new_id}");
                println!("  {} messages, branched at {}", branched.messages.len(), &at[..at.len().min(8)]);
                println!("  resume with:  claude --resume {new_id}");
            }
        },

        Cmd::Revert { commit, hard } => {
            let c = ctx
                .commits
                .find(&commit)?
                .ok_or_else(|| format!("no such commit: {commit}"))?;
            let point = if c.message_uuid.is_empty() { None } else { Some(c.message_uuid.as_str()) };
            match detect_provider(&ctx, &c.session_id) {
                "codex-cli" => {
                    let at_idx = parse_index(point)?;
                    if hard {
                        let idx = at_idx.ok_or("commit has no point for --hard")?;
                        let kept = sily_adapter_codex::truncate(&ctx.codex_home, &c.session_id, idx)?;
                        println!("hard-reset codex session to commit '{}' ({kept} messages kept)", c.name);
                    } else {
                        let b = sily_adapter_codex::branch(&ctx.codex_home, &c.session_id, at_idx)?;
                        println!("reverted commit '{}' into codex session {}", c.name, b.new_id);
                        println!("  {} messages restored", b.kept_messages);
                        println!("  resume with:  {}", b.resume);
                    }
                }
                "opencode" => {
                    if hard {
                        return Err(CliError::Msg(
                            "opencode --hard revert is not supported (branch is non-destructive)".into(),
                        ));
                    }
                    let b = sily_adapter_opencode::branch(&c.session_id, point)?;
                    println!("reverted commit '{}' via opencode import", c.name);
                    print_opencode_branch(&b);
                }
                _ => {
                    let store = claude_store_for(&ctx, &c.session_id)?;
                    let s = store.load(&c.session_id)?;
                    if hard {
                        let reset = truncate_at(&s, &c.message_uuid)?;
                        store.save(&reset)?;
                        println!(
                            "hard-reset session {} to commit '{}' ({} messages kept)",
                            &c.session_id[..c.session_id.len().min(8)],
                            c.name,
                            reset.messages.len()
                        );
                    } else {
                        let new_id = new_session_id();
                        let forked = branch_at(&s, &c.message_uuid, new_id.clone())?;
                        store.save(&forked)?;
                        ctx.branches.add(BranchRecord {
                            session_id: new_id.clone(),
                            from_session: c.session_id.clone(),
                            at_message: c.message_uuid.clone(),
                            origin: c.name.clone(),
                            created_at: now_iso(),
                        })?;
                        println!("reverted commit '{}' into new session {new_id}", c.name);
                        println!("  {} messages restored", forked.messages.len());
                        println!("  resume with:  claude --resume {new_id}");
                    }
                }
            }
        }

        Cmd::Diff { a, b } => {
            let sa = claude_store_for(&ctx, &a)?.load(&a)?;
            let sb = claude_store_for(&ctx, &b)?.load(&b)?;
            let ha = head_uuid(&sa)?;
            let hb = head_uuid(&sb)?;
            let d = diff(&sa, &ha, &sb, &hb)?;
            match &d.common_ancestor {
                Some(anc) => println!(
                    "common ancestor: {} ({} shared messages)",
                    &anc[..anc.len().min(8)],
                    d.common_len
                ),
                None => println!("no common ancestor"),
            }
            println!("only in {}: {} messages", &a[..a.len().min(8)], d.only_a.len());
            println!("only in {}: {} messages", &b[..b.len().min(8)], d.only_b.len());
        }

        Cmd::Port { session, to } => {
            let transcript = read_transcript(&ctx, &session)?;
            if transcript.is_empty() {
                return Err(CliError::Msg("no content to port from that session".into()));
            }
            let source = detect_provider(&ctx, &session);
            let context = build_context(source, &session, &transcript);
            let target = match to {
                Some(t) => normalize_provider(&t)?,
                None => choose_provider()?,
            };
            if target == source {
                eprintln!("sily: note — porting into the same provider ({source}); use 'branch' for same-tool forks.");
            }
            match target {
                "codex-cli" => {
                    let (id, resume) =
                        sily_adapter_codex::create_session(&ctx.codex_home, &ctx.cwd, &context)?;
                    println!("ported {} messages → codex session {id}", transcript.len());
                    println!("  resume with:  {resume}");
                }
                "opencode" => {
                    let b = sily_adapter_opencode::create_session(&ctx.cwd, &context)?;
                    println!("ported {} messages → opencode (experimental, verify):", transcript.len());
                    print_opencode_branch(&b);
                }
                _ => {
                    let new_id = new_session_id();
                    let mut s = sily_core::model::Session::new(&new_id);
                    s.meta.cwd = Some(ctx.cwd.clone());
                    s.headers = vec![
                        serde_json::json!({"type":"mode","mode":"normal","sessionId":new_id}),
                        serde_json::json!({"type":"permission-mode","permissionMode":"default","sessionId":new_id}),
                    ];
                    s.messages.push(sily_core::model::Message::new(
                        new_session_id(),
                        None,
                        sily_core::model::Role::User,
                        context,
                    ));
                    ClaudeStore::new(&ctx.claude_home, ctx.cwd.clone()).save(&s)?;
                    println!("ported {} messages → claude-code session {new_id}", transcript.len());
                    println!("  resume with:  claude --resume {new_id}");
                }
            }
        }

        // Handled before ctx is built.
        Cmd::Update => unreachable!(),
    }
    Ok(())
}
