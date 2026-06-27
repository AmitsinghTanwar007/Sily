//! Persistence for branch provenance — which session came from which, and where.
//! Stored as a JSON array at `~/.sily/branches.json`. Powers the graph view.

use std::fs;
use std::path::{Path, PathBuf};

use sily_core::error::{Error, Result};
use sily_core::model::BranchRecord;

pub struct BranchStore {
    path: PathBuf,
}

impl BranchStore {
    pub fn new(sily_home: impl AsRef<Path>) -> Self {
        Self {
            path: sily_home.as_ref().join("branches.json"),
        }
    }

    pub fn all(&self) -> Result<Vec<BranchRecord>> {
        match fs::read_to_string(&self.path) {
            Ok(s) if s.trim().is_empty() => Ok(Vec::new()),
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub fn add(&self, record: BranchRecord) -> Result<()> {
        let mut all = self.all()?;
        all.push(record);
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(&all)?)?;
        Ok(())
    }
}
