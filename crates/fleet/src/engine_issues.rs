//! Clear and bring back attention findings. Local state only: nothing on a machine changes.

use super::{Engine, EngineError};
use crate::issues::cleared::ClearStore;
use crate::sync::MutexExt;

impl Engine {
    fn clear_store(&self) -> &std::sync::Mutex<ClearStore> {
        if self.config().demo_mode {
            &self.demo_clears
        } else {
            &self.clears
        }
    }

    /// Hide a finding while the same problem lasts. Performs local disk I/O; call off the async runtime.
    pub fn clear_issue(&self, id: &str) -> Result<(), EngineError> {
        let view = self.view();
        let issue = view
            .issues
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| EngineError::NotFound("This item is no longer listed. Refresh and try again.".into()))?;
        self.clear_store().locked().clear(issue).map_err(EngineError::Storage)?;
        self.view_changed();
        Ok(())
    }

    /// Show a cleared finding again. Performs local disk I/O; call off the async runtime.
    pub fn restore_issue(&self, id: &str) -> Result<(), EngineError> {
        self.clear_store().locked().restore(id).map_err(EngineError::Storage)?;
        self.view_changed();
        Ok(())
    }

    /// Drop clears whose problem changed or went away. Performs local disk I/O; call off the async runtime.
    pub fn record_cleared(&self) {
        if self.config().demo_mode {
            return;
        }
        let view = self.view();
        if let Err(e) = self.clears.locked().record(&view) {
            tracing::warn!("saving cleared items failed: {e}");
        }
    }
}
