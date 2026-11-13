#![expect(clippy::unwrap_used, reason = "tests fail at the assertion site")]
//! runner-up pools read through a fake coordinator; nothing connects to a machine.

use std::sync::{Arc, Mutex};

use fleet::desired::model::Host;
use fleet::desired::remote::RemoteError;
use fleet::engine::{Engine, MemorySecrets};
use fleet::runner_up::{PoolSource, fixture};

const INVENTORY: &str = r#"
[organization]
name = "test"
url = "https://github.com/test"
runner_group = "test"

[runner_up]
host = "coordinator"
config = "~/.config/runner-up/config.json"
pools = ["central-linux", "central-linux-kvm", "central-macos"]

[[hosts]]
id = "coordinator"
platform = "darwin-arm64"
ssh = "ci@192.0.2.20"
root = "~/gh-runners"
service_mode = "launchagent"

[[hosts]]
id = "ci-linux"
platform = "linux-x64"
ssh = "ci@192.0.2.10"
root = "~/gh-runners"
service_mode = "systemd"
"#;

#[derive(Default)]
struct FakeCoordinator {
    fail: Mutex<bool>,
    reads: Mutex<Vec<(String, String)>>,
}

impl PoolSource for FakeCoordinator {
    fn read(&self, host: &Host, script: &str) -> Result<String, RemoteError> {
        self.reads.lock().unwrap().push((host.id.clone(), script.to_string()));
        if *self.fail.lock().unwrap() {
            return Err(RemoteError("coordinator: ssh: connect timed out".into()));
        }
        Ok(fixture::output())
    }
}

fn engine(dir: &std::path::Path, source: Arc<FakeCoordinator>) -> Engine {
    std::fs::write(dir.join("fleet.toml"), INVENTORY).unwrap();
    let engine = Engine::load(dir.join("config.json"), Arc::new(MemorySecrets::default()))
        .unwrap()
        .with_pool_source(source);
    engine
        .set_inventory_path(Some(dir.join("fleet.toml").display().to_string()))
        .unwrap();
    engine
}

#[tokio::test]
async fn pools_come_from_the_coordinator_and_survive_a_failed_read() {
    let dir = tempfile::tempdir().unwrap();
    let source = Arc::new(FakeCoordinator::default());
    let engine = engine(dir.path(), source.clone());
    assert!(engine.view().pools.is_none(), "nothing is shown before the first read");

    engine.poll_pools().await;
    let pools = engine.view().pools.unwrap();
    assert_eq!(pools.coordinator, "coordinator");
    assert_eq!(pools.pools.len(), 3);
    assert_eq!(pools.places.len(), 5);
    assert_eq!(pools.tart_vms.as_ref().unwrap().len(), 2);
    let (host, script) = source.reads.lock().unwrap()[0].clone();
    assert_eq!(host, "coordinator");
    assert!(script.contains("status --config"));
    assert!(
        script.contains("tart list"),
        "the Mac coordinator lists its tart VMs in the same read"
    );

    *source.fail.lock().unwrap() = true;
    engine.poll_pools().await;
    let failed = engine.view().pools.unwrap();
    assert!(failed.error.unwrap().contains("connect timed out"));
    assert_eq!(failed.pools, pools.pools, "the last good read stays");
    assert_eq!(failed.checked_at, pools.checked_at);
}
