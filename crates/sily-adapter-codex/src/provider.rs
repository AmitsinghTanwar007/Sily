//! [`Provider`] implementation for Codex CLI.

use std::path::PathBuf;

use sily_core::error::{Error, Result};
use sily_core::model::Role;
use sily_core::provider::{MsgPoint, NewSession, Provider};
use sily_core::store::ProjectSessions;

use crate::{branch, create_session, find_session_file, list_all_projects, message_points, truncate};

pub struct CodexProvider {
    home: PathBuf,
}

impl CodexProvider {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }
}

fn parse_index(at: Option<&str>) -> Result<Option<usize>> {
    match at {
        None => Ok(None),
        Some(s) => s
            .parse::<usize>()
            .map(Some)
            .map_err(|_| Error::Unsupported(format!("codex branch point must be a number, got '{s}'"))),
    }
}

fn role_of(s: &str) -> Role {
    Role::from(s)
}

impl Provider for CodexProvider {
    fn name(&self) -> &'static str {
        "codex-cli"
    }

    fn owns(&self, id: &str) -> bool {
        find_session_file(&self.home, id).is_some()
    }

    fn list_projects(&self) -> Result<Vec<ProjectSessions>> {
        list_all_projects(&self.home)
    }

    fn messages(&self, id: &str) -> Result<Vec<MsgPoint>> {
        Ok(message_points(&self.home, id)?
            .into_iter()
            .map(|(idx, role, text)| MsgPoint { point: idx.to_string(), role: role_of(&role), text })
            .collect())
    }

    fn resume_command(&self, id: &str) -> String {
        format!("codex resume {id}")
    }

    fn branch(&self, id: &str, at: Option<&str>) -> Result<NewSession> {
        let b = branch(&self.home, id, parse_index(at)?)?;
        Ok(NewSession { id: b.new_id, resume: b.resume, messages: b.kept_messages })
    }

    fn truncate(&self, id: &str, at: &str) -> Result<usize> {
        let idx = parse_index(Some(at))?.unwrap_or(0);
        truncate(&self.home, id, idx)
    }

    fn create_session(&self, cwd: &str, first_user_text: &str) -> Result<NewSession> {
        let (id, resume) = create_session(&self.home, cwd, first_user_text)?;
        Ok(NewSession { id, resume, messages: 1 })
    }
}
