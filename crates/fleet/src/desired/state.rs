//! Combining GitHub and host truth into status rows, ported from `state.py`.

use std::collections::{BTreeMap, BTreeSet};

use super::github::{FleetGithub, GithubError};
use super::model::{FleetSnapshot, GithubRunner, Host, Inventory, LocalRunner, LookupError, StatusRow, iter_runners};
use super::remote::FleetRemote;

/// Read-only: lists runners on GitHub and probes each selected host. Failures
/// are recorded, never turned into "missing".
pub fn collect_snapshot(
    inventory: &Inventory,
    github: &dyn FleetGithub,
    remote: &dyn FleetRemote,
    host_id: Option<&str>,
) -> Result<FleetSnapshot, LookupError> {
    let hosts = inventory.selected_hosts(host_id)?;
    let mut snapshot = FleetSnapshot::default();
    let group = inventory.organization.runner_group.as_deref();
    let mut repositories: BTreeSet<String> = iter_runners(&hosts)
        .filter_map(|(_, r)| r.replaces.as_ref()?.repository.clone())
        .collect();
    if group.is_none() {
        repositories.extend(inventory.runner_repositories());
    }
    match read_github_runners(github, group, &repositories) {
        Ok(runners) => {
            if let Some(group) = group
                && let Err(e) = github.validate_runner_group_policy(group)
            {
                snapshot.policy_errors.push(e.0);
            }
            snapshot.github = runners;
        }
        Err(e) => snapshot.github_error = Some(e.0),
    }
    for host in hosts {
        match remote.probe(host) {
            Ok(local) => snapshot.local.extend(local),
            Err(e) => {
                snapshot.host_errors.insert(host.id.clone(), e.0);
            }
        }
    }
    Ok(snapshot)
}

/// Shown when an inventory without a runner group names no repository, so
/// there is nothing to read runners from.
pub const NO_REPOSITORIES: &str = "The inventory has no runner_group and names no repository, so there are no runners to read. \
     Add a runner_group to [organization], or a repository to a legacy runner's replaces entry.";

/// The organization's runners when there is a runner group, plus those in
/// each of `repositories`. Without a group (a personal account) only the
/// per-repository reads run, so no organization endpoint is called. Without
/// a group or a repository, the read fails rather than report no runners.
pub(crate) fn read_github_runners(
    github: &dyn FleetGithub,
    group: Option<&str>,
    repositories: &BTreeSet<String>,
) -> Result<BTreeMap<String, GithubRunner>, GithubError> {
    let mut runners = match group {
        Some(group) => github.list_runners(group)?,
        None if repositories.is_empty() => return Err(GithubError(NO_REPOSITORIES.into())),
        None => BTreeMap::new(),
    };
    for repository in repositories {
        runners.extend(github.list_repository_runners(repository)?);
    }
    Ok(runners)
}

pub fn build_status_rows(
    inventory: &Inventory,
    snapshot: &FleetSnapshot,
    host_id: Option<&str>,
) -> Result<Vec<StatusRow>, LookupError> {
    let hosts = inventory.selected_hosts(host_id)?;
    let mut rows = Vec::new();
    let mut accounted: BTreeSet<&str> = BTreeSet::new();
    let error = snapshot.github_error.as_deref();

    for (host, runner) in iter_runners(&hosts) {
        let github = snapshot.github.get(&runner.name);
        let local = snapshot.local.get(&runner.name);
        rows.push(StatusRow {
            runner: runner.name.clone(),
            host: Some(host.id.clone()),
            github_state: github_state(github, error),
            service_state: Some(service_state(local, host, snapshot)),
            busy: busy(github, error),
            version: version(local),
            drift: if runner.decommission {
                decommission_drift(github, local, host, snapshot)
            } else {
                desired_drift(&host.labels_for(runner), github, local, host, snapshot)
            },
            desired: !runner.decommission,
        });
        accounted.insert(&runner.name);
        if let Some(old) = &runner.replaces {
            let old_github = snapshot.github.get(&old.name);
            let old_local = snapshot.local.get(&old.name);
            let host_error = snapshot.host_errors.contains_key(&host.id);
            if old_github.is_some() || old_local.is_some() || error.is_some() || host_error {
                rows.push(StatusRow {
                    runner: old.name.clone(),
                    host: Some(host.id.clone()),
                    github_state: github_state(old_github, error),
                    service_state: Some(service_state(old_local, host, snapshot)),
                    busy: busy(old_github, error),
                    version: version(old_local),
                    drift: if old_github.is_some() || old_local.is_some() {
                        format!("legacy → {}", runner.name)
                    } else {
                        "legacy state unavailable".into()
                    },
                    desired: false,
                });
            }
            accounted.insert(&old.name);
        }
    }

    if host_id.is_none() && error.is_none() {
        for (name, github) in &snapshot.github {
            if accounted.contains(name.as_str()) {
                continue;
            }
            rows.push(StatusRow {
                runner: name.clone(),
                host: None,
                github_state: github.status.clone(),
                service_state: None,
                busy: Some(github.busy),
                version: None,
                drift: "unmanaged".into(),
                desired: false,
            });
        }
    }
    Ok(rows)
}

fn version(local: Option<&LocalRunner>) -> Option<String> {
    local.and_then(|l| l.version.clone()).filter(|v| !v.is_empty())
}

fn github_state(runner: Option<&GithubRunner>, error: Option<&str>) -> String {
    if error.is_some() {
        return "unknown".into();
    }
    runner.map(|r| r.status.clone()).unwrap_or_else(|| "missing".into())
}

fn service_state(local: Option<&LocalRunner>, host: &Host, snapshot: &FleetSnapshot) -> String {
    if snapshot.host_errors.contains_key(&host.id) {
        return "unknown".into();
    }
    match local {
        Some(l) if l.configured => l.service_state.clone(),
        _ => "missing".into(),
    }
}

fn busy(runner: Option<&GithubRunner>, error: Option<&str>) -> Option<bool> {
    match (error, runner) {
        (None, Some(r)) => Some(r.busy),
        _ => None,
    }
}

fn decommission_drift(
    github: Option<&GithubRunner>,
    local: Option<&LocalRunner>,
    host: &Host,
    snapshot: &FleetSnapshot,
) -> String {
    if snapshot.github_error.is_some() || snapshot.host_errors.contains_key(&host.id) {
        return "decommission state unavailable".into();
    }
    if github.is_none() && !local.is_some_and(|l| l.configured) {
        return "decommissioned".into();
    }
    "decommission pending".into()
}

fn desired_drift(
    labels: &[String],
    github: Option<&GithubRunner>,
    local: Option<&LocalRunner>,
    host: &Host,
    snapshot: &FleetSnapshot,
) -> String {
    let mut unknown = Vec::new();
    if snapshot.github_error.is_some() {
        unknown.push("GitHub unavailable");
    }
    if snapshot.host_errors.contains_key(&host.id) {
        unknown.push("host unavailable");
    }
    if !unknown.is_empty() {
        return unknown.join("; ");
    }
    let (github, local) = match (github, local.filter(|l| l.configured)) {
        (None, None) => return "install".into(),
        (None, Some(_)) => return "missing on GitHub".into(),
        (Some(_), None) => return "missing locally".into(),
        (Some(g), Some(l)) => (g, l),
    };
    let mut issues = Vec::new();
    let (missing, unexpected) = label_drift(labels, github);
    if !missing.is_empty() {
        issues.push(format!("labels: +{}", missing.join(",")));
    }
    if !unexpected.is_empty() {
        issues.push(format!("labels: -{}", unexpected.join(",")));
    }
    match github.in_desired_group {
        Some(false) => issues.push("wrong group".into()),
        None => issues.push("group unavailable".into()),
        Some(true) => {}
    }
    if !local.service_active() {
        issues.push(format!("service {}", local.service_state));
    }
    if github.status != "online" {
        issues.push(format!("GitHub {}", github.status));
    }
    if issues.is_empty() {
        "in sync".into()
    } else {
        issues.join("; ")
    }
}

/// Desired labels GitHub lacks, and custom labels GitHub has that aren't
/// desired; both sorted.
pub(crate) fn label_drift(desired: &[String], github: &GithubRunner) -> (Vec<String>, Vec<String>) {
    let desired: BTreeSet<&String> = desired.iter().collect();
    let missing = desired
        .iter()
        .filter(|l| !github.labels.contains(**l))
        .map(|l| (*l).clone())
        .collect();
    let unexpected = github
        .custom_labels
        .iter()
        .filter(|l| !desired.contains(l))
        .cloned()
        .collect();
    (missing, unexpected)
}
