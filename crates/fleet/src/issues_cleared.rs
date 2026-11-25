//! Clear: which findings the user has hidden, saved next to `config.json`.
//!
//! The rules:
//! - Clear saves the finding's ID and the fingerprint the user saw (see [`super::rules`]).
//!   The finding stays hidden while a finding with that ID has that fingerprint.
//! - A new fingerprint brings it back, and the saved clear is dropped.
//! - A finding that goes away and returns comes back. "Gone" needs evidence from the
//!   source that raised it, saved with the clear as [`Confirm`]:
//!   - The inventory error: absence while the inventory is loaded.
//!   - An alert: absence while the inventory is loaded and machine history was read.
//!     Monitoring keeps an alert while its data is missing, but a broken inventory lists
//!     no machines, and unreadable history has no alerts.
//!   - A diagnostic, connection or runner finding on a machine: absence while the machine
//!     has a fresh SSH read, which reads those again.
//!   - A Tailscale finding: absence while Tailscale reports this Mac online.
//!   - Anything else: absence after the app showed the finding since launch, and for a
//!     machine finding, while the machine has a fresh SSH read.
//! - Bring back drops the saved clear at once.
//! - Every fleet view drops the clears that no longer apply, not only the poller's tick,
//!   so a problem that a view showed gone comes back even if it returns at once.
//! - A failed save changes nothing. The next view tries again.
//!
//! Not covered: a value the app can't read (`Unknown`) hides a diagnostic warning, and that
//! counts as gone. A "shown since launch" finding (GitHub, runner checks, the runner plan,
//! runner group, failed change) that is gone when the app starts and returns with the same
//! fingerprint stays cleared, because the app never saw it gone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{FleetIssue, FleetIssueCategory};
use crate::model::{FleetView, MachineConnection};

const VERSION: u32 = 1;

/// What shows that a cleared finding has really gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Confirm {
    InventoryLoaded,
    AlertsKnown,
    FreshRead { machine: String },
    TailnetOnline,
    Shown { machine: Option<String> },
}

impl Confirm {
    fn of(issue: &FleetIssue) -> Self {
        use FleetIssueCategory::{Alert, Connection, Diagnostic, Runner, Tailnet};
        match (issue.category, &issue.machine_id) {
            (Alert, _) => Self::AlertsKnown,
            _ if issue.id == "inventory" => Self::InventoryLoaded,
            (Diagnostic | Connection | Runner, Some(machine)) => Self::FreshRead {
                machine: machine.clone(),
            },
            (Tailnet, _) => Self::TailnetOnline,
            (_, machine) => Self::Shown {
                machine: machine.clone(),
            },
        }
    }

    fn gone(&self, shown: bool, view: &FleetView) -> bool {
        let fresh = |id: &str| {
            view.machines
                .iter()
                .any(|m| m.id == id && matches!(m.connection, MachineConnection::Connected { .. }))
        };
        match self {
            Self::InventoryLoaded => view.plan.organization.is_some(),
            Self::AlertsKnown => view.plan.organization.is_some() && view.monitoring.storage_error.is_none(),
            Self::FreshRead { machine } => fresh(machine),
            Self::TailnetOnline => view.tailnet.local_online == Some(true),
            Self::Shown { machine } => shown && machine.as_deref().is_none_or(fresh),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    fingerprint: String,
    confirm: Confirm,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    cleared: BTreeMap<String, Entry>,
}

#[derive(Debug)]
pub(crate) struct ClearStore {
    /// `None` keeps clears in memory only (demo data).
    path: Option<PathBuf>,
    cleared: BTreeMap<String, Entry>,
    /// Cleared findings the app has shown since launch.
    shown: BTreeSet<String>,
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
            shown: BTreeSet::new(),
            load_error,
        }
    }

    /// Clears kept only in memory, from issue ID to fingerprint.
    pub fn in_memory(cleared: BTreeMap<String, String>) -> Self {
        Self {
            path: None,
            cleared: cleared
                .into_iter()
                .map(|(id, fingerprint)| {
                    let confirm = Confirm::Shown { machine: None };
                    (id, Entry { fingerprint, confirm })
                })
                .collect(),
            shown: BTreeSet::new(),
            load_error: None,
        }
    }

    pub fn is_cleared(&self, issue: &FleetIssue) -> bool {
        self.cleared
            .get(&issue.id)
            .is_some_and(|e| e.fingerprint == issue.fingerprint)
    }

    pub fn clear(&mut self, issue: &FleetIssue) -> Result<(), String> {
        let mut next = self.cleared.clone();
        let entry = Entry {
            fingerprint: issue.fingerprint.clone(),
            confirm: Confirm::of(issue),
        };
        next.insert(issue.id.clone(), entry);
        self.commit(next)?;
        self.shown.insert(issue.id.clone());
        Ok(())
    }

    pub fn restore(&mut self, id: &str) -> Result<(), String> {
        let mut next = self.cleared.clone();
        if next.remove(id).is_some() {
            self.commit(next)?;
        }
        Ok(())
    }

    /// Drop clears that no longer apply: a changed fingerprint, or a finding that went away.
    pub fn record(&mut self, view: &FleetView) -> Result<(), String> {
        let present: BTreeMap<&str, &FleetIssue> = view.issues.iter().map(|i| (i.id.as_str(), i)).collect();
        let next: BTreeMap<String, Entry> = self
            .cleared
            .iter()
            .filter(|(id, entry)| match present.get(id.as_str()) {
                Some(issue) => issue.fingerprint == entry.fingerprint,
                None => !entry.confirm.gone(self.shown.contains(*id), view),
            })
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect();
        self.shown.extend(
            self.cleared
                .keys()
                .filter(|id| present.contains_key(id.as_str()))
                .cloned(),
        );
        if next != self.cleared {
            self.commit(next)?;
        }
        let cleared = &self.cleared;
        self.shown.retain(|id| cleared.contains_key(id));
        Ok(())
    }

    /// Save `next`, then use it. A failed save leaves the last saved clears in effect.
    fn commit(&mut self, next: BTreeMap<String, Entry>) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        if let Some(path) = &self.path {
            let saved = Saved {
                version: VERSION,
                cleared: next,
            };
            let bytes = serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())?;
            crate::atomic_write::atomic_write(path, &bytes).map_err(|e| e.to_string())?;
            self.cleared = saved.cleared;
        } else {
            self.cleared = next;
        }
        Ok(())
    }
}
