//! Persistence for sily's "commits" — named pointers into a session's DAG.
//!
//! Stored as a single JSON array at `~/.sily/commits.json`. A commit records
//! *where* a good point is (`session_id` + `message_uuid`), never a copy of the
//! conversation — so commits are tiny regardless of session size.

use std::fs;
use std::path::{Path, PathBuf};

use sily_core::error::{Error, Result};
use sily_core::model::Commit;

pub struct CommitStore {
    path: PathBuf,
}

impl CommitStore {
    /// `sily_home` is typically `~/.sily`.
    pub fn new(sily_home: impl AsRef<Path>) -> Self {
        Self {
            path: sily_home.as_ref().join("commits.json"),
        }
    }

    pub fn all(&self) -> Result<Vec<Commit>> {
        match fs::read_to_string(&self.path) {
            Ok(s) if s.trim().is_empty() => Ok(Vec::new()),
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub fn find(&self, name: &str) -> Result<Option<Commit>> {
        Ok(self.all()?.into_iter().find(|c| c.name == name))
    }

    /// Append a commit, persisting the whole list. Rejects duplicate names.
    pub fn add(&self, commit: Commit) -> Result<()> {
        let mut all = self.all()?;
        if all.iter().any(|c| c.name == commit.name) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("commit name already exists: {}", commit.name),
            )));
        }
        all.push(commit);
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(&all)?)?;
        Ok(())
    }
}
