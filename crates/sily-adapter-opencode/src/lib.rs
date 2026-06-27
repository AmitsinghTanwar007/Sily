//! # sily-adapter-opencode
//!
//! Read-only listing of OpenCode sessions. OpenCode stores everything in a
//! SQLite database (default `~/.local/share/opencode/opencode.db`). The `session`
//! table holds one row per session: `id`, `directory` (the cwd), `title` (a human
//! summary), and `time_updated` (epoch ms). Message counts come from the
//! `message` table.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

use sily_core::error::{Error, Result};
use sily_core::model::SessionMeta;
use sily_core::store::{ProjectSessions, SessionRef};

pub const PROVIDER: &str = "opencode";

fn io_err(e: impl std::fmt::Display) -> Error {
    Error::Io(std::io::Error::other(e.to_string()))
}

/// Enumerate every OpenCode session in the database, grouped by directory (cwd).
pub fn list_all_projects(db_path: &Path) -> Result<Vec<ProjectSessions>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    // Read-only; don't create or modify the DB.
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io_err)?;

    // message counts per session in one pass
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id")
            .map_err(io_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(io_err)?;
        for row in rows {
            let (sid, n) = row.map_err(io_err)?;
            counts.insert(sid, n as usize);
        }
    }

    let mut by_cwd: std::collections::BTreeMap<String, Vec<SessionRef>> = std::collections::BTreeMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, directory, title, time_updated FROM session")
            .map_err(io_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (id, directory, title, time_updated) = row.map_err(io_err)?;
            if directory.is_empty() {
                continue;
            }
            let modified = time_updated
                .filter(|&t| t > 0)
                .map(|t| UNIX_EPOCH + Duration::from_millis(t as u64));
            let message_count = counts.get(&id).copied().unwrap_or(0);
            by_cwd.entry(directory.clone()).or_default().push(SessionRef {
                id,
                summary: title.chars().take(80).collect(),
                message_count,
                modified,
                meta: SessionMeta {
                    cwd: Some(directory),
                    provider: Some(PROVIDER.to_string()),
                },
            });
        }
    }

    // newest first within each project
    Ok(by_cwd
        .into_iter()
        .map(|(cwd, mut sessions)| {
            sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
            ProjectSessions { cwd, sessions }
        })
        .collect())
}

/// Default OpenCode database path: `~/.local/share/opencode/opencode.db`.
pub fn default_db_path(home: &Path) -> std::path::PathBuf {
    home.join(".local/share/opencode/opencode.db")
}
