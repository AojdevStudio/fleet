use std::collections::HashMap;

use super::*;
use crate::desired::inventory::parse_inventory;
use crate::desired::model::SshTransport;
use crate::desired::testing::{FakeFleet, RecordingRunner};

fn inventory(hosts: &[(&str, &str, &str)]) -> Inventory {
    let mut text = String::from(
        "[organization]\nname = \"example\"\nurl = \"https://github.com/example\"\nrunner_group = \"trusted\"\n",
    );
    for (id, platform, ssh) in hosts {
        let (root, mode) = if platform.starts_with("windows") {
            ("C:\\\\gh-runners\\\\example", "windows-service")
        } else {
            ("~/gh-runners/example", "launchagent")
        };
        text.push_str(&format!(
            "[[hosts]]\nid = \"{id}\"\nplatform = \"{platform}\"\nssh = \"{ssh}\"\nroot = \"{root}\"\n\
             service_mode = \"{mode}\"\nlabels = [\"host-{id}\"]\nrunners = []\n"
        ));
    }
    parse_inventory(&text, &HashMap::new()).unwrap()
}

/// What `ssh -G` prints for a target: its hostname and login.
fn resolved(hostname: &str, user: &str) -> SshTransportIdentity {
    SshTransportIdentity::from_config(&format!("hostname {hostname}\nuser {user}\nport 22\n")).unwrap()
}

/// `ssh -G` for every target in `hosts` keeps the written name and logs in as `ci`.
fn plain_resolution(inventory: &Inventory) -> BTreeMap<String, SshTransportIdentity> {
    inventory
        .hosts
        .iter()
        .filter_map(|h| h.ssh_target.clone())
        .map(|t| {
            let name = t.rsplit('@').next().unwrap_or(&t).to_string();
            (t, resolved(&name, "ci"))
        })
        .collect()
}

#[test]
fn this_mac_is_only_a_plain_loopback_target_that_ssh_keeps_local() {
    let mut inventory = inventory(&[
        ("localhost", "darwin-arm64", "localhost"),
        ("v4", "darwin-arm64", "127.0.0.1"),
        ("v6", "darwin-arm64", "::1"),
        // A login, even the app's own, is not this Mac.
        ("login", "darwin-arm64", "ci@localhost"),
        // Tailnet names, including this Mac's own node, are checked over SSH.
        ("magicdns", "darwin-arm64", "laptop.example.ts.net"),
        ("self-node", "darwin-arm64", "laptop"),
        ("win", "windows-x64", "127.0.0.1"),
        ("tailscale-ssh", "darwin-arm64", "localhost"),
    ]);
    inventory.hosts[7].ssh_transport = SshTransport::Tailscale;
    let mut resolution = plain_resolution(&inventory);
    let this_mac = |resolution: &BTreeMap<String, SshTransportIdentity>, user| {
        hosts(&inventory, resolution, user).into_iter().collect::<Vec<_>>()
    };
    assert_eq!(this_mac(&resolution, Some("ci")), ["localhost", "v4", "v6"]);
    // Another account, or no known account, means no host is this Mac.
    assert!(this_mac(&resolution, Some("admin")).is_empty());
    assert!(this_mac(&resolution, None).is_empty());

    // `ssh -G` decides: a `User` or `HostName` from ~/.ssh/config sends it over SSH.
    resolution.insert("localhost".into(), resolved("localhost", "runner"));
    resolution.insert("127.0.0.1".into(), resolved("builder.example", "ci"));
    assert_eq!(this_mac(&resolution, Some("ci")), ["v6"]);
    resolution.remove("::1");
    assert!(
        this_mac(&resolution, Some("ci")).is_empty(),
        "unresolved is not this Mac"
    );
}

#[test]
fn two_hosts_that_are_both_this_mac_conflict() {
    let inventory = inventory(&[
        ("a", "darwin-arm64", "localhost"),
        ("b", "darwin-arm64", "127.0.0.1"),
        ("mini", "darwin-arm64", "mini"),
    ]);
    let this_mac = hosts(&inventory, &plain_resolution(&inventory), Some("ci"));
    let conflicts = crate::fleet_plan::host_identity_conflicts(&inventory, &BTreeMap::new(), &[], &this_mac);
    assert_eq!(conflicts.into_keys().collect::<Vec<_>>(), ["a", "b"]);
}

fn routed(inventory: &Inventory) -> (LocalOrSsh, FakeFleet, Arc<RecordingRunner>) {
    let ssh = FakeFleet::default();
    let local = RecordingRunner::new(|_| "RFD\tdisk\tpass\tlocal\n".into());
    let remote = LocalOrSsh {
        ssh: Arc::new(ssh.clone()),
        local: Arc::new(RemoteClient::local(local.clone(), Duration::from_secs(5))),
        local_hosts: hosts(inventory, &plain_resolution(inventory), Some("ci")),
    };
    (remote, ssh, local)
}

#[test]
fn this_macs_host_is_read_locally_and_others_over_ssh() {
    let inventory = inventory(&[
        ("laptop", "darwin-arm64", "localhost"),
        ("mini", "darwin-arm64", "mini"),
    ]);
    let (remote, ssh, local) = routed(&inventory);
    for host in &inventory.hosts {
        remote.probe(host).unwrap();
        assert_eq!(remote.doctor(host)[0].status, crate::desired::model::CheckStatus::Pass);
    }
    assert_eq!(ssh.world().reads, ["probe mini", "doctor mini"]);
    let runs = local.runs();
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(run.command, ["/bin/bash", "-s"]);
        assert!(run.input.is_some_and(|script| !script.is_empty()));
    }
}

#[test]
fn a_runner_change_on_this_mac_is_refused_and_runs_nothing() {
    let inventory = inventory(&[("laptop", "darwin-arm64", "localhost")]);
    let (remote, ssh, local) = routed(&inventory);
    let host = &inventory.hosts[0];
    let runner = Runner {
        name: "laptop-01".into(),
        directory: "runner-01".into(),
        labels: Vec::new(),
        exclude_labels: Vec::new(),
        replaces: None,
        decommission: false,
    };
    let none = Transient::new(None);
    let refused = remote.restart(host, &runner, RunnerRole::Declared, &none).unwrap_err();
    assert_eq!(refused.0, CHANGE_REFUSED);
    assert!(remote.start(host, &runner, &none).is_err());
    assert!(remote.retire(host, &runner, "token", &none).is_err());
    assert!(remote.decommission(host, &runner, "token", &none).is_err());
    assert!(local.runs().is_empty());
    assert!(ssh.world().calls.is_empty() && ssh.world().reads.is_empty());
}

#[test]
fn the_local_runner_reads_health_with_the_ssh_probe_script() {
    let inventory = inventory(&[("laptop", "darwin-arm64", "localhost")]);
    let host = &inventory.hosts[0];
    let local = RecordingRunner::new(|_| "RFM\tos\tmacos\nRFM\tcpu\t12\nRFM_END\nRFM_RUNNERS\n".into());
    let client = RemoteClient::local(local.clone(), Duration::from_secs(5));
    let report = MachineSource::inspect(&client, host).unwrap();
    assert_eq!(report.cpu_percent, Some(12.0));
    let run = local.last().unwrap();
    assert_eq!(run.command, ["/bin/bash", "-s"]);
    assert_eq!(
        run.input.as_deref(),
        Some(crate::machine::inspection_script(host).as_str())
    );
}

#[test]
fn the_local_runner_refuses_windows_scripts() {
    let inventory = inventory(&[("win", "windows-x64", "win")]);
    let local = RecordingRunner::silent();
    let client = RemoteClient::local(local.clone(), Duration::from_secs(5));
    assert!(client.probe(&inventory.hosts[0]).is_err());
    assert!(local.runs().is_empty());
}
