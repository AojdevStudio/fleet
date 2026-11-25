use super::cleared::ClearStore;
use super::*;
use crate::desired::model::{DoctorCheck, PlanItem};
use crate::machine_diagnostics::{DiagnosticGroup, MachineDiagnostic, MachineDiagnostics};
use crate::model::MachineConnection;
use crate::monitoring::{FleetMachineAlert, FleetMonitoring};
use chrono::Utc;

/// One connected demo machine with no findings of its own.
fn one_machine() -> FleetView {
    let now = Utc::now();
    let (config, observations) = crate::demo::dataset(now);
    let mut view = crate::derive::derive(
        &config,
        &observations,
        &crate::demo_desired::plan_inputs(now),
        now,
        DataSource::Live,
    );
    view.machines.truncate(1);
    view.unmatched_runners.clear();
    view.queue.jobs.clear();
    view.monitoring = FleetMonitoring::default();
    let machine = &mut view.machines[0];
    machine.desired = None;
    machine.runners.clear();
    machine.connection = MachineConnection::Connected { checked_at: now };
    view
}

fn warn(id: &str) -> MachineDiagnostic {
    MachineDiagnostic {
        id: id.into(),
        group: DiagnosticGroup::System,
        title: id.into(),
        status: DiagnosticStatus::Warn,
        value: "warning".into(),
        detail: "evidence".into(),
    }
}

fn set_diagnostics(view: &mut FleetView, checks: Vec<MachineDiagnostic>) {
    view.machines[0].diagnostics = Some(MachineDiagnostics {
        as_of: Utc::now(),
        stale: false,
        checks,
    });
}

fn set_disk_alert(view: &mut FleetView, mounts: &[&str], detail: &str) {
    let machine = &mut view.machines[0];
    let alert = FleetMachineAlert {
        id: "disk".into(),
        machine_key: machine.preferences_key(),
        machine_name: machine.display_name.clone(),
        kind: FleetAlertKind::LowDisk,
        since: Utc::now(),
        detail: detail.into(),
        mounts: mounts.iter().map(|m| (*m).into()).collect(),
        stale: false,
        muted: false,
    };
    machine.monitoring.alerts = vec![alert.clone()];
    view.monitoring.alerts = vec![alert];
}

/// Rebuild findings with the store's clears, then let the store drop stale clears,
/// as the poller does on each tick.
fn tick(view: &mut FleetView, store: &mut ClearStore) {
    attach(view, store.cleared());
    store.record(view).unwrap();
    attach(view, store.cleared());
}

fn issue<'a>(view: &'a FleetView, id: &str) -> &'a FleetIssue {
    view.issues.iter().find(|i| i.id == id).unwrap()
}

#[test]
fn alerts_replace_duplicate_disk_and_offline_findings_but_keep_other_diagnostics() {
    let mut view = one_machine();
    set_diagnostics(&mut view, vec![warn("disk-space"), warn("sleep")]);
    set_disk_alert(&mut view, &["/"], "Low disk space");
    view.machines[0].monitoring.alerts[0].stale = true;
    view.machines[0].monitoring.alerts[0].muted = true;
    view.monitoring.alerts[0].stale = true;
    view.monitoring.alerts[0].muted = true;
    attach(&mut view, &Default::default());
    let diagnostic = view.issues.iter().find(|i| i.category == Diagnostic).unwrap();
    assert_eq!(diagnostic.detail, "sleep: warning");
    assert_eq!(view.issues.iter().filter(|i| i.category == Alert).count(), 1);
    let alert = view.issues.iter().find(|i| i.category == Alert).unwrap();
    assert!(alert.stale && alert.muted);
    assert_eq!(view.summary.attention as usize, view.issues.len());
    view.machines[0].connection = MachineConnection::Offline { last_seen: None };
    view.machines[0].monitoring.alerts[0].kind = FleetAlertKind::Offline;
    view.monitoring.alerts[0].kind = FleetAlertKind::Offline;
    attach(&mut view, &Default::default());
    assert!(
        !view
            .issues
            .iter()
            .any(|i| matches!(i.category, Diagnostic | Connection))
    );
}

#[test]
fn a_cleared_item_comes_back_when_its_problem_changes() {
    let mut view = one_machine();
    let mut store = ClearStore::in_memory(Default::default());
    set_diagnostics(&mut view, vec![warn("power-recovery")]);
    tick(&mut view, &mut store);
    let id = format!("{}:diagnostics", view.machines[0].id);
    store.clear(issue(&view, &id)).unwrap();
    tick(&mut view, &mut store);
    assert!(issue(&view, &id).cleared);
    assert_eq!(view.summary.attention, 0);

    set_diagnostics(&mut view, vec![warn("power-recovery"), warn("power-sleep")]);
    tick(&mut view, &mut store);
    assert!(!issue(&view, &id).cleared);
    // The old problem returning is a change too: the clear was dropped.
    set_diagnostics(&mut view, vec![warn("power-recovery")]);
    tick(&mut view, &mut store);
    assert!(!issue(&view, &id).cleared);
}

#[test]
fn a_new_reading_of_the_same_problem_stays_cleared() {
    let mut view = one_machine();
    let mut store = ClearStore::in_memory(Default::default());
    set_disk_alert(&mut view, &["/"], "/: 5.7% free (13.0 GiB of 228.2 GiB).");
    tick(&mut view, &mut store);
    let id = issue(&view, "alert:disk").id.clone();
    store.clear(issue(&view, &id)).unwrap();

    set_disk_alert(&mut view, &["/"], "/: 4.1% free (9.4 GiB of 228.2 GiB).");
    tick(&mut view, &mut store);
    assert!(issue(&view, &id).cleared);
    assert_eq!(view.summary.attention, 0);

    set_disk_alert(
        &mut view,
        &["/", "/data"],
        "/, /data: 4.1% free (9.4 GiB of 228.2 GiB).",
    );
    tick(&mut view, &mut store);
    assert!(!issue(&view, &id).cleared);
}

#[test]
fn a_cleared_item_comes_back_when_it_returns_after_going_away() {
    let mut view = one_machine();
    let mut store = ClearStore::in_memory(Default::default());
    set_diagnostics(&mut view, vec![warn("power-recovery")]);
    tick(&mut view, &mut store);
    let id = format!("{}:diagnostics", view.machines[0].id);
    store.clear(issue(&view, &id)).unwrap();

    set_diagnostics(&mut view, vec![]);
    tick(&mut view, &mut store);
    assert!(store.cleared().is_empty());
    set_diagnostics(&mut view, vec![warn("power-recovery")]);
    tick(&mut view, &mut store);
    assert!(!issue(&view, &id).cleared);
}

#[test]
fn missing_data_does_not_count_as_the_problem_going_away() {
    let mut view = one_machine();
    set_diagnostics(&mut view, vec![warn("power-recovery")]);
    attach(&mut view, &Default::default());
    let cleared = issue(&view, &format!("{}:diagnostics", view.machines[0].id)).clone();
    let mut store = ClearStore::in_memory([(cleared.id.clone(), cleared.fingerprint.clone())].into());

    // After a restart, before the first read: the finding is absent but was never shown.
    let mut starting = view.clone();
    starting.machines[0].diagnostics = None;
    starting.machines[0].connection = MachineConnection::Pending;
    tick(&mut starting, &mut store);
    assert_eq!(store.cleared().len(), 1);

    tick(&mut view, &mut store);
    assert!(issue(&view, &cleared.id).cleared);
    // A stale read hides the finding without a fresh read saying it is fixed.
    view.machines[0].connection = MachineConnection::Stale {
        checked_at: Utc::now(),
        reason: "The last read failed.".into(),
    };
    tick(&mut view, &mut store);
    assert_eq!(store.cleared().len(), 1);
}

#[test]
fn clears_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cleared-issues.json");
    let mut view = one_machine();
    set_disk_alert(&mut view, &["/"], "/: 5.7% free.");
    attach(&mut view, &Default::default());
    ClearStore::load(path.clone())
        .clear(issue(&view, "alert:disk"))
        .unwrap();

    let mut store = ClearStore::load(path.clone());
    tick(&mut view, &mut store);
    assert!(issue(&view, "alert:disk").cleared);

    store.restore("alert:disk").unwrap();
    tick(&mut view, &mut ClearStore::load(path));
    assert!(!issue(&view, "alert:disk").cleared);
}

#[test]
fn a_saved_file_that_cannot_be_read_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cleared-issues.json");
    std::fs::write(&path, b"not json").unwrap();
    let mut view = one_machine();
    set_disk_alert(&mut view, &["/"], "/: 5.7% free.");
    attach(&mut view, &Default::default());
    let error = ClearStore::load(path.clone())
        .clear(issue(&view, "alert:disk"))
        .unwrap_err();
    assert!(error.contains("The file was kept."), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), b"not json");
}

#[test]
fn every_finding_says_what_to_do() {
    let now = Utc::now();
    let (config, observations) = crate::demo::dataset(now);
    let demo = crate::derive::derive(
        &config,
        &observations,
        &crate::demo_desired::plan_inputs(now),
        now,
        DataSource::Demo,
    );
    let mut actions: Vec<(String, String)> = demo.issues.iter().map(|i| (i.id.clone(), i.action.clone())).collect();

    let mut view = one_machine();
    view.plan.inventory_error = Some("bad".into());
    view.plan.github_error = Some("bad".into());
    view.plan.checks_error = Some("bad".into());
    view.tailnet.error = Some("bad".into());
    view.monitoring.storage_error = Some("bad".into());
    set_disk_alert(&mut view, &["/"], "low");
    let ids = [
        "tailscale-state",
        "tailscale-unattended",
        "tailscale-startup",
        "power-sleep",
        "power-recovery",
        "power-hibernate",
        "power-idle",
        "disk-space",
        "runner:a",
        "unknown",
    ];
    set_diagnostics(&mut view, ids.iter().map(|id| warn(id)).collect());
    attach(&mut view, &Default::default());
    actions.extend(view.issues.iter().map(|i| (i.id.clone(), i.action.clone())));

    for detail in [
        "Permission denied (publickey)",
        "no host key known",
        "timed out",
        "set the environment variable referenced by X",
        "anything",
    ] {
        actions.push((detail.into(), rules::ssh(detail).action));
    }
    for check in [
        "ssh",
        "prerequisite:git",
        "disk",
        "power",
        "power:ac-sleep",
        "power:ac-display",
        "power:ac-lid",
        "power:connected-standby",
        "service-mode",
        "sudo",
        "administrator",
        "service-identity:svc",
        "autologon",
        "session:user",
        "runner:a",
        "legacy:a",
        "decommission:a",
        "runner-probe",
        "github-auth",
        "inventory",
        "unknown",
    ] {
        for status in [CheckStatus::Warn, CheckStatus::Fail] {
            let advice = rules::doctor(&DoctorCheck::new("host", check, status, "detail"));
            actions.push((check.into(), advice.action));
        }
    }
    use FleetPlanAction::*;
    for action in [
        Install,
        ReconcileLabels,
        AssignGroup,
        StartService,
        RetireLegacy,
        Decommission,
        Restart,
        Inspect,
        InspectOffline,
        InspectGroup,
        PreserveLegacy,
        PreserveService,
        PreserveDecommission,
        RepairRegistration,
        RecoverLocal,
        HardenGroup,
    ] {
        let item = PlanItem::new(action, "host", "runner", "reason");
        actions.push((action.as_str().into(), rules::plan_steps(&[&item]).action));
    }
    let warnings = [
        "The runner is not configured at its inventory path (no .runner file).".to_owned(),
        "Runner service is stopped on the machine.".to_owned(),
    ];
    actions.push(("runner warnings".into(), rules::runner_warnings(&warnings).action));
    use crate::model::QueueStuckReason::*;
    for reason in [
        None,
        Some(OtherRepository),
        Some(NoLabelMatch),
        Some(RunnersOffline),
        Some(WaitedPastLimit),
    ] {
        actions.push((format!("queue {reason:?}"), rules::queue(reason).action));
    }

    let empty: Vec<_> = actions.iter().filter(|(_, a)| a.trim().is_empty()).collect();
    assert!(empty.is_empty(), "findings with no action: {empty:?}");
    assert!(actions.len() > 60);
}

#[test]
fn the_demo_shows_one_cleared_item() {
    let now = Utc::now();
    let (config, observations) = crate::demo::dataset(now);
    let view = crate::derive::derive(
        &config,
        &observations,
        &crate::demo_desired::plan_inputs(now),
        now,
        DataSource::Demo,
    );
    let cleared: Vec<_> = view
        .issues
        .iter()
        .filter(|i| i.cleared)
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(
        cleared,
        crate::demo::cleared_issues()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert_eq!(view.summary.attention as usize, view.issues.len() - 1);
}
