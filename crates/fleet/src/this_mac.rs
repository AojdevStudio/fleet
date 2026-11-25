//! The Mac this app runs on, when the inventory lists it as a host.
//!
//! One rule says which host that is. The host's written `ssh` target is
//! `localhost`, `127.0.0.1` or `::1`, or the name SSH connects to (its `ssh -G`
//! hostname, else the written target) is exactly the tailnet self node's DNS
//! name, hostname or a tailnet IP. A target that names a login (`ci@...`) must
//! name the account the app runs as. A Windows host never is. The app never
//! opens SSH to that host: its health probe, runner probe, checks, runner
//! scripts and runner-up reads run as local processes with `/bin/bash -s`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use crate::desired::model::{DoctorCheck, Host, Inventory, LocalRunner, Runner, RunnerRole};
use crate::desired::process::SubprocessRunner;
use crate::desired::remote::{FleetRemote, InstallRequest, RemoteClient, RemoteError, Transient};
use crate::fleet_plan::peer_for_host;
use crate::machine::MachineSource;
use crate::model::{TailnetPeer, TailnetSnapshot};
use crate::runner_up::PoolSource;

type Result<T> = std::result::Result<T, RemoteError>;

const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
/// Limit for one local read; operations pass their own.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);

/// The account the app runs as, which local scripts run as too.
pub fn app_user() -> Option<String> {
    std::env::var("USER").ok().filter(|u| !u.is_empty())
}

/// The tailnet node this app runs on.
pub fn self_node(tailnet: &TailnetSnapshot) -> Option<&TailnetPeer> {
    let id = tailnet.self_node_id.as_deref()?;
    tailnet.peers.iter().find(|p| p.node_id == id)
}

/// Whether `host` is the Mac this app runs on. `ssh_hostname` is what SSH
/// resolves the host's target to (`ssh -G`). Names match exactly, never by
/// their first label, because a wrong match would run a script on this Mac.
pub fn is_this_mac(
    host: &Host,
    ssh_hostname: Option<&str>,
    tailnet: Option<&TailnetSnapshot>,
    app_user: Option<&str>,
) -> bool {
    let Some(target) = host.ssh_target.as_deref() else {
        return false;
    };
    let (login, written) = match target.rsplit_once('@') {
        Some((login, name)) => (Some(login), name),
        None => (None, target),
    };
    if host.is_windows() || login.is_some_and(|l| Some(l) != app_user) {
        return false;
    }
    if LOOPBACK.iter().any(|l| written.eq_ignore_ascii_case(l)) {
        return true;
    }
    let Some(node) = tailnet.and_then(self_node) else {
        return false;
    };
    let name = ssh_hostname.unwrap_or(written).trim_end_matches('.');
    name.eq_ignore_ascii_case(&node.hostname)
        || node.dns_name.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(name))
        || node.ips.iter().any(|ip| ip == name)
}

/// The tailnet node an inventory host's SSH target names. Every host that is
/// this Mac gets the self node. No other host matches the self node, even by
/// a short name, so it can't be read or changed as this Mac.
pub fn host_peer<'a>(
    host: &Host,
    ssh_hostname: Option<&str>,
    tailnet: Option<&'a TailnetSnapshot>,
    app_user: Option<&str>,
) -> Option<&'a TailnetPeer> {
    let t = tailnet?;
    if is_this_mac(host, ssh_hostname, tailnet, app_user) {
        return self_node(t);
    }
    peer_for_host(host, ssh_hostname, &t.peers).filter(|p| Some(p.node_id.as_str()) != t.self_node_id.as_deref())
}

/// The ids of the inventory hosts that are this Mac.
pub fn hosts(
    inventory: &Inventory,
    ssh_hosts: &BTreeMap<String, String>,
    tailnet: Option<&TailnetSnapshot>,
    app_user: Option<&str>,
) -> BTreeSet<String> {
    inventory
        .hosts
        .iter()
        .filter(|host| {
            let resolved = host.ssh_target.as_deref().and_then(|t| ssh_hosts.get(t));
            is_this_mac(host, resolved.map(String::as_str), tailnet, app_user)
        })
        .map(|host| host.id.clone())
        .collect()
}

/// Runs scripts on this Mac: health reads, pool reads and runner operations. Tests replace it.
pub trait LocalSource: FleetRemote + MachineSource + PoolSource {}

impl<T: FleetRemote + MachineSource + PoolSource> LocalSource for T {}

/// The real local runner.
pub fn local_client() -> RemoteClient {
    RemoteClient::local(Arc::new(SubprocessRunner), LOCAL_TIMEOUT)
}

/// Sends this Mac's hosts to the local runner and every other host over SSH.
pub struct LocalOrSsh {
    pub ssh: Arc<dyn FleetRemote>,
    pub local: Arc<dyn LocalSource>,
    /// Host ids from [`hosts`].
    pub local_hosts: BTreeSet<String>,
}

impl std::fmt::Debug for LocalOrSsh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalOrSsh")
            .field("local_hosts", &self.local_hosts)
            .finish_non_exhaustive()
    }
}

impl LocalOrSsh {
    fn pick(&self, host: &Host) -> &dyn FleetRemote {
        if self.local_hosts.contains(&host.id) {
            self.local.as_ref()
        } else {
            self.ssh.as_ref()
        }
    }
}

impl FleetRemote for LocalOrSsh {
    fn probe(&self, host: &Host) -> Result<BTreeMap<String, LocalRunner>> {
        self.pick(host).probe(host)
    }

    fn install(&self, host: &Host, runner: &Runner, request: &InstallRequest) -> Result<String> {
        self.pick(host).install(host, runner, request)
    }

    fn retire(&self, host: &Host, runner: &Runner, token: &str, sudo_password: &Transient) -> Result<String> {
        self.pick(host).retire(host, runner, token, sudo_password)
    }

    fn decommission(&self, host: &Host, runner: &Runner, token: &str, sudo_password: &Transient) -> Result<String> {
        self.pick(host).decommission(host, runner, token, sudo_password)
    }

    fn start(&self, host: &Host, runner: &Runner, sudo_password: &Transient) -> Result<String> {
        self.pick(host).start(host, runner, sudo_password)
    }

    fn restart(&self, host: &Host, runner: &Runner, role: RunnerRole, sudo_password: &Transient) -> Result<String> {
        self.pick(host).restart(host, runner, role, sudo_password)
    }

    fn logs(&self, host: &Host, runner: &Runner, role: RunnerRole, lines: u32) -> Result<String> {
        self.pick(host).logs(host, runner, role, lines)
    }

    fn doctor(&self, host: &Host) -> Vec<DoctorCheck> {
        self.pick(host).doctor(host)
    }
}

#[cfg(test)]
#[path = "this_mac_tests.rs"]
mod tests;
