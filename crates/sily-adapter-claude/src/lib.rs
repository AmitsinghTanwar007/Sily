//! # sily-adapter-claude
//!
//! [`SessionStore`](sily_core::store::SessionStore) implementation for Claude Code.
//! A Claude session is a single `.jsonl` file at
//! `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, where `<encoded-cwd>`
//! is the working directory with every non-alphanumeric character replaced by `-`
//! (so `/home/amitsinghtanwar` → `-home-amitsinghtanwar`), and the filename UUID
//! must equal the `sessionId` inside the file.
//!
//! Each line is a JSON record. `user`/`assistant` records are messages; everything
//! else (`mode`, `permission-mode`, `file-history-snapshot`, `summary`, …) is kept
//! verbatim as a header so we can round-trip faithfully. On save we rewrite every
//! embedded `sessionId` to match the (possibly new) session id — this is what makes
//! a branched/reverted session a valid, resumable Claude session.
//!
//! Modules:
//! - [`encode`]  — working directory → project-folder name.
//! - [`convert`] — Claude record ⇄ canonical [`Message`](sily_core::model::Message).
//! - [`store`]   — [`ClaudeStore`], the filesystem-facing `SessionStore`.

mod convert;
mod encode;
mod store;

pub use convert::PROVIDER;
pub use encode::encode_cwd;
pub use store::ClaudeStore;
