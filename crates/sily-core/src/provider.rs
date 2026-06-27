//! The unified provider interface. Every adapter implements [`Provider`], so the
//! CLI can treat Claude Code, Codex, OpenCode, Gemini, … identically: detect
//! ownership, list, read messages, branch/revert, and port — all through one
//! trait. Adding a new tool = one new `impl Provider`.
//!
//! Capabilities that not every tool supports (branch, hard revert, create,
//! structured tree) have default impls that return [`Error::Unsupported`], so a
//! minimal adapter only needs `name`/`owns`/`list_projects`/`messages`/`resume`.

use crate::error::{Error, Result};
use crate::model::{Role, Session};
use crate::store::ProjectSessions;

/// One message plus the opaque identifier you'd branch/commit at. The meaning of
/// `point` is provider-specific (Claude: message uuid, Codex: 1-based index,
/// OpenCode/Gemini: message id) — the CLI treats it as an opaque token.
#[derive(Clone)]
pub struct MsgPoint {
    pub point: String,
    pub role: Role,
    pub text: String,
    /// Sortable timestamp (ISO-8601, or zero-padded epoch) — used to interleave
    /// lanes in the graph. Empty if the provider doesn't supply one.
    pub time: String,
}

/// Result of creating or branching a session.
pub struct NewSession {
    pub id: String,
    pub resume: String,
    pub messages: usize,
}

pub trait Provider {
    /// Stable provider name shown in the tree (e.g. "claude-code").
    fn name(&self) -> &'static str;

    /// Does this provider own the given session id?
    fn owns(&self, id: &str) -> bool;

    /// Sessions grouped by project (cwd).
    fn list_projects(&self) -> Result<Vec<ProjectSessions>>;

    /// Ordered messages with branch-point ids.
    fn messages(&self, id: &str) -> Result<Vec<MsgPoint>>;

    /// The command a user runs to resume this session in its tool.
    fn resume_command(&self, id: &str) -> String;

    // ---- optional capabilities (default: unsupported) ----

    /// Branch from `at` (None = whole/HEAD) into a new session.
    fn branch(&self, _id: &str, _at: Option<&str>) -> Result<NewSession> {
        Err(Error::Unsupported(format!("{}: branch", self.name())))
    }

    /// Destructive in-place truncate to `at`.
    fn truncate(&self, _id: &str, _at: &str) -> Result<usize> {
        Err(Error::Unsupported(format!("{}: hard revert", self.name())))
    }

    /// Create a fresh session seeded with one user message (port target).
    fn create_session(&self, _cwd: &str, _first_user_text: &str) -> Result<NewSession> {
        Err(Error::Unsupported(format!("{}: port target", self.name())))
    }

    /// Canonical session with parent structure, if the tool has one (Claude).
    /// Used by `tree`; others fall back to the linear `messages` view.
    fn structured(&self, _id: &str) -> Result<Option<Session>> {
        Ok(None)
    }

    /// Merge `branch` into `main`: produce a NEW session = `main`'s full history
    /// followed by `branch`'s work after their **common ancestor** (the shared
    /// prefix of the two histories). This works for branch→main *and*
    /// branch→branch (two forks off the same point combine onto the shared base).
    /// It is a replay/concatenation, not a semantic 3-way merge.
    fn merge(&self, _main_id: &str, _branch_id: &str) -> Result<NewSession> {
        Err(Error::Unsupported(format!("{}: merge", self.name())))
    }
}
