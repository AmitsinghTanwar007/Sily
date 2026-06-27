//! The port (hexagonal boundary). Adapters implement [`SessionStore`] to bridge
//! the canonical model to a concrete backend (Claude Code files, etc.). The core
//! and CLI depend only on this trait, never on a specific provider.

use crate::error::Result;
use crate::model::{Session, SessionMeta};

/// Lightweight listing entry — enough to show a picker without loading full bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRef {
    pub id: String,
    /// Short human summary (e.g. first user message).
    pub summary: String,
    pub meta: SessionMeta,
}

/// A provider backend that can read, write, and enumerate sessions.
pub trait SessionStore {
    /// Load a full session by id into the canonical model.
    fn load(&self, id: &str) -> Result<Session>;

    /// Persist a session in the provider's native format. Adapters are
    /// responsible for any provider-specific fixups (e.g. rewriting embedded
    /// session ids to match `session.id`).
    fn save(&self, session: &Session) -> Result<()>;

    /// Enumerate available sessions.
    fn list(&self) -> Result<Vec<SessionRef>>;
}
