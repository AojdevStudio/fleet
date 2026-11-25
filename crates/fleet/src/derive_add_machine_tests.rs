//! Add machine in the view: which tailnet devices are offered, and which a host already names.

use super::{Fixture, peer};
use crate::model::*;

const ORGANIZATION: &str =
    "[organization]\nname = \"acme\"\nurl = \"https://github.com/acme\"\nrunner_group = \"trusted\"\n";

fn host(id: &str, ssh: &str, extra: &str) -> String {
    format!(
        "[[hosts]]\nid = \"{id}\"\nplatform = \"linux-x64\"\nssh = \"{ssh}\"\n{extra}root = \"~/gh-runners/acme\"\n\
         service_mode = \"systemd\"\nlabels = [\"fleet\", \"linux-build\"]\n"
    )
}

/// A fixture whose inventory holds `hosts`, with `peers` added to the tailnet beside this Mac.
fn fixture(hosts: &[String], peers: impl IntoIterator<Item = TailnetPeer>) -> Fixture {
    let text = format!("{ORGANIZATION}\n{}", hosts.join("\n"));
    let mut f = Fixture::new();
    f.plan.inventory = Some(Ok(crate::desired::inventory::parse_inventory(
        &text,
        &Default::default(),
    )
    .unwrap()));
    f.obs.tailscale.as_mut().unwrap().value.peers.extend(peers);
    f
}

fn named(id: &str, hostname: &str, dns_name: Option<&str>, ips: &[&str]) -> TailnetPeer {
    TailnetPeer {
        dns_name: dns_name.map(Into::into),
        ips: ips.iter().map(|ip| (*ip).into()).collect(),
        ..peer(id, hostname, "linux", true)
    }
}

fn offered(view: &FleetView) -> Vec<&str> {
    view.tailnet_devices.iter().map(|d| d.node_id.as_str()).collect()
}

#[test]
fn add_machine_leaves_out_matched_devices_and_any_device_an_unmatched_host_names() {
    let mut f = fixture(
        &[
            host("alpha", "alpha-lan.home", ""),
            host("beta", "runner@Beta-Box.ts.net.", ""),
            host("gamma", "gamma-alias", ""),
            host("delta", "delta.ts.net", "enabled = false\n"),
            host("builder.site", "lan-two.home", ""),
            host("epsilon", "lan-box.home", ""),
        ],
        [
            // Its host matches no device, and the host ID names it.
            peer("nAlpha", "alpha", "linux", true),
            // Its MagicDNS name is the written SSH target.
            named("nBeta", "other-name", Some("beta-box.ts.net"), &[]),
            // Its tailnet IP is what SSH resolves the alias to.
            named("nGamma", "unrelated", Some("unrelated.ts.net"), &["100.64.0.7"]),
            // A disabled host still names its device.
            peer("nDelta", "delta", "linux", true),
            // The machine row for `epsilon` matches it by its SSH name's first label.
            peer("nLanBox", "lan-box", "linux", true),
            // A host ID is compared whole: `builder.site` doesn't name `builder`.
            peer("nBuilder", "builder", "linux", true),
            peer("nSpare", "spare", "linux", true),
        ],
    );
    f.plan.ssh_hosts.insert("gamma-alias".into(), "100.64.0.7".into());
    let view = f.view();
    assert_eq!(offered(&view), ["nBuilder", "nSpare"]);
    let epsilon = view.machines.iter().find(|m| m.display_name == "epsilon").unwrap();
    assert_eq!(epsilon.id, "ts:nLanBox");
}

#[test]
fn pasting_one_entry_keeps_a_device_with_the_same_hostname_offered() {
    let seed = host("seed", "seed.lan", "");
    let boxes = || {
        [
            named("nBox", "box", Some("box.example.ts.net"), &["100.64.0.21"]),
            named("nBox1", "box", Some("box-1.example.ts.net"), &["100.64.0.22"]),
        ]
    };
    let before = fixture(std::slice::from_ref(&seed), boxes()).view();
    assert_eq!(offered(&before), ["nBox", "nBox1"]);

    let pasted = before.tailnet_devices[0].inventory_entry.clone();
    let after = fixture(&[seed, pasted], boxes()).view();
    assert_eq!(offered(&after), ["nBox1"]);
}

#[test]
fn add_machine_and_the_machine_row_agree_when_ssh_sends_a_target_elsewhere() {
    for written in ["box.example.ts.net", "ssh://builder@box.example.ts.net:2222"] {
        let mut f = fixture(
            &[host("box", written, "")],
            [
                named("nBox", "box", Some("box.example.ts.net"), &[]),
                named("nBuild", "build", Some("build.example.ts.net"), &[]),
                peer("nSpare", "spare", "linux", true),
            ],
        );
        f.plan.ssh_hosts.insert(written.into(), "build".into());
        let view = f.view();
        let row = view.machines.iter().find(|m| m.display_name == "box").unwrap();
        assert_eq!(row.id, "ts:nBuild", "{written}");
        // `nBox` would repeat the host name the inventory's `ssh` value writes, so it isn't offered either.
        assert_eq!(offered(&view), ["nSpare"], "{written}");
    }
}

#[test]
fn an_entry_without_a_magicdns_name_uses_the_tailnet_ip_and_never_the_hostname() {
    let mut f = fixture(
        &[host("main", "box", "")],
        [
            named("nBox", "box", None, &["100.64.0.30"]),
            // Its entry would have no SSH target that names it alone.
            named("nBare", "bare", None, &[]),
            peer("nBuild", "build", "linux", true),
            peer("nSpare", "spare", "linux", true),
        ],
    );
    f.plan.ssh_hosts.insert("box".into(), "build".into());
    let view = f.view();
    let row = view.machines.iter().find(|m| m.display_name == "main").unwrap();
    assert_eq!(row.id, "ts:nBuild");
    assert_eq!(offered(&view), ["nBox", "nSpare"]);
    // `ssh = "box"` would repeat the inventory's target, so the entry uses the tailnet IP.
    assert!(
        view.tailnet_devices[0]
            .inventory_entry
            .contains("ssh = \"100.64.0.30\"\n")
    );
}

#[test]
fn add_machine_offers_neither_the_row_device_nor_the_device_the_ssh_target_names() {
    let view = fixture(
        &[host("box", "box.example.ts.net.", "")],
        [
            named("nBox1", "box", Some("box-1.example.ts.net"), &[]),
            named("nBox", "box", Some("box.example.ts.net"), &[]),
        ],
    )
    .view();
    let row = view.machines.iter().find(|m| m.display_name == "box").unwrap();
    assert!(
        ["ts:nBox", "ts:nBox1"].contains(&row.id.as_str()),
        "the box row matched no box device: {}",
        row.id
    );
    // The row's device is taken, and `nBox` would repeat `ssh = "box.example.ts.net."`.
    assert!(view.tailnet_devices.is_empty(), "{:?}", view.tailnet_devices);
}

#[test]
fn add_machine_never_offers_this_mac() {
    // The fixture's snapshot lists this Mac's own node, `nSelf`, among its peers.
    let view = fixture(
        &[host("seed", "seed.lan", "")],
        [peer("nOther", "other-mac", "macOS", true)],
    )
    .view();
    assert_eq!(offered(&view), ["nOther"]);
}
