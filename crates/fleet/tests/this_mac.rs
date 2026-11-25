#![expect(clippy::unwrap_used, reason = "tests fail at the assertion site")]
//! The inventory host that is this Mac: read locally, never over SSH.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fleet::desired::model::Host;
use fleet::desired::remote::{RemoteClient, RemoteError};
use fleet::desired::testing::{FakeFleet, RecordingRunner, no_sleep};
use fleet::engine::desired::{DesiredIo, SshTransportIdentity};
use fleet::engine::{Engine, MemorySecrets};
use fleet::machine::{MachineReport, MachineSource, parse_inspection};
use fleet::model::{MachineConnection, MachineView, TailnetPeer, TailnetSnapshot};

const ORGANIZATION: &str = r#"
[organization]
name = "test"
url = "https://github.com/test"
runner_group = "test"
"#;

const MINI: &str = r#"
[[hosts]]
id = "mini"
platform = "darwin-arm64"
ssh = "mini"
root = "~/gh-runners/test"
service_mode = "launchagent"
labels = ["fleet", "macos-build", "host-mini"]
runners = [{ name = "mini-01", directory = "runner-01" }]
"#;

/// Output the local runner answers every script with: a doctor row, health rows and no runners.
const LOCAL_OUTPUT: &str = "RFD\tdisk\tpass\tlocal\nRFM\tos\tmacos\nRFM\tcpu\t12\nRFM\tmemory_total\t2000\n\
                            RFM\tmemory_available\t1000\nRFM_END\nRFM_RUNNERS\n";

fn laptop(ssh: &str) -> String {
    format!(
        "[[hosts]]\nid = \"laptop\"\nplatform = \"darwin-arm64\"\nssh = \"{ssh}\"\nroot = \"~/gh-runners/test\"\n\
         service_mode = \"launchagent\"\nlabels = [\"fleet\", \"macos-build\", \"host-laptop\"]\nrunners = []\n"
    )
}

/// The SSH health read; records the host ids it reaches.
#[derive(Default)]
struct FakeSsh(Mutex<Vec<String>>);

impl MachineSource for FakeSsh {
    fn inspect(&self, host: &Host) -> Result<MachineReport, RemoteError> {
        self.0.lock().unwrap().push(host.id.clone());
        parse_inspection(
            host,
            "RFM\tos\tmacos\nRFM_END\nRFM_RUNNERS\nRF1\tmini-01\t/Users/ci/gh-runners/test/runner-01\ttrue\tactive\t2.329.0\n",
        )
    }
}

fn peer(id: &str, hostname: &str) -> TailnetPeer {
    TailnetPeer {
        node_id: id.into(),
        hostname: hostname.into(),
        dns_name: Some(format!("{hostname}.demo.ts.net")),
        os: "macOS".into(),
        ips: vec![],
        online: true,
        last_seen: None,
        tags: vec![],
        ssh_host_keys: vec![],
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    ssh_health: Arc<FakeSsh>,
    ssh_fleet: FakeFleet,
    local: Arc<RecordingRunner>,
}

fn fixture(inventory: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fleet.toml");
    std::fs::write(&path, inventory).unwrap();
    let ssh_health = Arc::new(FakeSsh::default());
    let ssh_fleet = FakeFleet::default();
    let local = RecordingRunner::new(|_| LOCAL_OUTPUT.into());
    let boundaries = ssh_fleet.clone();
    let engine = Engine::load(dir.path().join("config.json"), Arc::new(MemorySecrets::default()))
        .unwrap()
        .with_machine_source(ssh_health.clone())
        .with_this_mac_source(Arc::new(RemoteClient::local(local.clone(), Duration::from_secs(5))))
        .with_desired_io(DesiredIo {
            boundaries: Arc::new(move |_| (Arc::new(boundaries.clone()) as _, Arc::new(boundaries.clone()) as _)),
            sleep: no_sleep,
            resolve_ssh_transport: Arc::new(|target| {
                SshTransportIdentity::from_config(&format!(
                    "hostname {target}\nport 22\nuser fake-ci\nproxycommand none\nproxyjump none\n"
                ))
            }),
        })
        .with_tailscale_source(|| async {
            Ok(TailnetSnapshot {
                self_node_id: Some("nSelf".into()),
                local_online: Some(true),
                tailnet_name: None,
                peers: vec![peer("nSelf", "laptop"), peer("nMini", "mini")],
            })
        });
    engine.set_inventory_path(Some(path.display().to_string())).unwrap();
    Fixture {
        _dir: dir,
        engine: Arc::new(engine),
        ssh_health,
        ssh_fleet,
        local,
    }
}

fn machine(engine: &Engine, id: &str) -> MachineView {
    engine
        .view()
        .machines
        .into_iter()
        .find(|m| m.desired.as_ref().is_some_and(|d| d.host.id == id))
        .unwrap()
}

/// Polls health, checks the fleet and runs the host checks, as the app does.
async fn check_everything(f: &Fixture) {
    f.engine.poll_tailscale().await;
    f.engine.poll_machines().await;
    f.engine.desired_refresh(None).await.unwrap();
    f.engine.desired_doctor(None).await.unwrap();
}

async fn assert_read_locally(f: &Fixture) {
    check_everything(f).await;
    assert_eq!(
        *f.ssh_health.0.lock().unwrap(),
        ["mini"],
        "only mini's health goes over SSH"
    );
    let reads = f.ssh_fleet.world().reads.clone();
    assert!(reads.contains(&"probe mini".into()) && reads.contains(&"doctor mini".into()));
    assert!(!reads.iter().any(|r| r.contains("laptop")), "{reads:?}");
    let runs = f.local.runs();
    assert!(runs.len() >= 3, "health, runner probe and checks run locally");
    assert!(runs.iter().all(|run| run.command == ["/bin/bash", "-s"]));

    let laptop = machine(&f.engine, "laptop");
    assert!(laptop.this_mac);
    assert!(matches!(laptop.connection, MachineConnection::Connected { .. }));
    assert_eq!(laptop.health.unwrap().cpu_percent, Some(12.0));
    assert_eq!(laptop.desired.unwrap().error, None);
    assert!(!machine(&f.engine, "mini").this_mac);
    let titles: Vec<String> = f.engine.view().issues.into_iter().map(|i| i.title).collect();
    assert!(
        !titles
            .iter()
            .any(|t| t.contains("couldn't be checked") || t.contains("SSH unavailable")),
        "{titles:?}"
    );
}

#[tokio::test]
async fn a_host_on_the_tailnet_self_node_is_read_locally() {
    let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop("laptop")));
    assert_read_locally(&f).await;
    assert_eq!(machine(&f.engine, "laptop").tailnet.unwrap().node_id, "nSelf");
}

#[tokio::test]
async fn a_localhost_host_is_read_locally() {
    let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop("ci@localhost")));
    assert_read_locally(&f).await;
}
