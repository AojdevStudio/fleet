//! Pool runners in the view: linked to their runner-up place by name.

use super::{Fixture, ago, demo, gh, job, now};
use crate::model::*;

fn coordinator_read(f: &mut Fixture, read: bool) {
    f.obs.pools = Some(PoolsObservation {
        runner_up: crate::desired::model::RunnerUp {
            host: "coordinator".into(),
            config: "~/.config/runner-up/config.json".into(),
            program: "runner-up".into(),
            tart_host: None,
            pools: Vec::new(),
        },
        report: read.then(|| {
            Timed::new(
                ago(30),
                crate::runner_up::parse_read(&crate::runner_up::fixture::output(), None).unwrap(),
            )
        }),
        error: None,
    });
}

#[test]
fn pool_runners_sit_under_their_place_and_still_count() {
    let mut f = Fixture::new();
    coordinator_read(&mut f, true);
    let linux = &["self-hosted", "Linux", "central-linux"];
    f.gh_runners(
        ago(10),
        vec![
            gh(1, "runner-up-ci-linux-1-1795573483", "online", true, linux),
            gh(2, "runner-up-ci-linux-9-1795573484", "online", false, linux),
            gh(3, "cloud-gpu-1", "online", false, &["self-hosted", "gpu"]),
        ],
    )
    .jobs(ago(5), vec![job(7, Some(1), "in_progress", linux, Some(ago(60)))]);
    let v = f.view();
    let pool: Vec<(&str, Option<&str>)> = v
        .pool_runners
        .iter()
        .map(|r| (r.name.as_str(), r.place.as_deref()))
        .collect();
    assert_eq!(pool, [("runner-up-ci-linux-1-1795573483", Some("ci-linux-1"))]);
    let unlinked: Vec<&str> = v.unmatched_runners.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(unlinked, ["cloud-gpu-1", "runner-up-ci-linux-9-1795573484"]);
    assert_eq!(v.summary.runners, 3);
    assert_eq!(v.summary.runners_busy, 1);
    let busy = &v.pool_runners[0];
    assert!(matches!(busy.activity, RunnerActivity::Busy { .. }));
    assert_eq!(busy.current_job.as_ref().map(|j| j.id), Some(7));

    // Without a coordinator read there are no places, so nothing links.
    coordinator_read(&mut f, false);
    let v = f.view();
    assert!(v.pool_runners.is_empty());
    assert_eq!(v.unmatched_runners.len(), 3);
    f.obs.pools = None;
    assert!(f.view().pool_runners.is_empty());
}

#[test]
fn the_demo_has_this_mac_and_pool_runners() {
    let (config, obs, plan) = demo();
    let view = crate::derive::derive(&config, &obs, &plan, now(), DataSource::Demo);
    let this_mac: Vec<_> = view.machines.iter().filter(|m| m.this_mac).collect();
    assert_eq!(this_mac.len(), 1);
    assert_eq!(this_mac[0].display_name, "dev-laptop");
    assert!(matches!(this_mac[0].connection, MachineConnection::Connected { .. }));
    let pool: Vec<_> = view.pool_runners.iter().map(|r| r.place.as_deref()).collect();
    assert_eq!(pool, [Some("ci-kvm-1"), Some("ci-linux-1")]);
}
