//! [`Provider`] implementation for OpenCode.

use std::path::PathBuf;

use sily_core::error::{Error, Result};
use sily_core::model::Role;
use sily_core::provider::{MsgPoint, NewSession, Provider};
use sily_core::store::ProjectSessions;

use crate::{branch, create_session, list_all_projects, merge, message_points, Branched};

pub struct OpenCodeProvider {
    db_path: PathBuf,
}

impl OpenCodeProvider {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self { db_path: db_path.into() }
    }
}

fn into_new(b: Branched) -> Result<NewSession> {
    match (b.new_id, b.resume) {
        (Some(id), Some(resume)) => Ok(NewSession { id, resume, messages: b.kept_messages }),
        _ => Err(Error::Unsupported(
            "opencode import succeeded but the new session id wasn't detected".into(),
        )),
    }
}

impl Provider for OpenCodeProvider {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn owns(&self, id: &str) -> bool {
        id.starts_with("ses_")
    }

    fn list_projects(&self) -> Result<Vec<ProjectSessions>> {
        list_all_projects(&self.db_path)
    }

    fn messages(&self, id: &str) -> Result<Vec<MsgPoint>> {
        Ok(message_points(id)?
            .into_iter()
            .map(|(mid, role, text)| MsgPoint { point: mid, role: Role::from(role.as_str()), text })
            .collect())
    }

    fn resume_command(&self, id: &str) -> String {
        format!("opencode --session {id}")
    }

    fn branch(&self, id: &str, at: Option<&str>) -> Result<NewSession> {
        into_new(branch(id, at)?)
    }

    fn create_session(&self, cwd: &str, first_user_text: &str) -> Result<NewSession> {
        into_new(create_session(cwd, first_user_text)?)
    }

    fn merge(&self, main_id: &str, branch_id: &str) -> Result<NewSession> {
        into_new(merge(main_id, branch_id)?)
    }
}
