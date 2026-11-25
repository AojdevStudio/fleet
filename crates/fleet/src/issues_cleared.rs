//! Clear: which findings the user has hidden, saved next to `config.json`.
//!
//! The rules:
//! - Clear saves the finding's ID and fingerprint (see [`super::rules`]). The finding
//!   stays hidden while a finding with that ID has that fingerprint.
//! - A new fingerprint brings it back, and the saved clear is dropped.
//! - A finding that goes away and returns comes back. "Gone" needs evidence: the app
//!   showed the finding since launch, and for a machine finding other than an alert,
//!   the machine has a fresh SSH read. So a restart, a stale read or an offline spell
//!   doesn't count as the problem going away. Alerts handle missing data themselves.
//! - Bring back drops the saved clear at once.
//!
//! Not covered: a problem fixed while the app is closed, that returns with the same
//! fingerprint before the app sees it gone, stays cleared.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{FleetIssue, FleetIssueCategory};
use crate::model::{FleetView, MachineConnection};

const VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    /// Issue ID to the fingerprint it had when cleared.
    cleared: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct ClearStore {
    /// `None` keeps clears in memory only (demo data).
    path: Option<PathBuf>,
    cleared: BTreeMap<String, String>,
    /// Cleared findings shown since launch, with the machine whose fresh read
    /// confirms their absence (`None` when absence needs no machine read).
    shown: BTreeMap<String, Option<String>>,
    /// Set when the saved file couldn't be read. It is kept, and nothing is saved over it.
    load_error: Option<String>,
}

impl ClearStore {
    pub fn load(path: PathBuf) -> Self {
        let result = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Saved>(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    if s.version == VERSION {
                        Ok(s.cleared)
                    } else {
                        Err(format!("Unsupported version {}.", s.version))
                    }
                }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.to_string()),
        };
        let (cleared, load_error) = match result {
            Ok(cleared) => (cleared, None),
            Err(e) => {
                tracing::warn!("couldn't read cleared items at {}: {e}", path.display());
                (
                    BTreeMap::new(),
                    Some(format!(
                        "the saved cleared items at {} couldn't be read ({e}). The file was kept.",
                        path.display()
                    )),
                )
            }
        };
        Self {
            path: Some(path),
            cleared,
            shown: BTreeMap::new(),
            load_error,
        }
    }

    pub fn in_memory(cleared: BTreeMap<String, String>) -> Self {
        Self {
            path: None,
            cleared,
            shown: BTreeMap::new(),
            load_error: None,
        }
    }

    pub fn cleared(&self) -> &BTreeMap<String, String> {
        &self.cleared
    }

    pub fn clear(&mut self, issue: &FleetIssue) -> Result<(), String> {
        self.writable()?;
        self.cleared.insert(issue.id.clone(), issue.fingerprint.clone());
        self.shown.insert(issue.id.clone(), absence_machine(issue));
        self.save()
    }

    pub fn restore(&mut self, id: &str) -> Result<(), String> {
        self.writable()?;
        if self.cleared.remove(id).is_some() {
            self.save()?;
        }
        Ok(())
    }

    /// Drop clears that no longer apply: a changed fingerprint, or a finding that
    /// went away. Saves only when something was dropped.
    pub fn record(&mut self, view: &FleetView) -> Result<(), String> {
        let present: BTreeMap<&str, &FleetIssue> = view.issues.iter().map(|i| (i.id.as_str(), i)).collect();
        let fresh: BTreeSet<&str> = view
            .machines
            .iter()
            .filter(|m| matches!(m.connection, MachineConnection::Connected { .. }))
            .map(|m| m.id.as_str())
            .collect();
        let Self { cleared, shown, .. } = self;
        let before = cleared.len();
        cleared.retain(|id, fingerprint| match present.get(id.as_str()) {
            Some(issue) => {
                shown.insert(id.clone(), absence_machine(issue));
                issue.fingerprint == *fingerprint
            }
            None => match shown.get(id) {
                None => true,
                Some(machine) => machine.as_deref().is_some_and(|m| !fresh.contains(m)),
            },
        });
        shown.retain(|id, _| cleared.contains_key(id));
        if cleared.len() == before || self.load_error.is_some() {
            return Ok(());
        }
        self.save()
    }

    fn writable(&self) -> Result<(), String> {
        self.load_error.clone().map_or(Ok(()), Err)
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let saved = Saved {
            version: VERSION,
            cleared: self.cleared.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())?;
        crate::atomic_write::atomic_write(path, &bytes).map_err(|e| e.to_string())
    }
}

/// The machine whose fresh read shows a finding has really gone.
fn absence_machine(issue: &FleetIssue) -> Option<String> {
    if issue.category == FleetIssueCategory::Alert {
        None
    } else {
        issue.machine_id.clone()
    }
}
