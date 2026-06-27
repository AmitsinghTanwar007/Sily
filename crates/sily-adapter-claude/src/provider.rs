//! [`Provider`] implementation for Claude Code.

use std::path::PathBuf;

use sily_core::error::{Error, Result};
use sily_core::model::{Message, Role, Session};
use sily_core::provider::{MsgPoint, NewSession, Provider};
use sily_core::store::{ProjectSessions, SessionStore};
use sily_core::{branch_at, truncate_at};

use crate::store::{list_all_projects, locate, ClaudeStore};

pub struct ClaudeProvider {
    home: PathBuf,
}

impl ClaudeProvider {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn store_for(&self, id: &str) -> Result<ClaudeStore> {
        locate(&self.home, id)
            .map(|(dir, cwd)| ClaudeStore::from_project_dir(dir, cwd))
            .ok_or_else(|| Error::SessionNotFound(id.to_string()))
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Provider for ClaudeProvider {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn owns(&self, id: &str) -> bool {
        locate(&self.home, id).is_some()
    }

    fn list_projects(&self) -> Result<Vec<ProjectSessions>> {
        list_all_projects(&self.home)
    }

    fn messages(&self, id: &str) -> Result<Vec<MsgPoint>> {
        let s = self.store_for(id)?.load(id)?;
        Ok(s.messages
            .into_iter()
            .map(|m| MsgPoint { point: m.uuid, role: m.role, text: m.text })
            .collect())
    }

    fn resume_command(&self, id: &str) -> String {
        format!("claude --resume {id}")
    }

    fn branch(&self, id: &str, at: Option<&str>) -> Result<NewSession> {
        let store = self.store_for(id)?;
        let s = store.load(id)?;
        let at_uuid = match at {
            Some(a) => a.to_string(),
            None => s
                .messages
                .last()
                .map(|m| m.uuid.clone())
                .ok_or_else(|| Error::MessageNotFound("HEAD".into()))?,
        };
        let id2 = new_id();
        let branched = branch_at(&s, &at_uuid, id2.clone())?;
        store.save(&branched)?;
        Ok(NewSession {
            id: id2.clone(),
            resume: format!("claude --resume {id2}"),
            messages: branched.messages.len(),
        })
    }

    fn truncate(&self, id: &str, at: &str) -> Result<usize> {
        let store = self.store_for(id)?;
        let s = store.load(id)?;
        let reset = truncate_at(&s, at)?;
        let n = reset.messages.len();
        store.save(&reset)?;
        Ok(n)
    }

    fn create_session(&self, cwd: &str, first_user_text: &str) -> Result<NewSession> {
        let id2 = new_id();
        let mut s = Session::new(&id2);
        s.meta.cwd = Some(cwd.to_string());
        s.headers = vec![
            serde_json::json!({"type":"mode","mode":"normal","sessionId":id2}),
            serde_json::json!({"type":"permission-mode","permissionMode":"default","sessionId":id2}),
        ];
        s.messages
            .push(Message::new(new_id(), None, Role::User, first_user_text));
        ClaudeStore::new(&self.home, cwd.to_string()).save(&s)?;
        Ok(NewSession {
            id: id2.clone(),
            resume: format!("claude --resume {id2}"),
            messages: 1,
        })
    }

    fn structured(&self, id: &str) -> Result<Option<Session>> {
        Ok(Some(self.store_for(id)?.load(id)?))
    }
}
