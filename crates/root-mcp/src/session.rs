//! MCP server state: one workspace store plus the active agent session.

use crate::policy::Policy;
use anyhow::Result;
use root_work::{Repository, WorkStore};
use std::path::PathBuf;

pub struct ServerState {
    pub(crate) store: WorkStore,
    pub(crate) root_dir: PathBuf,
    pub(crate) repository: Repository,
    pub(crate) policy: Policy,
    pub(crate) session_id: Option<String>,
    pub(crate) harness: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) initialized: bool,
}

impl ServerState {
    pub fn new(
        store: WorkStore,
        root_dir: PathBuf,
        repository: Repository,
        policy: Policy,
    ) -> Self {
        Self {
            store,
            root_dir,
            repository,
            policy,
            session_id: None,
            harness: None,
            agent: None,
            initialized: false,
        }
    }

    pub fn store(&self) -> &WorkStore {
        &self.store
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    pub fn harness(&self) -> Option<&str> {
        self.harness.as_deref()
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Associate an agent session with this connection. Re-initialization keeps
    /// the original session rather than duplicating it.
    pub fn initialize(&mut self, harness: String, agent: String) -> Result<()> {
        self.harness = Some(harness);
        self.agent = Some(agent);
        if self.session_id.is_none() {
            let harness = self.harness.clone();
            let agent = self.agent.clone();
            let session = self
                .store
                .start_session(harness.as_deref(), agent.as_deref())?;
            self.session_id = Some(session.id);
        }
        self.initialized = true;
        Ok(())
    }
}
