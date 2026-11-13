//! runner-up central pools: one read-only SSH read of the coordinator per poll.

use std::sync::Arc;

use chrono::Utc;

use super::Engine;
use crate::desired::model::Inventory;
use crate::model::{PoolsObservation, Timed};
use crate::runner_up::{self, PoolSource};
use crate::sync::RwLockExt;

impl Engine {
    /// Replace the coordinator read with a deterministic boundary in tests.
    pub fn with_pool_source(mut self, source: Arc<dyn PoolSource>) -> Self {
        self.pool_source = source;
        self
    }

    /// Reads runner-up's status, config and tart VMs from the coordinator host the
    /// inventory names. A failed read keeps the last good one beside the error.
    pub async fn poll_pools(&self) {
        if self.config().demo_mode {
            return;
        }
        let Ok(inventory) = self.read_inventory().await else {
            return;
        };
        let Some(runner_up) = inventory.runner_up.clone() else {
            self.obs.write_locked().pools = None;
            return;
        };
        let source = self.pool_source.clone();
        let read = runner_up.clone();
        let result = tokio::task::spawn_blocking(move || read_coordinator(&inventory, source.as_ref()))
            .await
            .map_err(|e| format!("The runner-up read stopped unexpectedly: {e}"))
            .and_then(|r| r);
        let now = Utc::now();
        {
            let mut obs = self.obs.write_locked();
            let entry = obs.pools.get_or_insert_with(|| PoolsObservation {
                runner_up: read.clone(),
                report: None,
                error: None,
            });
            if entry.runner_up != read {
                *entry = PoolsObservation {
                    runner_up: read,
                    report: None,
                    error: None,
                };
            }
            match result {
                Ok(report) => {
                    entry.report = Some(Timed::new(now, report));
                    entry.error = None;
                }
                Err(e) => entry.error = Some(Timed::new(now, e)),
            }
        }
        self.view_changed();
    }
}

/// The coordinator read, and tart on another Mac when the inventory says so.
fn read_coordinator(inventory: &Inventory, source: &dyn PoolSource) -> Result<runner_up::PoolsReport, String> {
    let Some(config) = &inventory.runner_up else {
        return Err("The inventory has no [runner_up] section.".into());
    };
    let host = inventory.find_host(&config.host).map_err(|e| e.0)?;
    let tart_host = match &config.tart_host {
        Some(id) if id != &config.host => Some(inventory.find_host(id).map_err(|e| e.0)?),
        _ => None,
    };
    let with_tart = tart_host.is_none() && host.platform == crate::desired::model::Platform::DarwinArm64;
    let output = source
        .read(host, &runner_up::read_script(config, with_tart))
        .map_err(|e| e.to_string())?;
    let tart = match tart_host {
        // A tart read that fails leaves the VM list unknown, not the pools.
        Some(mac) => source.read(mac, &runner_up::tart_script()).ok(),
        None => None,
    };
    runner_up::parse_read(&output, tart.as_deref())
}
