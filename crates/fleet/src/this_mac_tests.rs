use std::collections::HashMap;

use super::*;
use crate::desired::inventory::parse_inventory;
use crate::desired::testing::{FakeFleet, RecordingRunner};
use crate::model::TailnetPeer;

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

fn peer(id: &str, hostname: &str) -> TailnetPeer {
    TailnetPeer {
        node_id: id.into(),
        hostname: hostname.into(),
        dns_name: Some(format!("{hostname}.example.ts.net")),
        os: "macOS".into(),
        ips: vec![],
        online: true,
        last_seen: None,
        tags: vec![],
        ssh_host_keys: vec![],
    }
}

fn tailnet() -> TailnetSnapshot {
    TailnetSnapshot {
        self_node_id: Some("nSelf".into()),
        local_online: Some(true),
        tailnet_name: None,
        peers: vec![peer("nSelf", "laptop"), peer("nMini", "mini")],
    }
}

#[test]
fn this_mac_is_the_tailnet_self_node_or_a_loopback_target() {
    let inventory = inventory(&[
        ("laptop", "darwin-arm64", "laptop"),
        ("alias", "darwin-arm64", "me"),
        ("loop", "darwin-arm64", "ci@localhost"),
        ("v4", "darwin-arm64", "127.0.0.1"),
        ("v6", "darwin-arm64", "::1"),
        ("mini", "darwin-arm64", "mini"),
        ("win", "windows-x64", "localhost"),
        // Shares only its first label with this Mac's node, so it stays on SSH.
        ("office", "darwin-arm64", "laptop.office.example"),
        // Local scripts run as the app's account, so another login is not this Mac.
        ("other-login", "darwin-arm64", "admin@localhost"),
        // Written like this Mac, but SSH resolves it to another machine.
        ("redirected", "darwin-arm64", "laptop.example.ts.net"),
    ]);
    let ssh_hosts = BTreeMap::from([
        ("me".to_string(), "laptop.example.ts.net".to_string()),
        (
            "laptop.example.ts.net".to_string(),
            "builder.example.ts.net".to_string(),
        ),
    ]);
    let tailnet = tailnet();
    assert_eq!(
        hosts(&inventory, &ssh_hosts, Some(&tailnet), Some("ci")),
        BTreeSet::from(["alias", "laptop", "loop", "v4", "v6"].map(String::from))
    );
    // Without a tailnet read, only the loopback targets are this Mac.
    assert_eq!(
        hosts(&inventory, &ssh_hosts, None, Some("ci")),
        BTreeSet::from(["loop", "v4", "v6"].map(String::from))
    );
}

#[test]
fn only_this_macs_hosts_get_the_self_node_and_two_of_them_conflict() {
    let inventory = inventory(&[
        ("laptop", "darwin-arm64", "laptop"),
        ("office", "darwin-arm64", "laptop.office.example"),
        ("mini", "darwin-arm64", "mini"),
    ]);
    let tailnet = tailnet();
    let peer =
        |id: &str| host_peer(inventory.find_host(id).unwrap(), None, Some(&tailnet), None).map(|p| p.node_id.as_str());
    assert_eq!(peer("laptop"), Some("nSelf"));
    assert_eq!(peer("office"), None);
    assert_eq!(peer("mini"), Some("nMini"));
    let conflicts = |inventory: &Inventory| {
        crate::fleet_plan::host_identity_conflicts(inventory, &BTreeMap::new(), Some(&tailnet), None)
    };
    assert!(conflicts(&inventory).is_empty());
    let twice = self::inventory(&[
        ("laptop", "darwin-arm64", "laptop"),
        ("loop", "darwin-arm64", "localhost"),
    ]);
    assert_eq!(conflicts(&twice).into_keys().collect::<Vec<_>>(), ["laptop", "loop"]);
}

#[test]
fn this_macs_host_goes_to_the_local_runner_and_others_over_ssh() {
    let inventory = inventory(&[("laptop", "darwin-arm64", "laptop"), ("mini", "darwin-arm64", "mini")]);
    let ssh = FakeFleet::default();
    let local = RecordingRunner::new(|_| "RFD\tdisk\tpass\tlocal\n".into());
    let remote = LocalOrSsh {
        ssh: Arc::new(ssh.clone()),
        local: Arc::new(RemoteClient::local(local.clone(), Duration::from_secs(5))),
        local_hosts: hosts(&inventory, &BTreeMap::new(), Some(&tailnet()), None),
    };
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
fn the_local_runner_reads_health_with_the_ssh_probe_script() {
    let inventory = inventory(&[("laptop", "darwin-arm64", "laptop")]);
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
