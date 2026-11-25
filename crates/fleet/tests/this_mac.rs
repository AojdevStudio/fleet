#![expect(clippy::unwrap_used, reason = "tests fail at the assertion site")]
//! The inventory host that is this Mac: read locally, never over SSH, and never changed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fleet::desired::model::{GithubRunner, Host, LocalRunner};
use fleet::desired::remote::{RemoteClient, RemoteError};
use fleet::desired::testing::{FakeFleet, RecordingRunner, no_sleep};
use fleet::engine::desired::{ChangeSecrets, DesiredIo, SshTransportIdentity};
use fleet::engine::{Engine, MemorySecrets};
use fleet::fleet_plan::ChangeRequest;
use fleet::machine::{MachineReport, MachineSource, parse_inspection};
use fleet::model::{MachineConnection, MachineView, TailnetPeer, TailnetSnapshot};
use fleet::runner_up::PoolSource;

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

const LAPTOP_WITH_RUNNER: &str = r#"
[[hosts]]
id = "laptop"
platform = "darwin-arm64"
ssh = "localhost"
root = "~/gh-runners/test"
service_mode = "launchagent"
labels = ["fleet", "macos-build", "host-laptop"]
runners = [{ name = "laptop-01", directory = "runner-01" }]
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

/// The SSH coordinator read; records the host ids it reaches.
#[derive(Default)]
struct FakePools(Mutex<Vec<String>>);

impl PoolSource for FakePools {
    fn read(&self, host: &Host, _script: &str) -> Result<String, RemoteError> {
        self.0.lock().unwrap().push(host.id.clone());
        Ok(fleet::runner_up::fixture::output())
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

fn app_user() -> String {
    fleet::this_mac::app_user().expect("tests need USER set")
}

/// What the fake `ssh -G` prints for every target: its login, and a
/// `HostName` from ~/.ssh/config when one is set.
#[derive(Clone)]
struct SshConfig {
    user: String,
    hostname: Option<String>,
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    ssh_health: Arc<FakeSsh>,
    ssh_fleet: FakeFleet,
    ssh_pools: Arc<FakePools>,
    local: Arc<RecordingRunner>,
    ssh_config: Arc<Mutex<SshConfig>>,
    tailnet: Arc<Mutex<TailnetSnapshot>>,
}

fn fixture(inventory: &str) -> Fixture {
    fixture_answering(inventory, LOCAL_OUTPUT.into())
}

/// `local_output` is what every local script prints.
fn fixture_answering(inventory: &str, local_output: String) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fleet.toml");
    std::fs::write(&path, inventory).unwrap();
    let ssh_health = Arc::new(FakeSsh::default());
    let ssh_fleet = FakeFleet::default();
    let ssh_pools = Arc::new(FakePools::default());
    let local = RecordingRunner::new(move |_| local_output.clone());
    let ssh_config = Arc::new(Mutex::new(SshConfig {
        user: app_user(),
        hostname: None,
    }));
    // The self node is named like a `laptop` target, which still doesn't make it this Mac.
    let tailnet = Arc::new(Mutex::new(TailnetSnapshot {
        self_node_id: Some("nSelf".into()),
        local_online: Some(true),
        tailnet_name: None,
        peers: vec![peer("nSelf", "laptop"), peer("nMini", "mini")],
    }));
    let boundaries = ssh_fleet.clone();
    let config = ssh_config.clone();
    let snapshot = tailnet.clone();
    let engine = Engine::load(dir.path().join("config.json"), Arc::new(MemorySecrets::default()))
        .unwrap()
        .with_machine_source(ssh_health.clone())
        .with_pool_source(ssh_pools.clone())
        .with_this_mac_source(Arc::new(RemoteClient::local(local.clone(), Duration::from_secs(5))))
        .with_desired_io(DesiredIo {
            boundaries: Arc::new(move |_| (Arc::new(boundaries.clone()) as _, Arc::new(boundaries.clone()) as _)),
            sleep: no_sleep,
            resolve_ssh_transport: Arc::new(move |target| {
                let config = config.lock().unwrap().clone();
                let written = target.rsplit('@').next().unwrap_or(target);
                let hostname = config.hostname.as_deref().unwrap_or(written);
                SshTransportIdentity::from_config(&format!(
                    "hostname {hostname}\nport 22\nuser {}\nproxycommand none\nproxyjump none\n",
                    config.user
                ))
            }),
        })
        .with_tailscale_source(move || {
            let snapshot = snapshot.lock().unwrap().clone();
            async move { Ok(snapshot) }
        });
    engine.set_inventory_path(Some(path.display().to_string())).unwrap();
    Fixture {
        _dir: dir,
        engine: Arc::new(engine),
        ssh_health,
        ssh_fleet,
        ssh_pools,
        local,
        ssh_config,
        tailnet,
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
async fn a_loopback_host_is_read_locally() {
    for target in ["localhost", "127.0.0.1", "::1"] {
        let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop(target)));
        assert_read_locally(&f).await;
    }
}

#[tokio::test]
async fn any_other_target_is_checked_over_ssh() {
    let cases: [(&str, Option<&str>, Option<&str>); 5] = [
        ("ci@localhost", None, None),
        // The self node's name and its MagicDNS name.
        ("laptop", None, None),
        ("laptop.demo.ts.net", None, None),
        // ~/.ssh/config gives localhost another login, or another machine.
        ("localhost", Some("runner"), None),
        ("localhost", None, Some("builder.example")),
    ];
    for (target, user, hostname) in cases {
        let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop(target)));
        {
            let mut config = f.ssh_config.lock().unwrap();
            if let Some(user) = user {
                config.user = user.into();
            }
            config.hostname = hostname.map(str::to_string);
        }
        check_everything(&f).await;
        assert!(!machine(&f.engine, "laptop").this_mac, "{target}");
        assert!(
            f.ssh_health.0.lock().unwrap().contains(&"laptop".to_string()),
            "{target}"
        );
        assert!(f.ssh_fleet.world().reads.contains(&"probe laptop".into()), "{target}");
        assert!(f.local.runs().is_empty(), "{target}");
    }
}

#[tokio::test]
async fn this_mac_stays_listed_and_read_when_tailscale_reports_itself_offline() {
    let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop("localhost")));
    {
        let mut tailnet = f.tailnet.lock().unwrap();
        tailnet.local_online = Some(false);
        tailnet.peers[0].online = false;
    }
    f.engine.poll_tailscale().await;
    f.engine.poll_machines().await;
    let laptop = machine(&f.engine, "laptop");
    assert!(laptop.this_mac);
    assert_eq!(laptop.tailnet, None);
    assert_eq!(laptop.health.unwrap().cpu_percent, Some(12.0));
    assert!(!f.local.runs().is_empty());
}

fn restart() -> ChangeRequest {
    ChangeRequest::Restart {
        runner: "laptop-01".into(),
    }
}

fn assert_nothing_changed(f: &Fixture) {
    let world = f.ssh_fleet.world();
    assert!(world.calls.is_empty() && world.tokens.is_empty(), "{:?}", world.calls);
}

#[tokio::test]
async fn a_runner_change_on_this_mac_is_refused() {
    let f = fixture(&format!("{ORGANIZATION}{LAPTOP_WITH_RUNNER}{MINI}"));
    let refused = f.engine.desired_review(restart()).await.unwrap_err();
    assert_eq!(refused.to_string(), fleet::this_mac::CHANGE_REFUSED);
    assert!(f.local.runs().is_empty(), "a refused review runs nothing");
    assert_nothing_changed(&f);
}

#[tokio::test]
async fn a_host_that_becomes_this_mac_after_review_is_refused_at_execute() {
    let f = fixture(&format!("{ORGANIZATION}{LAPTOP_WITH_RUNNER}{MINI}"));
    f.ssh_config.lock().unwrap().user = "runner".into();
    {
        let mut world = f.ssh_fleet.world();
        let labels = ["fleet", "macos-build", "host-laptop"];
        world.github.insert(
            "laptop-01".into(),
            GithubRunner::new(7, "laptop-01", "online", false, &labels, Some(true)),
        );
        world.local.insert(
            "laptop-01".into(),
            LocalRunner::new("laptop-01", "p", true, "active", Some("2.330.0")),
        );
    }
    let review = f.engine.desired_review(restart()).await.unwrap();
    assert!(review.ready, "{:?}", review.blockers);
    f.ssh_config.lock().unwrap().user = app_user();
    let refused = f
        .engine
        .desired_execute(&review.id, ChangeSecrets::default(), None)
        .await
        .unwrap_err();
    assert_eq!(refused.to_string(), fleet::this_mac::CHANGE_REFUSED);
    assert!(f.local.runs().is_empty());
    assert_nothing_changed(&f);
}

#[tokio::test]
async fn a_coordinator_on_this_mac_is_read_locally() {
    let inventory = format!(
        "{ORGANIZATION}[runner_up]\nhost = \"laptop\"\nconfig = \"~/.config/runner-up/config.json\"\n{}{MINI}",
        laptop("localhost")
    );
    let f = fixture_answering(&inventory, fleet::runner_up::fixture::output());
    f.engine.poll_pools().await;
    assert!(f.ssh_pools.0.lock().unwrap().is_empty());
    let run = f.local.last().unwrap();
    assert_eq!(run.command, ["/bin/bash", "-s"]);
    assert!(run.input.unwrap().contains("runner-up"));
    let pools = f.engine.view().pools.unwrap();
    assert_eq!(pools.error, None);
    assert_eq!(pools.places.len(), 5);
}
