//! Shared attention findings for the desktop and local HTTP clients.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::desired::model::{CheckStatus, FleetPlanAction};
use crate::machine_diagnostics::DiagnosticStatus;
use crate::model::{DataSource, FleetView, MachineConnection, MachineView, count_u32};
use crate::monitoring::FleetAlertKind;
use FleetIssueCategory::{
    Alert, Connection, Diagnostic, Github, Inventory, Operation, Queue, Runner, Storage, Tailnet,
};
use rules::Advice;

#[path = "issues_cleared.rs"]
pub(crate) mod cleared;

/// The observation responsible for a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum FleetIssueCategory {
    Alert,
    Diagnostic,
    Inventory,
    Connection,
    Runner,
    Queue,
    Github,
    Tailnet,
    Storage,
    Operation,
}

/// What resolves a finding in Settings, for findings that aren't about one machine or runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum FleetIssueFix {
    /// Add the inventory's organization for live runner status and jobs.
    AddOrganization,
    /// Repair an added organization's GitHub sign-in.
    OrganizationSignIn,
}

/// A stable, actionable finding; stale and muted conditions remain unresolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FleetIssue {
    pub id: String,
    pub category: FleetIssueCategory,
    pub title: String,
    pub detail: String,
    /// What to do, in one or two sentences. A grouped finding has one line per problem.
    pub action: String,
    pub machine_id: Option<String>,
    pub runner_key: Option<String>,
    pub stale: bool,
    pub muted: bool,
    pub fix: Option<FleetIssueFix>,
    /// Hidden by Clear while the same problem lasts. Cleared findings don't count in `summary.attention`.
    pub cleared: bool,
    /// What makes this the same problem for Clear (see [`rules`]). Clear sends back the
    /// fingerprint the user saw, so a problem that changed meanwhile isn't hidden.
    pub fingerprint: String,
}

impl FleetIssue {
    fn new(
        id: impl Into<String>,
        category: FleetIssueCategory,
        title: impl Into<String>,
        detail: impl Into<String>,
        advice: Advice,
    ) -> Self {
        Self {
            id: id.into(),
            category,
            title: title.into(),
            detail: detail.into(),
            action: advice.action,
            machine_id: None,
            runner_key: None,
            stale: false,
            muted: false,
            fix: None,
            cleared: false,
            fingerprint: advice.fingerprint,
        }
    }
}

/// Rebuild findings after monitoring has attached its current alerts.
pub(crate) fn attach(view: &mut FleetView, is_cleared: impl Fn(&FleetIssue) -> bool) {
    let mut issues: Vec<FleetIssue> = [
        alert_issues(view),
        fleet_issues(view),
        machine_issues(view),
        runner_issues(view),
        queue_issues(view),
        github_issues(view),
        control_check_issues(view),
        operation_issues(view),
    ]
    .into_iter()
    .flatten()
    .collect();
    for issue in &mut issues {
        issue.cleared = is_cleared(issue);
    }
    view.summary.attention = count_u32(issues.iter().filter(|i| !i.cleared).count());
    view.issues = issues;
}

fn alert_issues(view: &FleetView) -> Vec<FleetIssue> {
    view.monitoring
        .alerts
        .iter()
        .map(|alert| {
            let mut issue = FleetIssue::new(
                format!("alert:{}", alert.id),
                Alert,
                format!(
                    "{}: {}",
                    alert.machine_name,
                    match alert.kind {
                        FleetAlertKind::Offline => "offline",
                        FleetAlertKind::LowDisk => "low disk space",
                    }
                ),
                &alert.detail,
                rules::alert(alert),
            );
            issue.machine_id = view
                .machines
                .iter()
                .find(|m| m.preferences_key() == alert.machine_key || m.id == alert.machine_key)
                .map(|m| m.id.clone());
            issue.stale = alert.stale;
            issue.muted = alert.muted;
            issue
        })
        .collect()
}

/// Problems with the inventory, Tailscale, local storage and the runner group.
fn fleet_issues(view: &FleetView) -> Vec<FleetIssue> {
    let mut issues = Vec::new();
    for (id, title, error, category, action) in [
        (
            "inventory",
            "The inventory has a problem",
            &view.plan.inventory_error,
            Inventory,
            rules::INVENTORY,
        ),
        (
            "plan-github",
            "GitHub couldn't be read for the inventory check",
            &view.plan.github_error,
            Inventory,
            rules::PLAN_GITHUB,
        ),
        (
            "doctor",
            "Runner checks failed",
            &view.plan.checks_error,
            Inventory,
            rules::CHECKS_FAILED,
        ),
        (
            "tailnet",
            "Tailscale",
            &view.tailnet.error,
            Tailnet,
            rules::TAILNET_ERROR,
        ),
    ] {
        if let Some(error) = error {
            issues.push(FleetIssue::new(id, category, title, error, rules::error(action, error)));
        }
    }
    if let Some(error) = &view.monitoring.storage_error {
        issues.push(FleetIssue::new(
            "monitoring-storage",
            Storage,
            "History storage has a problem",
            error,
            rules::history_storage(error),
        ));
    }
    if view.tailnet.local_online == Some(false) && view.tailnet.error.is_none() {
        issues.push(FleetIssue::new(
            "tailnet-offline",
            Tailnet,
            "Tailscale is offline",
            "This Mac is disconnected from the tailnet.",
            rules::tailnet_offline(),
        ));
    }
    for (index, item) in view
        .plan
        .control
        .iter()
        .enumerate()
        .filter(|(_, p)| p.action != FleetPlanAction::NoChange)
    {
        issues.push(FleetIssue::new(
            format!("control:{index}"),
            Inventory,
            "Runner group needs attention",
            &item.reason,
            rules::plan_steps(std::slice::from_ref(&item)),
        ));
    }
    issues
}

fn machine_issues(view: &FleetView) -> Vec<FleetIssue> {
    view.machines
        .iter()
        .flat_map(|machine| {
            let mut issues = issues_for_machine(machine);
            for issue in &mut issues {
                issue.machine_id = Some(machine.id.clone());
            }
            issues
        })
        .collect()
}

fn issues_for_machine(machine: &MachineView) -> Vec<FleetIssue> {
    let mut issues = Vec::new();
    let has_alert = |kind| machine.monitoring.alerts.iter().any(|a| a.kind == kind);
    let offline_alert = has_alert(FleetAlertKind::Offline);
    let disk_alert = has_alert(FleetAlertKind::LowDisk);
    if let Some(diagnostics) = &machine.diagnostics
        && !diagnostics.stale
        && matches!(machine.connection, MachineConnection::Connected { .. })
    {
        let warnings: Vec<_> = diagnostics
            .checks
            .iter()
            .filter(|c| c.status == DiagnosticStatus::Warn && !(disk_alert && c.id == "disk-space"))
            .collect();
        if !warnings.is_empty() {
            issues.push(FleetIssue::new(
                format!("{}:diagnostics", machine.id),
                Diagnostic,
                format!(
                    "{}: {} diagnostic warning{}",
                    machine.display_name,
                    warnings.len(),
                    if warnings.len() == 1 { "" } else { "s" }
                ),
                warnings
                    .iter()
                    .map(|c| format!("{}: {}", c.title, c.value))
                    .collect::<Vec<_>>()
                    .join(" · "),
                rules::diagnostics(machine.os, &warnings),
            ));
        }
    }
    if let Some(desired) = &machine.desired {
        if let Some(error) = &desired.error {
            if !offline_alert {
                issues.push(FleetIssue::new(
                    format!("{}:check", machine.id),
                    Inventory,
                    if machine.this_mac {
                        format!("{}: the local check failed", machine.display_name)
                    } else {
                        format!("{} couldn't be checked", machine.display_name)
                    },
                    error,
                    if machine.this_mac {
                        rules::local()
                    } else {
                        rules::ssh(error)
                    },
                ));
            }
        } else {
            let steps: Vec<_> = desired
                .plan
                .iter()
                .filter(|p| p.action != FleetPlanAction::NoChange)
                .collect();
            if !steps.is_empty() {
                issues.push(FleetIssue::new(
                    format!("{}:changes", machine.id),
                    Inventory,
                    format!("{}: runner plan needs attention", machine.display_name),
                    steps
                        .iter()
                        .map(|p| {
                            format!(
                                "{}: {}{}",
                                p.runner,
                                p.reason,
                                if p.blocked { " (blocked)" } else { "" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" · "),
                    rules::plan_steps(&steps),
                ));
            }
        }
        for check in desired.checks.iter().filter(|c| c.status != CheckStatus::Pass) {
            issues.push(FleetIssue::new(
                format!("{}:doctor:{}", machine.id, check.check),
                Inventory,
                format!("{}: {}", machine.display_name, check.check),
                &check.detail,
                rules::doctor(check),
            ));
        }
    }
    if !offline_alert && machine.desired.as_ref().is_none_or(|d| d.error.is_none()) {
        let problem = match &machine.connection {
            MachineConnection::Stale { reason, .. } => Some(("stale", reason.as_str())),
            MachineConnection::Error { detail } if machine.this_mac => {
                Some(("the local check failed", detail.as_str()))
            }
            MachineConnection::Error { detail } => Some(("SSH unavailable", detail.as_str())),
            MachineConnection::Offline { .. } => Some(("offline", "Offline on the tailnet")),
            MachineConnection::Connected { .. } | MachineConnection::Pending | MachineConnection::Disabled => None,
        };
        if let Some((title, detail)) = problem {
            let mut issue = FleetIssue::new(
                format!("{}:connection", machine.id),
                Connection,
                format!("{}: {title}", machine.display_name),
                detail,
                match &machine.connection {
                    MachineConnection::Error { .. } | MachineConnection::Stale { .. } if machine.this_mac => {
                        rules::local()
                    }
                    connection => rules::connection(connection),
                },
            );
            issue.stale = matches!(machine.connection, MachineConnection::Stale { .. });
            issues.push(issue);
        }
    }
    issues
}

fn runner_issues(view: &FleetView) -> Vec<FleetIssue> {
    view.machines
        .iter()
        .flat_map(|m| &m.runners)
        .chain(&view.pool_runners)
        .chain(&view.unmatched_runners)
        .filter(|runner| !runner.warnings.is_empty())
        .map(|runner| {
            let mut issue = FleetIssue::new(
                format!("{}:warnings", runner.key),
                Runner,
                &runner.name,
                runner.warnings.join(" · "),
                rules::runner_warnings(&runner.warnings),
            );
            issue.runner_key = Some(runner.key.clone());
            issue.machine_id.clone_from(&runner.machine_id);
            issue
        })
        .collect()
}

fn queue_issues(view: &FleetView) -> Vec<FleetIssue> {
    view.queue
        .jobs
        .iter()
        .filter(|q| q.compatible_online == 0)
        .map(|job| {
            let mut issue = FleetIssue::new(
                format!("queue:{}:{}", job.org, job.job.id),
                Queue,
                format!("{}: no compatible online runner observed", job.job.name),
                job.stuck.as_ref().map_or_else(
                    || "GitHub hasn't sent the runner list yet, so no runner is matched.".into(),
                    |stuck| stuck.detail.clone(),
                ),
                rules::queue(job.stuck.as_ref().map(|s| s.reason)),
            );
            issue.stale = !view.queue.complete;
            issue
        })
        .collect()
}

fn github_issues(view: &FleetView) -> Vec<FleetIssue> {
    let mut issues = Vec::new();
    if let Some(org) = &view.plan.organization
        && view.source == DataSource::Live
        && !view.orgs.iter().any(|o| o.login.eq_ignore_ascii_case(&org.name))
    {
        let mut issue = FleetIssue::new(
            "watch-org",
            Github,
            format!("Watch {} on GitHub live", org.name),
            "Runner state comes from the last check.",
            rules::error(rules::WATCH_ORG, &org.name),
        );
        issue.fix = Some(FleetIssueFix::AddOrganization);
        issues.push(issue);
    }
    for org in &view.orgs {
        if let Some(error) = org
            .error
            .as_deref()
            .or_else(|| (!org.has_token).then_some("Token missing from Keychain"))
        {
            let mut issue = FleetIssue::new(
                format!("org:{}", org.login),
                Github,
                format!("GitHub: {}", org.login),
                error,
                rules::error(rules::ORG_SIGN_IN, error),
            );
            issue.fix = Some(FleetIssueFix::OrganizationSignIn);
            issues.push(issue);
        }
    }
    issues
}

fn control_check_issues(view: &FleetView) -> Vec<FleetIssue> {
    view.plan
        .control_checks
        .iter()
        .filter(|c| c.status != CheckStatus::Pass)
        .map(|check| {
            FleetIssue::new(
                format!("doctor:{}:{}", check.host, check.check),
                Inventory,
                &check.check,
                &check.detail,
                rules::doctor(check),
            )
        })
        .collect()
}

fn operation_issues(view: &FleetView) -> Vec<FleetIssue> {
    view.plan
        .operation
        .iter()
        .filter_map(|operation| {
            let error = operation.error.as_ref()?;
            Some(FleetIssue::new(
                "last-operation",
                Operation,
                &operation.title,
                error,
                rules::error(
                    rules::OPERATION_FAILED,
                    &format!("{} {}: {error}", operation.title, operation.started_at),
                ),
            ))
        })
        .collect()
}

/// What to do about each finding, and its fingerprint: what makes it the same
/// problem for Clear. A cleared finding stays hidden while its fingerprint holds.
///
/// Fingerprints by category. Live values never count, so a new reading of the
/// same problem keeps it cleared.
/// - Alert: the kind. For low disk, also the mount points (not the free space).
/// - Diagnostic: the IDs of the warning checks (not their values).
/// - Connection: the state (stale, offline, SSH error). For SSH errors, also the
///   kind of failure (key, host key, unreachable, other).
/// - Inventory: a runner check's ID and status; a runner plan's runners and their
///   steps, with the label changes for a label step; the runner group's policy
///   violations; the kind of SSH failure for a machine that couldn't be checked;
///   the error text for everything else.
/// - Runner: the kind of each warning (not configured, not running).
/// - Queue: the stuck reason, or that the runner list hasn't arrived.
/// - Github, Tailnet, Storage: the error text. Operation: the error text and the start
///   time, so a retry that fails the same way is a new problem.
pub(crate) mod rules {
    use crate::desired::model::{DoctorCheck, FleetPlanAction, PlanItem};
    use crate::machine::OsFamily;
    use crate::machine_diagnostics::MachineDiagnostic;
    use crate::model::{MachineConnection, QueueStuckReason};
    use crate::monitoring::{FleetAlertKind, FleetMachineAlert};

    pub(crate) struct Advice {
        pub action: String,
        pub fingerprint: String,
    }

    fn advice(action: impl Into<String>, fingerprint: impl Into<String>) -> Advice {
        Advice {
            action: action.into(),
            fingerprint: fingerprint.into(),
        }
    }

    pub(super) const INVENTORY: &str = "Create or fix the inventory file. The app reads it again when it changes.";
    pub(super) const PLAN_GITHUB: &str = "Check this Mac's network and GitHub sign-in, then select Check now.";
    pub(super) const CHECKS_FAILED: &str = "Fix the error in the detail, then run Runner checks again.";
    pub(super) const TAILNET_ERROR: &str = "Check that Tailscale is installed and running on this Mac.";
    pub(super) const WATCH_ORG: &str = "Add the organization in Settings for live status and jobs.";
    pub(super) const ORG_SIGN_IN: &str = "Sign in to this organization again in Settings.";
    pub(super) const OPERATION_FAILED: &str = "Fix what the error names, then review the change again.";

    /// A finding whose fingerprint is its error text.
    pub(super) fn error(action: &str, error: &str) -> Advice {
        advice(action, error)
    }

    /// History that can't be read is kept as is, so it has to be moved aside. A failed save
    /// is about a file that reads fine, so the write error is the thing to fix.
    pub(super) fn history_storage(error: &str) -> Advice {
        let action = if error.starts_with(crate::monitoring::UNREADABLE_HISTORY) {
            "Move the history file named in the detail out of the way, then restart the app."
        } else {
            "Fix the write error on the history file named in the detail, for example its permissions or a full disk. \
             Keep the file where it is. The app tries to save again every few seconds."
        };
        advice(action, error)
    }

    pub(super) fn tailnet_offline() -> Advice {
        advice("Open Tailscale on this Mac and connect.", "offline")
    }

    pub(super) fn alert(alert: &FleetMachineAlert) -> Advice {
        match alert.kind {
            FleetAlertKind::Offline => advice(
                "Check that the machine is powered on and connected to the tailnet.",
                "offline",
            ),
            FleetAlertKind::LowDisk => {
                let mounts = alert.mounts.join(", ");
                advice(
                    format!("Free space on {mounts}, or grow the disk."),
                    format!("low-disk:{mounts}"),
                )
            }
        }
    }

    pub(super) fn connection(connection: &MachineConnection) -> Advice {
        match connection {
            MachineConnection::Stale { .. } => advice(
                "Check that the machine is awake and reachable over SSH. The app keeps trying.",
                "stale",
            ),
            MachineConnection::Offline { .. } => {
                advice("Check that the machine is powered on and on the tailnet.", "offline")
            }
            MachineConnection::Error { detail } => {
                let ssh = ssh(detail);
                advice(ssh.action, format!("error:{}", ssh.fingerprint))
            }
            MachineConnection::Connected { .. } | MachineConnection::Pending | MachineConnection::Disabled => {
                advice("Nothing to do.", "none")
            }
        }
    }

    /// A failed local read of this Mac. No SSH is involved, so SSH advice never applies.
    pub(super) fn local() -> Advice {
        advice(
            "Read the error, fix it on this Mac, then select Check now. This Mac is read without SSH.",
            "local",
        )
    }

    /// An SSH failure, by kind. The fingerprint is the kind, so a new timeout or
    /// address in the same failure keeps it cleared.
    pub(super) fn ssh(detail: &str) -> Advice {
        let detail = detail.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| detail.contains(n));
        let (kind, action) = if has(&["permission denied"]) {
            (
                "key",
                "Add this Mac's SSH key to the machine, or read its health another way.",
            )
        } else if has(&["host key"]) {
            ("host-key", "Check the machine's host key, then save it in known_hosts.")
        } else if has(&["set the environment variable"]) {
            (
                "target",
                "Set the environment variable named in the detail, then select Check now.",
            )
        } else if has(&[
            "timed out",
            "connection refused",
            "no route",
            "could not resolve",
            "unreachable",
        ]) {
            (
                "unreachable",
                "Check that the machine is on and its SSH server is running.",
            )
        } else {
            (
                "other",
                "Check that this Mac can reach the machine over SSH, then select Check now.",
            )
        };
        advice(action, kind)
    }

    /// One line per warning, each led by its title.
    pub(super) fn diagnostics(os: Option<OsFamily>, warnings: &[&MachineDiagnostic]) -> Advice {
        let lines: Vec<_> = warnings
            .iter()
            .map(|c| format!("{}: {}", c.title, diagnostic(os, c)))
            .collect();
        let ids: Vec<_> = warnings.iter().map(|c| c.id.as_str()).collect();
        advice(lines.join("\n"), ids.join(","))
    }

    fn diagnostic(os: Option<OsFamily>, check: &MachineDiagnostic) -> String {
        let windows = os == Some(OsFamily::Windows);
        let id = check.id.as_str();
        match id {
            "tailscale-state" => "Open Tailscale on the machine and connect it.".into(),
            "tailscale-unattended" => "Turn on Run unattended in the Tailscale menu on that PC.".into(),
            "tailscale-startup" if windows => "Set the Tailscale service to start automatically.".into(),
            "tailscale-startup" if check.detail.contains("masked") => {
                "Run `sudo systemctl unmask tailscaled`, then `sudo systemctl enable --now tailscaled`.".into()
            }
            "tailscale-startup" => "Run `sudo systemctl enable --now tailscaled` on the machine.".into(),
            "power-sleep" if windows => "Run `powercfg /change standby-timeout-ac 0` on that PC.".into(),
            "power-sleep" => "Run `sudo pmset -c sleep 0` on that Mac.".into(),
            "power-recovery" => "Run `sudo pmset -a autorestart 1` on that Mac.".into(),
            "power-hibernate" => "Run `powercfg /change hibernate-timeout-ac 0` on that PC.".into(),
            "power-idle" => {
                "Set `IdleAction=ignore` in logind.conf, or in the drop-in that sets it, then restart systemd-logind."
                    .into()
            }
            "disk-space" => "Free space on the fullest disk, or grow it.".into(),
            _ if id.starts_with("runner:") && check.value == "Not configured" => {
                "Select Check now, then configure it as its step under Changes says.".into()
            }
            _ if id.starts_with("runner:") => "Start the runner's service, or restart it from its runner pane.".into(),
            _ => "Fix this on the machine. The Diagnostics section shows what was read.".into(),
        }
    }

    /// One line per runner, each led by the runner's name.
    pub(super) fn plan_steps(steps: &[&PlanItem]) -> Advice {
        let lines: Vec<_> = steps
            .iter()
            .map(|p| {
                let who = if p.runner.is_empty() { "Runner group" } else { &p.runner };
                format!("{who}: {}", plan_action(p))
            })
            .collect();
        // A label step's reason is the label changes, and a runner group step's reason is the
        // policy violations, so a new label or policy problem is a new fingerprint.
        let print: Vec<_> = steps
            .iter()
            .map(|p| match p.action {
                FleetPlanAction::ReconcileLabels | FleetPlanAction::HardenGroup => {
                    format!("{}:{}:{}", p.runner, p.action.as_str(), p.reason)
                }
                _ => format!("{}:{}", p.runner, p.action.as_str()),
            })
            .collect();
        advice(lines.join("\n"), print.join(","))
    }

    /// Apply, Retire and Decommission are turned off in this app, so those steps name
    /// the Runner Fleet command line the plan gives for them.
    fn plan_action(step: &PlanItem) -> String {
        use FleetPlanAction::*;
        let run = |purpose: &str, without: &str| match &step.command {
            Some(command) => format!("Run `{command}` from Runner Fleet {purpose}."),
            None => without.into(),
        };
        match step.action {
            Install => run("to install it", "Install it with Runner Fleet."),
            ReconcileLabels => match &step.command {
                Some(command) => format!(
                    "Run `{command}` from Runner Fleet to add and remove the listed labels, or change the labels in the inventory."
                ),
                None => "Change the runner's labels to match the inventory, or change the inventory.".into(),
            },
            AssignGroup => run(
                "to move it into the runner group",
                "Move it into the runner group in GitHub.",
            ),
            StartService => run("to start its service", "Start its service on the machine."),
            RetireLegacy => run(
                "to retire the legacy runner",
                "Retire the legacy runner with Runner Fleet.",
            ),
            Decommission => run("to stop and deregister it", "Decommission it with Runner Fleet."),
            Restart => "Restart it from its runner pane.".into(),
            Inspect => "Read the reason, then check the runner on the machine and in GitHub.".into(),
            InspectOffline => "Read its logs on the runner pane to see why GitHub reports it offline.".into(),
            InspectGroup => "Create the runner group in GitHub, or fix runner_group in the inventory.".into(),
            PreserveLegacy => "Wait. The legacy runner stays until its replacement is online.".into(),
            PreserveService => "Wait for its job to finish, then start its service.".into(),
            PreserveDecommission => "Wait for its job to finish, then decommission it.".into(),
            RepairRegistration => "Check it by hand. GitHub has no runner for its local configuration.".into(),
            // Decommission refuses to delete a runner GitHub reports online when its inventory
            // directory isn't configured, so this never leads with removal.
            RecoverLocal => {
                "Find the runner GitHub lists under this name, and check its directory against the inventory \
                 before you change anything. If it runs from a valid directory under the host's root, set \
                 this runner's `directory` in the inventory file to match. Remove it from GitHub only when \
                 it is offline, idle, and no machine runs it."
                    .into()
            }
            HardenGroup => {
                "Set the runner group to allow all private repositories, no public ones, and all workflows.".into()
            }
            NoChange => "Nothing to do.".into(),
        }
    }

    /// A Runner checks result. The fingerprint is the check ID and status.
    pub(super) fn doctor(check: &DoctorCheck) -> Advice {
        let (base, name) = check.check.split_once(':').unwrap_or((&check.check, ""));
        let action = match base {
            "ssh" => ssh(&check.detail).action,
            "prerequisite" => format!("Install {name} on the machine, then run Runner checks again."),
            "disk" => "Free space on the machine's home volume.".into(),
            "power" => match name {
                "ac-sleep" => "Run `powercfg /change standby-timeout-ac 0` on that PC.".into(),
                "ac-display" => "Run `powercfg /change monitor-timeout-ac 0` on that PC.".into(),
                "ac-lid" => "Set the lid close action on AC power to Do nothing.".into(),
                "connected-standby" => {
                    "Turn on network connectivity in standby on AC power, then restart the machine.".into()
                }
                _ => "Turn off automatic sleep on AC power, and turn on restart after power failure.".into(),
            },
            "service-mode" => "Keep the runner user logged in, for example with automatic login.".into(),
            "sudo" => "Allow non-interactive sudo for the SSH account, or do the install steps by hand.".into(),
            "administrator" => "Use an administrator account for SSH on this machine.".into(),
            "service-identity" => "Check that the service account's profile and certificates are in place.".into(),
            "autologon" => "Set up automatic logon for the runner user with Sysinternals Autologon.".into(),
            "session" => format!("Sign in as {name} on the machine so its logon-task runners start."),
            "runner" if check.status == crate::desired::model::CheckStatus::Warn => {
                format!("Select Check now, then configure {name} as its step under Changes says.")
            }
            "runner" => format!("Start {name}'s service, or restart it from its runner pane."),
            "legacy" => format!("Check {name}'s service. It stays until its replacement is online."),
            "decommission" => format!("Run the decommission command in the detail to remove {name}."),
            "runner-probe" => {
                "Check that the runner folders can be read over SSH, then run Runner checks again.".into()
            }
            "github-auth" => "Fix GitHub access as the detail says, then run Runner checks again.".into(),
            "inventory" => "Fix the inventory file, then run Runner checks again.".into(),
            _ => "Fix what the detail names on the machine, then run Runner checks again.".into(),
        };
        advice(action, format!("{}:{}", check.check, check.status.as_str()))
    }

    /// A queued job no online runner can take. The fingerprint is the stuck reason, so a job
    /// that waits for a new reason comes back. The ID already names the job.
    pub(super) fn queue(reason: Option<QueueStuckReason>) -> Advice {
        use QueueStuckReason::*;
        let (action, kind) = match reason {
            None => (
                "Check that the organization's GitHub sign-in can read its runners, then select Check now.",
                "runners-unknown",
            ),
            Some(OtherRepository) => (
                "Register a runner with its labels to the job's repository or the organization.",
                "other-repository",
            ),
            Some(NoLabelMatch) => (
                "Give every label the job asks for to a runner its repository and workflow can use, or change the workflow's labels.",
                "no-label-match",
            ),
            Some(RunnersOffline) => (
                "Bring a matching runner online. `compatibleRunners` in /v1/queue names them.",
                "runners-offline",
            ),
            Some(WaitedPastLimit) => ("Open the job on the board to see why it waits.", "waited-past-limit"),
        };
        advice(action, kind)
    }

    /// One line per warning. The fingerprint is the kind of each warning.
    pub(super) fn runner_warnings(warnings: &[String]) -> Advice {
        let (actions, kinds): (Vec<_>, Vec<_>) = warnings
            .iter()
            .map(|w| {
                if w.starts_with("The runner is not configured") {
                    (
                        "Select Check now, then configure it as its step under Changes says.",
                        "not-configured",
                    )
                } else if w.starts_with("Runner service is") {
                    ("Start its service, or restart it from the runner pane.", "not-running")
                } else {
                    ("Check the runner on its machine.", w.as_str())
                }
            })
            .unzip();
        advice(actions.join("\n"), kinds.join(","))
    }
}

#[cfg(test)]
#[path = "issues_tests.rs"]
mod tests;
