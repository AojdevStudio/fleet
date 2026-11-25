//! Add machine: the inventory entry for a tailnet device outside the inventory.
//! Built from the tailnet snapshot alone. Nothing here contacts the device.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::desired::model::{Host, Platform, SshTransport};
use crate::model::TailnetPeer;

/// Whether an inventory host that `peer_for_host` matched to no device plausibly names `peer`, so
/// Add machine leaves it out. Hosts that `peer_for_host` did match are handled by the caller: each
/// excludes its own device and nothing else, the same device its machine row shows. An offer is
/// only a paste suggestion, so for the other hosts any sign of a match is enough. The device's
/// MagicDNS name, its short name (MagicDNS label or Tailscale hostname) and its tailnet IPs are
/// compared with each host's `id`, its written `ssh` target without the user part, and the
/// hostname `ssh -G` resolved that target to, also by first DNS label. The comparison ignores case
/// and a final dot. Disabled and decommissioned hosts count too, because they are still in the
/// inventory.
///
/// Known limit: an alias that resolves to a LAN address or a LAN name unlike the device's names
/// doesn't exclude the device, so the user checks before pasting an entry.
pub(crate) fn unmatched_host_names_device<'a>(
    hosts: impl IntoIterator<Item = &'a Host>,
    ssh_hosts: &BTreeMap<String, String>,
    peer: &TailnetPeer,
) -> bool {
    let label = peer.dns_name.as_deref().and_then(|d| d.split('.').next());
    let device: HashSet<String> = [peer.dns_name.as_deref(), label, Some(peer.hostname.as_str())]
        .into_iter()
        .flatten()
        .chain(peer.ips.iter().map(String::as_str))
        .map(normalized)
        .collect();
    hosts.into_iter().any(|host| {
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

/// Whether some host's written or `ssh -G` SSH name is exactly `peer`'s MagicDNS name, one of its
/// tailnet IPs, or the `ssh` value its entry would use. Such a device's entry would repeat an SSH
/// target the inventory already has, so it is never offered, whichever device that host's machine
/// row shows.
pub(crate) fn ssh_target_reaches_device(
    hosts: &[Host],
    ssh_hosts: &BTreeMap<String, String>,
    peer: &TailnetPeer,
) -> bool {
    let address = peer.address();
    let device: HashSet<String> = peer
        .dns_name
        .as_deref()
        .into_iter()
        .chain(peer.ips.iter().map(String::as_str))
        .chain([address.as_str()])
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
fn normalized(name: &str) -> String {
    let name = name
        .rsplit('@')
        .next()
        .unwrap_or(name)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().map_or(name.clone(), |ip| ip.to_string())
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

/// `None` for a device no inventory platform fits, such as a phone. `taken` holds the host IDs
/// already in use, so a pasted entry never duplicates one; this entry's ID is added to it.
pub(crate) fn device_view(
    peer: &TailnetPeer,
    organization: &str,
    taken: &mut HashSet<String>,
) -> Option<TailnetDeviceView> {
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
        quote(&peer.address()),
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
    fn devices_no_platform_fits_are_not_offered() {
        assert!(device_view(&peer("phone", "iOS", Vec::new()), "acme", &mut HashSet::new()).is_none());
        assert!(device_view(&peer("tv", "android", Vec::new()), "acme", &mut HashSet::new()).is_none());
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
