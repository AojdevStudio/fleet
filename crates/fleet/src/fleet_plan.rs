//! Desired state as the UI sees it: what the inventory says about a machine
//! or runner, how that compares with the last check, and the state of checks
//! and changes. Pure data; `derive_machines` fills it in.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::desired::model::{
    DoctorCheck, FleetSnapshot, Host, Inventory, Organization, PlanItem, SshTransport, StatusRow,
};
use crate::model::TailnetPeer;

/// A change the user can review and run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChangeRequest {
    /// Install/reconcile one host's runners.
    Apply {
        host: String,
        runner: Option<String>,
    },
    /// Retire the legacy runner `runner` replaces.
    Retire {
        host: String,
        runner: String,
    },
    Decommission {
        host: String,
        runner: String,
    },
    Restart {
        runner: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum OperationStatus {
    Running,
    Succeeded,
    Failed,
}

/// The current or last change that ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    pub title: String,
    pub request: ChangeRequest,
    pub status: OperationStatus,
    pub messages: Vec<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// The last host checks (Runner Fleet's `doctor`).
#[derive(Debug, Clone, PartialEq)]
pub struct DoctorView {
    /// The one host checked, or `None` for all of them.
    pub host: Option<String>,
    pub checks: Vec<DoctorCheck>,
    pub error: Option<String>,
    pub checked_at: DateTime<Utc>,
}

/// What the inventory declares for a machine, and how it compares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MachineDesired {
    pub host: Host,
    /// Plan items for this host's runners, from the last check.
    pub plan: Vec<PlanItem>,
    /// Items an apply would carry out.
    pub pending_changes: u32,
    /// Why the last check couldn't read this host.
    pub error: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    /// This host's rows from the last host checks.
    pub checks: Vec<DoctorCheck>,
    pub checks_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DesiredRunnerRole {
    /// An inventory runner.
    Declared,
    /// The legacy runner an inventory runner replaces.
    Legacy { replaced_by: String },
    /// Declared `decommission = true`.
    Decommission,
    /// On this machine, but not in the inventory.
    Undeclared,
}

/// What the inventory says about one runner, and how it compares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RunnerDesired {
    pub host_id: String,
    pub role: DesiredRunnerRole,
    /// Labels the inventory gives it (empty unless declared).
    pub desired_labels: Vec<String>,
    /// Its row from the last check.
    pub status: Option<StatusRow>,
    /// Plan items for this runner from the last check.
    pub plan: Vec<PlanItem>,
}

/// Fleet-wide desired-state status.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FleetPlanStatus {
    pub inventory_path: String,
    pub inventory_error: Option<String>,
    pub organization: Option<Organization>,
    pub github_error: Option<String>,
    pub policy_errors: Vec<String>,
    /// Plan items that aren't about one host (runner group policy).
    pub control: Vec<PlanItem>,
    /// When GitHub was last checked; host probe times are independent.
    pub checked_at: Option<DateTime<Utc>>,
    pub refreshing: bool,
    /// Fleet-wide rows of the last host checks (GitHub auth, inventory).
    pub control_checks: Vec<DoctorCheck>,
    pub checks_at: Option<DateTime<Utc>>,
    pub checks_error: Option<String>,
    pub doctor_running: bool,
    pub operation: Option<OperationView>,
}

/// Everything `derive` needs about desired state. Not sent to the UI.
#[derive(Debug, Clone, Default)]
pub struct FleetPlanInputs {
    pub inventory_path: String,
    /// `None` until an inventory has been read.
    pub inventory: Option<Result<Inventory, String>>,
    pub snapshot: Option<FleetSnapshot>,
    /// GitHub freshness; local runner data uses `host_checked_at` exclusively.
    pub checked_at: Option<DateTime<Utc>>,
    pub host_checked_at: BTreeMap<String, DateTime<Utc>>,
    /// SSH target → the hostname SSH resolves it to.
    pub ssh_hosts: BTreeMap<String, String>,
    pub doctor: Option<DoctorView>,
    pub refreshing: bool,
    pub doctor_running: bool,
    pub operation: Option<OperationView>,
}

/// The tailnet machine an inventory host's SSH target names. `ssh_hostname`
/// is what SSH resolves the target to (`ssh -G`), which matters for aliases
/// like `mini` → `mac-mini.<tailnet>.ts.net`. `tailscale ssh` never reads the
/// OpenSSH config, so a Tailscale host is the peer its written name names.
/// The last resort is a guess by the name's first part, in snapshot order.
pub fn peer_for_host<'a>(host: &Host, ssh_hostname: Option<&str>, peers: &'a [TailnetPeer]) -> Option<&'a TailnetPeer> {
    let written = written_hostname(host)?;
    transport_hostname(host, ssh_hostname)
        .into_iter()
        .chain([written])
        .map(normalized)
        .find_map(|name| {
            named_exactly(&name, host.ssh_transport, peers)
                .first()
                .copied()
                .or_else(|| {
                    let short = name.split('.').next().unwrap_or(&name);
                    peers.iter().find(|p| {
                        label(p).is_some_and(|l| l.eq_ignore_ascii_case(short))
                            || p.hostname.eq_ignore_ascii_case(short)
                    })
                })
        })
}

/// The tailnet machines the name the host's transport connects to names exactly: a full
/// MagicDNS name or a tailnet IP, or a bare MagicDNS label for `tailscale ssh`. Exactly one
/// proves the host's device. The guesses `peer_for_host` falls back to don't count, so Add
/// machine can't duplicate a host.
pub fn proof_candidates<'a>(host: &Host, ssh_hostname: Option<&str>, peers: &'a [TailnetPeer]) -> Vec<&'a TailnetPeer> {
    connected_hostname(host, ssh_hostname).map_or_else(Vec::new, |name| named_exactly(&name, host.ssh_transport, peers))
}

/// The normalized name the host's transport connects to.
pub fn connected_hostname(host: &Host, ssh_hostname: Option<&str>) -> Option<String> {
    transport_hostname(host, ssh_hostname)
        .or_else(|| written_hostname(host))
        .map(normalized)
}

fn written_hostname(host: &Host) -> Option<&str> {
    let target = host.ssh_target.as_deref()?;
    target.rsplit('@').next()
}

/// The resolved hostname the host's transport connects to. Only OpenSSH reads `ssh -G`.
fn transport_hostname<'a>(host: &Host, ssh_hostname: Option<&'a str>) -> Option<&'a str> {
    match host.ssh_transport {
        SshTransport::Openssh => ssh_hostname,
        SshTransport::Tailscale => None,
    }
}

/// The host part, lowercase, without a final DNS dot, and an IP address in its canonical spelling.
fn normalized(name: &str) -> String {
    let name = name.rsplit('@').next().unwrap_or(name);
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().map_or(name.clone(), |ip| ip.to_string())
}

fn label(peer: &TailnetPeer) -> Option<&str> {
    peer.dns_name.as_deref().and_then(|d| d.split('.').next())
}

/// OpenSSH may resolve a bare name through the hosts file or LAN DNS, so only `tailscale ssh`
/// proves a device by its bare MagicDNS label.
/// A shared device from another tailnet can carry the same label, so a label can name several.
fn named_exactly<'a>(name: &str, transport: SshTransport, peers: &'a [TailnetPeer]) -> Vec<&'a TailnetPeer> {
    let ip = name.parse::<IpAddr>().ok();
    let bare_label = transport == SshTransport::Tailscale && !name.contains('.');
    peers
        .iter()
        .filter(|p| {
            p.dns_name.as_deref().is_some_and(|d| normalized(d) == name)
                || ip.is_some_and(|ip| p.ips.iter().any(|i| i.parse::<IpAddr>().ok() == Some(ip)))
                || (bare_label && label(p).is_some_and(|l| l.eq_ignore_ascii_case(name)))
        })
        .collect()
}

/// Hosts that share a tailnet peer or SSH destination must not own independent execution targets.
pub fn host_identity_conflicts(
    inventory: &Inventory,
    ssh_hosts: &BTreeMap<String, String>,
    peers: &[TailnetPeer],
) -> BTreeMap<String, String> {
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for host in &inventory.hosts {
        let resolved = transport_hostname(
            host,
            host.ssh_target
                .as_ref()
                .and_then(|target| ssh_hosts.get(target))
                .map(String::as_str),
        );
        if let Some(peer) = peer_for_host(host, resolved, peers) {
            owners
                .entry(format!("peer:{}", peer.node_id))
                .or_default()
                .insert(host.id.clone());
        }
        // Both the written name and the OpenSSH resolution count, so two hosts that may reach
        // one machine are blocked rather than trusted to a name guess.
        for hostname in written_hostname(host).into_iter().chain(resolved).map(normalized) {
            owners
                .entry(format!("ssh:{hostname}"))
                .or_default()
                .insert(host.id.clone());
        }
    }
    let mut conflicts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ids in owners.values().filter(|ids| ids.len() > 1) {
        for id in ids {
            conflicts.entry(id.clone()).or_default().extend(ids.iter().cloned());
        }
    }
    conflicts.into_iter().map(|(id, owners)| {
        let hosts = owners.into_iter().collect::<Vec<_>>().join(", ");
        (id, format!("Inventory hosts {hosts} share the same machine or SSH destination. Give each machine one inventory host before making changes."))
    }).collect()
}
