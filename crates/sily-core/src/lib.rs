//! # sily-core
//!
//! Provider-agnostic core for **sily** — a git-like commit / branch / revert
//! system for AI sessions.
//!
//! This crate holds the canonical conversation model and the *pure* operations
//! over it. It performs no I/O and knows nothing about Claude Code or any other
//! provider. Backends plug in by implementing [`store::SessionStore`].
//!
//! Layers:
//! - [`model`] — `Session`, `Message`, `Role`, `Commit`.
//! - [`ops`]   — `lineage`, `branch_at`, `truncate_at`, `diff` (the branch engine).
//! - [`store`] — the `SessionStore` port adapters implement.
//! - [`error`] — `Error` / `Result`.

pub mod error;
pub mod model;
pub mod ops;
pub mod store;

pub use error::{Error, Result};
pub use model::{BranchRecord, Commit, Message, Role, Session, SessionMeta};
pub use ops::{branch_at, diff, index_of, lineage, prefix_until, truncate_at, Divergence};
pub use store::{SessionRef, SessionStore};
