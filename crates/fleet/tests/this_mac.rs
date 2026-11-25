#![expect(clippy::unwrap_used, reason = "tests fail at the assertion site")]
//! The inventory host that is this Mac: read locally, never over SSH, and never changed.

use std::sync::atomic::{AtomicUsize, Ordering};
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

/// The self node's tailnet IP in the fake tailnet.
const SELF_IP: &str = "100.64.0.1";

fn laptop(ssh: &str) -> String {
    host("laptop", ssh)
}

fn host(id: &str, ssh: &str) -> String {
    format!(
        "[[hosts]]\nid = \"{id}\"\nplatform = \"darwin-arm64\"\nssh = \"{ssh}\"\nroot = \"~/gh-runners/test\"\n\
         service_mode = \"launchagent\"\nlabels = [\"fleet\", \"macos-build\", \"host-{id}\"]\nrunners = []\n"
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
    fleet::this_mac::app_user()
        .expect("the OS names the app's account")
        .to_string()
}

/// A login that is never the app's own account.
fn other_user() -> String {
    format!("{}-runner", app_user())
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
    /// How many times the fake `ssh -G` ran.
    ssh_resolves: Arc<AtomicUsize>,
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
        self_names: [SELF_IP.to_string(), "laptop.demo.ts.net".into()].into(),
        local_online: Some(true),
        tailnet_name: None,
        peers: vec![peer("nSelf", "laptop"), peer("nMini", "mini")],
    }));
    let boundaries = ssh_fleet.clone();
    let config = ssh_config.clone();
    let ssh_resolves = Arc::new(AtomicUsize::new(0));
    let resolves = ssh_resolves.clone();
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
                resolves.fetch_add(1, Ordering::SeqCst);
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
        ssh_resolves,
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
    // (target, ~/.ssh/config gives another login, ~/.ssh/config `HostName`)
    let cases: [(&str, bool, Option<&str>); 5] = [
        ("ci@localhost", false, None),
        // The self node's name and its MagicDNS name.
        ("laptop", false, None),
        ("laptop.demo.ts.net", false, None),
        ("localhost", true, None),
        ("localhost", false, Some("builder.example")),
    ];
    for (target, other_login, hostname) in cases {
        let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop(target)));
        {
            let mut config = f.ssh_config.lock().unwrap();
            if other_login {
                config.user = other_user();
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
    let (host, runner) = ("laptop".to_string(), "laptop-01".to_string());
    // Apply, Retire and Decommission are off everywhere, but this Mac says why it is different.
    for request in [
        restart(),
        ChangeRequest::Apply {
            host: host.clone(),
            runner: None,
        },
        ChangeRequest::Retire {
            host: host.clone(),
            runner: runner.clone(),
        },
        ChangeRequest::Decommission { host, runner },
    ] {
        let refused = f.engine.desired_review(request).await.unwrap_err();
        assert_eq!(refused.to_string(), fleet::this_mac::CHANGE_REFUSED);
    }
    assert!(f.local.runs().is_empty(), "a refused review runs nothing");
    assert_nothing_changed(&f);
}

fn identity_error(machine: &MachineView) -> Option<String> {
    machine.desired.as_ref().and_then(|d| d.error.clone())
}

#[tokio::test]
async fn an_entry_edited_away_from_localhost_stops_being_this_mac_before_the_next_resolve() {
    let local_pair = format!(
        "{ORGANIZATION}{}{}{MINI}",
        laptop("localhost"),
        host("spare", "127.0.0.1")
    );
    let f = fixture(&local_pair);
    check_everything(&f).await;
    for id in ["laptop", "spare"] {
        let conflict = identity_error(&machine(&f.engine, id)).unwrap_or_default();
        assert!(conflict.contains("share the same machine"), "{id}: {conflict}");
    }
    let resolves = f.ssh_resolves.load(Ordering::SeqCst);
    let path = f._dir.path().join("fleet.toml");
    let edited = format!(
        "{ORGANIZATION}{}{}{MINI}",
        laptop("laptop-remote"),
        host("spare", "127.0.0.1")
    );
    std::fs::write(&path, edited).unwrap();
    // The view reloads the edited file before any new SSH resolution, drops the old route and clears the conflict.
    let laptop = machine(&f.engine, "laptop");
    let spare = machine(&f.engine, "spare");
    assert_eq!(f.ssh_resolves.load(Ordering::SeqCst), resolves);
    assert_eq!(laptop.desired.as_ref().unwrap().host.ssh_setting, "laptop-remote");
    assert!(!laptop.this_mac);
    assert!(spare.this_mac);
    assert_eq!(identity_error(&laptop), None);
    assert_eq!(identity_error(&spare), None);
}

#[tokio::test]
async fn an_entry_at_this_macs_tailnet_ip_conflicts_with_its_loopback_entry() {
    let tailnet_runner = host("tailnet", SELF_IP).replace(
        "runners = []",
        "runners = [{ name = \"tailnet-01\", directory = \"runner-01\" }]",
    );
    let f = fixture(&format!("{ORGANIZATION}{}{tailnet_runner}{MINI}", laptop("localhost")));
    {
        let mut world = f.ssh_fleet.world();
        let labels = ["fleet", "macos-build", "host-tailnet"];
        world.github.insert(
            "tailnet-01".into(),
            GithubRunner::new(7, "tailnet-01", "online", false, &labels, Some(true)),
        );
        world.local.insert(
            "tailnet-01".into(),
            LocalRunner::new("tailnet-01", "p", true, "active", Some("2.330.0")),
        );
    }
    f.engine.poll_tailscale().await;
    let review = f
        .engine
        .desired_review(ChangeRequest::Restart {
            runner: "tailnet-01".into(),
        })
        .await
        .unwrap();
    assert!(!review.ready);
    assert!(
        review.blockers.iter().any(|b| b.contains("share the same machine")),
        "{:?}",
        review.blockers
    );
    // The entry at the tailnet IP is still a remote host: it is checked over SSH, never locally.
    assert!(!machine(&f.engine, "tailnet").this_mac);
    assert!(f.ssh_fleet.world().reads.contains(&"probe tailnet".into()));
    assert!(f.local.runs().is_empty());
    let refused = f
        .engine
        .desired_execute(&review.id, ChangeSecrets::default(), None)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("blockers"), "{refused}");
    assert_nothing_changed(&f);
}

#[tokio::test]
async fn a_remote_peer_that_shares_this_macs_hostname_still_matches() {
    let f = fixture(&format!("{ORGANIZATION}{}{MINI}", laptop("localhost")));
    {
        let mut tailnet = f.tailnet.lock().unwrap();
        tailnet.peers[0].hostname = "mini".into();
        tailnet.peers[0].dns_name = Some("mini-1.demo.ts.net".into());
    }
    check_everything(&f).await;
    assert_eq!(machine(&f.engine, "mini").tailnet.unwrap().node_id, "nMini");
    assert!(machine(&f.engine, "laptop").this_mac);
}

#[tokio::test]
async fn a_host_that_becomes_this_mac_after_review_is_refused_at_execute() {
    let f = fixture(&format!("{ORGANIZATION}{LAPTOP_WITH_RUNNER}{MINI}"));
    f.ssh_config.lock().unwrap().user = other_user();
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
