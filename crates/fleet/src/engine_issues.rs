//! Clear and bring back attention findings. Local state only: nothing on a machine changes.

use std::sync::Mutex;

use super::{Engine, EngineError};
use crate::issues::cleared::ClearStore;
use crate::model::AppConfig;
use crate::sync::MutexExt;

impl Engine {
    pub(super) fn clear_store(&self, config: &AppConfig) -> &Mutex<ClearStore> {
        if config.demo_mode {
            &self.demo_clears
        } else {
            &self.clears
        }
    }

    /// Hide a finding while the problem the user saw, `fingerprint`, lasts.
    /// Performs local disk I/O; call off the async runtime.
    pub fn clear_issue(&self, id: &str, fingerprint: &str) -> Result<(), EngineError> {
        let config = self.config();
        let mut clears = self.clear_store(&config).locked();
        let view = self.view_with(config, &clears);
        let issue = view
            .issues
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| EngineError::NotFound("This item is no longer listed. Refresh and try again.".into()))?;
        if issue.fingerprint != fingerprint {
            return Err(EngineError::Invalid(
                "This item changed since it was shown. Look at it again, then clear it.".into(),
            ));
        }
        clears.clear(issue).map_err(EngineError::Storage)?;
        drop(clears);
        self.view_changed();
        Ok(())
    }

    /// Show a cleared finding again. Performs local disk I/O; call off the async runtime.
    pub fn restore_issue(&self, id: &str) -> Result<(), EngineError> {
        let config = self.config();
        self.clear_store(&config)
            .locked()
            .restore(id)
            .map_err(EngineError::Storage)?;
        self.view_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::engine::{Engine, MemorySecrets};

    #[test]
    fn a_recovery_a_view_showed_brings_the_problem_back_before_the_next_poll() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::load(dir.path().join("config.json"), Arc::new(MemorySecrets::default())).unwrap();
        let broken = dir.path().join("broken.toml").display().to_string();
        let valid = dir.path().join("fleet.toml");
        std::fs::write(&valid, include_str!("../../../inventory/fleet.example.toml")).unwrap();
        let inventory = |engine: &Engine| engine.view().issues.into_iter().find(|i| i.id == "inventory");

        engine.set_inventory_path(Some(broken.clone())).unwrap();
        let shown = inventory(&engine).unwrap();
        engine.clear_issue(&shown.id, &shown.fingerprint).unwrap();
        assert!(inventory(&engine).unwrap().cleared);

        engine.set_inventory_path(Some(valid.display().to_string())).unwrap();
        assert!(inventory(&engine).is_none());
        engine.set_inventory_path(Some(broken)).unwrap();
        let back = inventory(&engine).unwrap();
        assert_eq!(back.fingerprint, shown.fingerprint);
        assert!(!back.cleared);
    }
}
