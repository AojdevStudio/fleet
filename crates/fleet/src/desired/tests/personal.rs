//! A personal account has no runner groups and no organization endpoints, so
//! an inventory without `runner_group` is monitored per repository.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::super::github::{FleetGithub, GithubError, ReleaseAsset};
use super::super::model::{CheckStatus, FleetPlanAction};
use super::super::operations::doctor;
use super::super::plan::generate_plan;
use super::super::state::{NO_REPOSITORIES, build_status_rows, collect_snapshot};
use super::*;
use crate::sync::MutexExt;

/// A personal account's GitHub: every organization endpoint fails, and each
/// repository read is recorded.
#[derive(Debug, Default)]
struct PersonalGithub {
    runners: BTreeMap<String, GithubRunner>,
    reads: Mutex<Vec<String>>,
}

impl PersonalGithub {
    fn read(&self, what: String) {
        self.reads.locked().push(what);
    }

    fn reads(&self) -> Vec<String> {
        self.reads.locked().clone()
    }
}

fn not_an_org() -> GithubError {
    GithubError("organization endpoints don't exist for a personal account".into())
}

impl FleetGithub for PersonalGithub {
    fn auth_status(&self) -> Result<String, GithubError> {
        Ok("ok".into())
    }
    fn list_runners(&self, group: &str) -> Result<BTreeMap<String, GithubRunner>, GithubError> {
        self.read(format!("org runners {group}"));
        Err(not_an_org())
    }
    fn list_repository_runners(&self, repository: &str) -> Result<BTreeMap<String, GithubRunner>, GithubError> {
        self.read(format!("repo runners {repository}"));
        Ok(self.runners.clone())
    }
    fn validate_runner_group_policy(&self, name: &str) -> Result<i64, GithubError> {
        self.read(format!("org group {name}"));
        Err(not_an_org())
    }
    fn latest_asset(&self, _: &str) -> Result<ReleaseAsset, GithubError> {
        Err(not_an_org())
    }
    fn registration_token(&self) -> Result<String, GithubError> {
        Err(not_an_org())
    }
    fn removal_token(&self, _: Option<&str>) -> Result<String, GithubError> {
        Err(not_an_org())
    }
    fn set_labels(&self, _: i64, _: &[String]) -> Result<(), GithubError> {
        Err(not_an_org())
    }
    fn assign_group(&self, _: i64, _: i64) -> Result<(), GithubError> {
        Err(not_an_org())
    }
    fn delete_runner(&self, _: i64) -> Result<(), GithubError> {
        Err(not_an_org())
    }
}

/// `base()` for a personal account: no runner group, and `test-01` replaces a
/// legacy runner registered to `acme/app`.
fn personal() -> Inventory {
    let text = base().replace("runner_group = \"trusted\"\n", "").replace(
        "directory = \"runner-01\" }",
        "directory = \"runner-01\", replaces = { name = \"old\", directory = \"~/old\", repository = \"acme/app\" } }",
    );
    load(&text).unwrap()
}

#[test]
fn an_inventory_without_a_runner_group_loads() {
    let inventory = personal();
    assert_eq!(inventory.organization.runner_group, None);
    let e = load_err(&base().replace("runner_group = \"trusted\"", "runner_group = \"\""));
    assert_eq!(e, "runner_group must be a non-empty string");
}

#[test]
fn monitoring_without_a_runner_group_reads_runners_per_repository() {
    let inventory = personal();
    let (host, runner) = (&inventory.hosts[0], &inventory.hosts[0].runners[0]);
    let labels = host.labels_for(runner);
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let github = PersonalGithub {
        runners: [("test-01".to_string(), gr("test-01", "online", false, &labels, None))].into(),
        ..Default::default()
    };
    let fleet = FakeFleet::default();

    let snapshot = collect_snapshot(&inventory, &github, &fleet, None).unwrap();
    assert_eq!(snapshot.github_error, None);
    assert!(snapshot.policy_errors.is_empty());
    assert!(snapshot.github.contains_key("test-01"));
    assert_eq!(github.reads(), vec!["repo runners acme/app"]);

    // No group means no group membership to report as drift.
    let plan = generate_plan(&inventory, &snapshot, None, None).unwrap();
    let group_steps = [
        FleetPlanAction::AssignGroup,
        FleetPlanAction::InspectGroup,
        FleetPlanAction::HardenGroup,
    ];
    assert!(plan.iter().all(|i| !group_steps.contains(&i.action)), "{plan:?}");

    let checks = doctor(&inventory, &github, &fleet, None).unwrap();
    let auth = checks.iter().find(|c| c.check == "github-auth").unwrap();
    assert_eq!(auth.status, CheckStatus::Pass, "{}", auth.detail);
    assert!(github.reads().iter().all(|r| r.starts_with("repo runners ")));
}

#[test]
fn apply_still_needs_a_runner_group() {
    let inventory = personal();
    let github = PersonalGithub::default();
    let fleet = FakeFleet::default();
    let ops = Ops {
        inventory: &inventory,
        github: &github,
        remote: &fleet,
        secrets: Secrets::default(),
        sleep: &no_sleep,
    };
    let e = ops.apply_host("test", None).unwrap_err();
    assert_eq!(
        e.0,
        "preflight failed without mutation: The inventory has no runner_group. \
         Add one to [organization] before registering runners."
    );
    assert!(fleet.world().tokens.is_empty());
    assert!(fleet.world().calls.is_empty());
}

#[test]
fn no_runner_group_and_no_repository_is_an_empty_scope_error() {
    let inventory = load(&base().replace("runner_group = \"trusted\"\n", "")).unwrap();
    assert!(inventory.runner_repositories().is_empty());
    let github = PersonalGithub::default();
    let fleet = FakeFleet::default();

    let snapshot = collect_snapshot(&inventory, &github, &fleet, None).unwrap();
    assert_eq!(snapshot.github_error.as_deref(), Some(NO_REPOSITORIES));
    let rows = build_status_rows(&inventory, &snapshot, None).unwrap();
    assert!(rows.iter().all(|r| r.drift == "GitHub unavailable"), "{rows:?}");

    let checks = doctor(&inventory, &github, &fleet, None).unwrap();
    let auth = checks.iter().find(|c| c.check == "github-auth").unwrap();
    assert_eq!(auth.status, CheckStatus::Fail);
    assert_eq!(auth.detail, NO_REPOSITORIES);
    assert!(github.reads().is_empty(), "{:?}", github.reads());
}

#[test]
fn status_rows_without_a_runner_group_show_no_group_drift() {
    let inventory = personal();
    let (host, runner) = (&inventory.hosts[0], &inventory.hosts[0].runners[0]);
    let labels = host.labels_for(runner);
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let local = LocalRunner::new("test-01", "~/gh-runners/acme/runner-01", true, "active", None);
    // A repository read can't tell group membership, so it is `None`.
    let snap = snapshot(vec![gr("test-01", "online", false, &labels, None)], vec![local.clone()]);
    let rows = build_status_rows(&inventory, &snap, None).unwrap();
    let row = rows.iter().find(|r| r.runner == "test-01").unwrap();
    assert_eq!(row.drift, "in sync");

    // With a runner group, unknown membership is still drift.
    let grouped = load(&base()).unwrap();
    let snap = snapshot(vec![gr("test-01", "online", false, &labels, None)], vec![local]);
    let rows = build_status_rows(&grouped, &snap, None).unwrap();
    assert_eq!(rows[0].drift, "group unavailable");
}

#[test]
fn no_runner_group_reads_the_listed_repositories_without_legacy_runners() {
    let inventory = load(&format!(
        "repositories = [\"acme/web\", \"acme/app\"]\n{}",
        base().replace("runner_group = \"trusted\"\n", "")
    ))
    .unwrap();
    let github = PersonalGithub::default();
    let fleet = FakeFleet::default();

    let snapshot = collect_snapshot(&inventory, &github, &fleet, None).unwrap();
    assert_eq!(snapshot.github_error, None);
    assert_eq!(github.reads(), vec!["repo runners acme/app", "repo runners acme/web"]);

    let checks = doctor(&inventory, &github, &fleet, None).unwrap();
    let auth = checks.iter().find(|c| c.check == "github-auth").unwrap();
    assert_eq!(auth.status, CheckStatus::Pass, "{}", auth.detail);
}
