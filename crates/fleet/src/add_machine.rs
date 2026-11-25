//! Add machine: the inventory entry for a tailnet device outside the inventory.
//! Built from the tailnet snapshot alone. Nothing here contacts the device.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::desired::model::{Host, Inventory, Platform, SshTransport};
use crate::model::TailnetPeer;

/// The rule for which tailnet devices Add machine offers, sorted by name, each with its entry.
/// `rows` pairs each inventory host with the device its machine row shows, if any. The caller
/// offers nothing unless Tailscale reports MagicDNS on, so every entry can use a MagicDNS name.
///
/// What it covers:
/// - `peers` never holds this Mac's own node: Tailscale reports it apart from its peers, and
///   machine assembly also drops the self node by ID. So this Mac is never offered.
/// - The device a host's machine row shows is not offered.
/// - A device whose MagicDNS name or tailnet IP equals any host's written or `ssh -G` SSH name is
///   not offered, so a pasted entry can't repeat an SSH target.
/// - For a host whose row shows no device (no match, this Mac, or an identity conflict), a device
///   it plausibly names is not offered. The device's MagicDNS name, short name (MagicDNS label or
///   Tailscale hostname) and tailnet IPs are compared with the host's `id`, its written `ssh`
///   target without the user part, and the hostname `ssh -G` resolved that target to, also by
///   first DNS label.
/// - Comparisons ignore case and a final dot. Disabled and decommissioned hosts count, because
///   they are still in the inventory.
/// - Everything else is offered. An entry's `id` is one no inventory host or other entry uses.
///
/// What it does not cover: a device in the inventory under a name unlike all of its own, such as
/// behind an alias that resolves to a LAN address, is still offered. So the user checks before
/// pasting an entry. The rule hides a device on any sign of a match, because a hidden device can
/// still be added by hand.
pub(crate) fn tailnet_devices(
    peers: &[TailnetPeer],
    inventory: &Inventory,
    ssh_hosts: &BTreeMap<String, String>,
    rows: &[(&Host, Option<&TailnetPeer>)],
) -> Vec<TailnetDeviceView> {
    let shown: HashSet<&str> = rows
        .iter()
        .filter_map(|(_, peer)| peer.map(|p| p.node_id.as_str()))
        .collect();
    let unmatched: Vec<&Host> = rows
        .iter()
        .filter(|(_, peer)| peer.is_none())
        .map(|(host, _)| *host)
        .collect();
    let mut offered: Vec<_> = peers
        .iter()
        .filter(|p| !shown.contains(p.node_id.as_str()))
        .filter(|p| !ssh_target_reaches_device(&inventory.hosts, ssh_hosts, p))
        .filter(|p| !unmatched_host_names_device(&unmatched, ssh_hosts, p))
        .collect();
    offered.sort_by_key(|p| (p.hostname.to_lowercase(), p.node_id.clone()));
    let mut taken = inventory.hosts.iter().map(|h| h.id.clone()).collect();
    offered
        .into_iter()
        .filter_map(|p| device_view(p, &inventory.organization.name, &mut taken))
        .collect()
}

/// Whether one of `hosts` names `peer` by its `id`, written SSH target or `ssh -G` hostname.
fn unmatched_host_names_device(hosts: &[&Host], ssh_hosts: &BTreeMap<String, String>, peer: &TailnetPeer) -> bool {
    let label = peer.dns_name.as_deref().and_then(|d| d.split('.').next());
    let device: HashSet<String> = [peer.dns_name.as_deref(), label, Some(peer.hostname.as_str())]
        .into_iter()
        .flatten()
        .chain(peer.ips.iter().map(String::as_str))
        .map(normalized)
        .collect();
    hosts.iter().any(|host| {
        let written = host.ssh_target.as_deref();
        let resolved = written.and_then(|t| ssh_hosts.get(t)).map(String::as_str);
        device.contains(&normalized(&host.id))
            || [written, resolved]
                .into_iter()
                .flatten()
                .map(normalized)
                .any(|name| device.contains(&name) || device.contains(first_label(&name)))
    })
}

/// Whether some host's written or `ssh -G` SSH name is exactly `peer`'s MagicDNS name or one of
/// its tailnet IPs. Its entry's `ssh` value is always one of those, whether MagicDNS is on or off.
fn ssh_target_reaches_device(hosts: &[Host], ssh_hosts: &BTreeMap<String, String>, peer: &TailnetPeer) -> bool {
    let device: HashSet<String> = peer
        .dns_name
        .as_deref()
        .into_iter()
        .chain(peer.ips.iter().map(String::as_str))
        .map(normalized)
        .collect();
    hosts.iter().any(|host| {
        let written = host.ssh_target.as_deref();
        let resolved = written.and_then(|t| ssh_hosts.get(t)).map(String::as_str);
        [written, resolved]
            .into_iter()
            .flatten()
            .any(|name| device.contains(&normalized(name)))
    })
}

/// A DNS name's first label. An IP address has none, so it stays whole.
fn first_label(name: &str) -> &str {
    if name.parse::<IpAddr>().is_ok() {
        return name;
    }
    name.split('.').next().unwrap_or(name)
}

/// The host part, lowercase, without a final dot, and an IP address in its canonical spelling.
/// An `ssh://` URI loses its scheme, user, port and path.
fn normalized(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    let (uri, name) = name.strip_prefix("ssh://").map_or((false, name.as_str()), |rest| {
        (true, rest.split('/').next().unwrap_or(rest))
    });
    let host = name.rsplit('@').next().unwrap_or(name);
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or(bracketed),
        None if uri => host.split(':').next().unwrap_or(host),
        None => host,
    }
    .trim_end_matches('.');
    host.parse::<IpAddr>()
        .map_or_else(|_| host.to_string(), |ip| ip.to_string())
}

/// A tailnet device no inventory host claims, with the `[[hosts]]` entry that adds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TailnetDeviceView {
    pub node_id: String,
    pub name: String,
    pub inventory_entry: String,
}

/// `None` for a device no inventory platform fits, such as a phone, and for a device with no `ssh`
/// target that resolves. `taken` holds the host IDs already in use, so a pasted entry never
/// duplicates one. This entry's ID is added to it.
fn device_view(peer: &TailnetPeer, organization: &str, taken: &mut HashSet<String>) -> Option<TailnetDeviceView> {
    let platform = match peer.os.to_ascii_lowercase().as_str() {
        "macos" => Platform::DarwinArm64,
        "linux" => Platform::LinuxX64,
        "windows" => Platform::WindowsX64,
        _ => return None,
    };
    let root = if platform.is_windows() {
        format!("C:\\gh-runners\\{organization}")
    } else {
        format!("~/gh-runners/{organization}")
    };
    let transport = if peer.ssh_host_keys.is_empty() {
        SshTransport::Openssh
    } else {
        SshTransport::Tailscale
    };
    let ssh = peer.dns_name.as_ref().or(peer.ips.first()).cloned()?;
    let platform_note = match platform {
        Platform::WindowsX64 => " # Tailscale doesn't report the CPU: use windows-arm64 on an ARM machine.",
        Platform::DarwinArm64 => " # Tailscale doesn't report the CPU: an Intel Mac isn't supported.",
        _ => "",
    };
    let quote = |value: &str| toml::Value::String(value.into()).to_string();
    let inventory_entry = format!(
        "[[hosts]]\nid = {}\nplatform = {}{platform_note}\nssh = {}\nssh_transport = {}\nroot = {}\nservice_mode = {}\n",
        quote(&unique_host_id(&peer.hostname, taken)),
        quote(platform.as_str()),
        quote(&ssh),
        quote(transport.as_str()),
        quote(&root),
        quote(platform.service_modes()[0].as_str()),
    );
    Some(TailnetDeviceView {
        node_id: peer.node_id.clone(),
        name: peer.hostname.clone(),
        inventory_entry,
    })
}

fn unique_host_id(hostname: &str, taken: &mut HashSet<String>) -> String {
    let base = host_id(hostname);
    let id = (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find(|id| !taken.contains(id))
        .unwrap_or(base);
    taken.insert(id.clone());
    id
}

/// A valid inventory host ID from a tailnet hostname.
fn host_id(hostname: &str) -> String {
    let id: String = hostname
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .skip_while(|c| !c.is_ascii_alphanumeric())
        .collect();
    if id.is_empty() { "machine".into() } else { id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desired::inventory::parse_inventory;
    use crate::desired::model::ServiceMode;

    const ORGANIZATION: &str = "[organization]\nname = \"acme\"\nurl = \"https://github.com/acme\"\n\n";

    fn peer(hostname: &str, os: &str, ssh_host_keys: Vec<String>) -> TailnetPeer {
        TailnetPeer {
            node_id: format!("n-{hostname}"),
            hostname: hostname.into(),
            dns_name: Some(format!("{}.example.ts.net", hostname.to_lowercase().replace(' ', "-"))),
            os: os.into(),
            ips: vec!["100.64.0.9".into()],
            online: true,
            last_seen: None,
            tags: Vec::new(),
            ssh_host_keys,
        }
    }

    #[test]
    fn each_entry_is_a_valid_inventory_host() {
        for (os, platform, mode, transport, keys) in [
            (
                "macOS",
                Platform::DarwinArm64,
                ServiceMode::Launchagent,
                SshTransport::Tailscale,
                vec!["key".into()],
            ),
            (
                "linux",
                Platform::LinuxX64,
                ServiceMode::Systemd,
                SshTransport::Openssh,
                Vec::new(),
            ),
            (
                "windows",
                Platform::WindowsX64,
                ServiceMode::WindowsService,
                SshTransport::Openssh,
                Vec::new(),
            ),
        ] {
            let device = device_view(&peer("Dev Box", os, keys), "acme", &mut HashSet::new()).unwrap();
            assert_eq!(device.name, "Dev Box");
            let inventory = parse_inventory(
                &format!("{ORGANIZATION}{}", device.inventory_entry),
                &Default::default(),
            )
            .unwrap_or_else(|e| panic!("{os}: {e}\n{}", device.inventory_entry));
            let host = &inventory.hosts[0];
            assert_eq!(host.id, "dev-box");
            assert_eq!(host.platform, platform);
            assert_eq!(host.service_mode, mode);
            assert_eq!(host.ssh_transport, transport);
            assert_eq!(host.ssh_target.as_deref(), Some("dev-box.example.ts.net"));
            assert!(host.runners.is_empty());
        }
    }

    #[test]
    fn devices_no_platform_fits_or_without_an_ssh_target_are_not_offered() {
        assert!(device_view(&peer("phone", "iOS", Vec::new()), "acme", &mut HashSet::new()).is_none());
        assert!(device_view(&peer("tv", "android", Vec::new()), "acme", &mut HashSet::new()).is_none());
        let bare = TailnetPeer {
            dns_name: None,
            ips: Vec::new(),
            ..peer("bare", "linux", Vec::new())
        };
        assert!(device_view(&bare, "acme", &mut HashSet::new()).is_none());
    }

    #[test]
    fn entries_pasted_together_into_an_inventory_keep_it_valid() {
        let example = include_str!("../../../inventory/fleet.example.toml");
        let inventory = parse_inventory(example, &Default::default()).unwrap();
        let mut taken = inventory.hosts.iter().map(|h| h.id.clone()).collect();
        // One device shares an inventory host's ID, and two share a name with each other.
        let entries: Vec<_> = [
            peer("Studio", "macOS", Vec::new()),
            peer("box", "linux", Vec::new()),
            peer("BOX", "linux", Vec::new()),
        ]
        .iter()
        .map(|p| device_view(p, "acme", &mut taken).unwrap().inventory_entry)
        .collect();
        let combined = parse_inventory(&format!("{example}\n{}", entries.join("\n")), &Default::default())
            .unwrap_or_else(|e| panic!("{e}"));
        let ids: Vec<_> = combined.hosts.iter().rev().take(3).map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["box-2", "box", "studio-2"]);
    }
}
