//! The move to the central pools: which repositories are moving, and which
//! long-lived runners they leave behind. The inventory's `[[rollout]]` list is
//! the source; GitHub's view of each old runner is added beside it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::desired::model::{Inventory, RolloutState};
use crate::model::{RunnerActivity, RunnerView};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RolloutRepoView {
    /// `owner/repo`.
    pub repo: String,
    pub state: RolloutState,
    pub safe_to_retire: bool,
    pub old_runners: Vec<OldRunnerView>,
}

/// A long-lived runner a repository used before its move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OldRunnerView {
    pub name: String,
    /// The runner's key in this view, when the dashboard lists it.
    pub runner_key: Option<String>,
    /// `None` when no listed runner has this name.
    pub activity: Option<RunnerActivity>,
}

/// Every `[[rollout]]` entry in inventory order, with each old runner's activity.
pub fn view(inventory: Option<&Inventory>, runners: &[&RunnerView]) -> Vec<RolloutRepoView> {
    let Some(inventory) = inventory else {
        return Vec::new();
    };
    inventory
        .rollout
        .iter()
        .map(|entry| RolloutRepoView {
            repo: entry.repo.clone(),
            state: entry.state,
            safe_to_retire: entry.safe_to_retire,
            old_runners: entry
                .old_runners
                .iter()
                .map(|name| {
                    let runner = runners.iter().find(|r| r.name.eq_ignore_ascii_case(name));
                    OldRunnerView {
                        name: name.clone(),
                        runner_key: runner.map(|r| r.key.clone()),
                        activity: runner.map(|r| r.activity.clone()),
                    }
                })
                .collect(),
        })
        .collect()
}
