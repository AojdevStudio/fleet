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
    /// The runner's key in this view, when exactly one listed runner of the
    /// repository's owner has this name.
    pub runner_key: Option<String>,
    /// `None` when the runner isn't linked.
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
                    let runner = old_runner(inventory, &entry.repo, name, runners);
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

/// The one listed runner called `name` that belongs to `repo`'s owner. A name
/// alone links nothing: a runner of another account, or a name two runners
/// share, stays unlinked rather than showing the wrong runner's activity.
fn old_runner<'a>(inventory: &Inventory, repo: &str, name: &str, runners: &[&'a RunnerView]) -> Option<&'a RunnerView> {
    let owner = repo.split_once('/').map(|(owner, _)| owner)?;
    let mut matches = runners
        .iter()
        .copied()
        .filter(|r| r.name.eq_ignore_ascii_case(name))
        .filter(|r| runner_owner(inventory, r).is_some_and(|o| o.eq_ignore_ascii_case(owner)));
    let first = matches.next()?;
    matches.all(|r| r.key == first.key).then_some(first)
}

/// The account a runner is registered to. A runner only the inventory lists
/// (`inv:` key) belongs to the inventory's organization.
fn runner_owner<'a>(inventory: &'a Inventory, runner: &'a RunnerView) -> Option<&'a str> {
    match &runner.org {
        Some(org) => Some(org),
        None if runner.key.starts_with("inv:") => Some(&inventory.organization.name),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::desired::inventory::parse_inventory;
    use crate::model::Tri;

    fn runner(key: &str, name: &str, org: Option<&str>) -> RunnerView {
        RunnerView {
            key: key.into(),
            name: name.into(),
            org: org.map(str::to_string),
            machine_id: None,
            installed: Tri::Unknown,
            connected: Tri::Unknown,
            activity: RunnerActivity::Idle,
            github: None,
            local: None,
            current_job: None,
            warnings: Vec::new(),
            desired: None,
        }
    }

    fn inventory(old_runners: &str) -> Inventory {
        let text = format!(
            r#"
[organization]
name = "example"
url = "https://github.com/example"

[[rollout]]
repo = "example/app"
state = "green"
old_runners = [{old_runners}]

[[hosts]]
id = "linux"
platform = "linux-x64"
ssh = "builder@192.0.2.10"
root = "~/gh-runners/example"
service_mode = "systemd"
labels = ["fleet", "linux-build", "host-linux"]
runners = [{{ name = "linux-01", directory = "runner-01" }}]
"#
        );
        parse_inventory(&text, &HashMap::new()).unwrap()
    }

    fn linked(inventory: &Inventory, runners: &[RunnerView]) -> Vec<Option<String>> {
        let runners: Vec<&RunnerView> = runners.iter().collect();
        view(Some(inventory), &runners)[0]
            .old_runners
            .iter()
            .map(|r| r.runner_key.clone())
            .collect()
    }

    #[test]
    fn a_runner_of_another_owner_is_not_linked_by_name() {
        let inventory = inventory(r#""builder-99""#);
        let unrelated = runner("gh:unrelated:99", "builder-99", Some("unrelated"));
        assert_eq!(linked(&inventory, std::slice::from_ref(&unrelated)), vec![None]);
        let own = runner("gh:example:7", "Builder-99", Some("example"));
        assert_eq!(
            linked(&inventory, &[unrelated, own]),
            vec![Some("gh:example:7".to_string())]
        );
    }

    #[test]
    fn an_ambiguous_name_stays_unlinked() {
        let inventory = inventory(r#""builder-99""#);
        let first = runner("gh:example:7", "builder-99", Some("example"));
        let second = runner("gh:example:8", "builder-99", Some("example"));
        assert_eq!(linked(&inventory, &[first, second]), vec![None]);
    }

    #[test]
    fn an_inventory_runner_belongs_to_the_inventory_organization() {
        let inventory = inventory(r#""linux-01""#);
        let local = runner("inv:linux:linux-01", "linux-01", None);
        assert_eq!(
            linked(&inventory, &[local]),
            vec![Some("inv:linux:linux-01".to_string())]
        );
    }
}
