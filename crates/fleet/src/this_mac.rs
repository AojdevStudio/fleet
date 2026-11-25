//! The Mac this app runs on, when the inventory lists it as a host.
//!
//! A host is this Mac only when its `ssh` target is exactly `localhost`,
//! `127.0.0.1` or `::1`, with no login, over OpenSSH, and `ssh -G` for that
//! target still connects to a loopback address as the account the app runs
//! as. Any other host, including one named like this Mac on the tailnet, is
//! checked over SSH. This Mac is only read, with local processes
//! (`/bin/bash -s`): its health probe, runner probe, checks, logs and
//! runner-up reads. The app never changes runners on it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use crate::desired::model::{DoctorCheck, Host, Inventory, LocalRunner, Runner, RunnerRole, SshTransport};
use crate::desired::process::SubprocessRunner;
use crate::desired::remote::{FleetRemote, InstallRequest, RemoteClient, RemoteError, Transient};
use crate::engine::desired::SshTransportIdentity;
use crate::machine::MachineSource;
use crate::runner_up::PoolSource;

type Result<T> = std::result::Result<T, RemoteError>;

const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
/// Limit for one local read; operations pass their own.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Why every runner change on this Mac is refused.
pub const CHANGE_REFUSED: &str = "The app doesn't change runners on the Mac it runs on. Use Runner Fleet on this Mac.";

/// The account the app runs as, which local scripts run as too.
pub fn app_user() -> Option<String> {
    std::env::var("USER").ok().filter(|u| !u.is_empty())
}

/// Whether `host` is the Mac this app runs on. `resolved` is what `ssh -G`
/// gives for the host's target. Without it the host is not this Mac.
pub fn is_this_mac(host: &Host, resolved: Option<&SshTransportIdentity>, app_user: Option<&str>) -> bool {
    let (Some(target), Some(resolved)) = (host.ssh_target.as_deref(), resolved) else {
        return false;
    };
    let hostname = resolved.hostname().trim_end_matches('.');
    host.ssh_transport == SshTransport::Openssh
        && !host.is_windows()
        && LOOPBACK.contains(&target)
        && LOOPBACK.iter().any(|l| hostname.eq_ignore_ascii_case(l))
        && app_user.is_some_and(|user| resolved.user() == Some(user))
}

/// The ids of the inventory hosts that are this Mac. `resolved` maps each
/// SSH target to its `ssh -G` result.
pub fn hosts(
    inventory: &Inventory,
    resolved: &BTreeMap<String, SshTransportIdentity>,
    app_user: Option<&str>,
) -> BTreeSet<String> {
    inventory
        .hosts
        .iter()
        .filter(|host| {
            let identity = host.ssh_target.as_ref().and_then(|t| resolved.get(t));
            is_this_mac(host, identity, app_user)
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

/// Reads this Mac's hosts with the local runner and every other host over SSH.
/// A change on this Mac is refused with [`CHANGE_REFUSED`].
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

    fn change(&self, host: &Host) -> Result<&dyn FleetRemote> {
        if self.local_hosts.contains(&host.id) {
            Err(RemoteError(CHANGE_REFUSED.into()))
        } else {
            Ok(self.ssh.as_ref())
        }
    }
}

impl FleetRemote for LocalOrSsh {
    fn probe(&self, host: &Host) -> Result<BTreeMap<String, LocalRunner>> {
        self.pick(host).probe(host)
    }

    fn install(&self, host: &Host, runner: &Runner, request: &InstallRequest) -> Result<String> {
        self.change(host)?.install(host, runner, request)
    }

    fn retire(&self, host: &Host, runner: &Runner, token: &str, sudo_password: &Transient) -> Result<String> {
        self.change(host)?.retire(host, runner, token, sudo_password)
    }

    fn decommission(&self, host: &Host, runner: &Runner, token: &str, sudo_password: &Transient) -> Result<String> {
        self.change(host)?.decommission(host, runner, token, sudo_password)
    }

    fn start(&self, host: &Host, runner: &Runner, sudo_password: &Transient) -> Result<String> {
        self.change(host)?.start(host, runner, sudo_password)
    }

    fn restart(&self, host: &Host, runner: &Runner, role: RunnerRole, sudo_password: &Transient) -> Result<String> {
        self.change(host)?.restart(host, runner, role, sudo_password)
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
