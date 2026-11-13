use super::*;

fn runner_up(pools: &[&str]) -> RunnerUp {
    RunnerUp {
        host: "coordinator".into(),
        config: "~/.config/runner-up/config.json".into(),
        program: "runner-up".into(),
        tart_host: None,
        pools: pools.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn parses_place_waiting_and_starting_lines() {
    let status = parse_status(fixture::STATUS);
    assert_eq!(status.places.len(), 5);
    assert_eq!(
        status.places[0],
        PlaceStatus {
            id: "ci-linux-1".into(),
            machine: "ci-linux".into(),
            provider: "docker".into(),
            cores: 4,
            memory_mb: 8192,
            state: PlaceState::Busy,
            detail: "busy (job): example/app \"test / linux\" running".into(),
        }
    );
    let states: Vec<PlaceState> = status.places.iter().map(|p| p.state).collect();
    assert_eq!(
        states,
        [
            PlaceState::Busy,
            PlaceState::Free,
            PlaceState::Free,
            PlaceState::Busy,
            PlaceState::Resting
        ]
    );
    assert_eq!(
        status.waiting,
        vec![
            PoolJob {
                repo: Some("example/app".into()),
                pool: "central-linux".into(),
                name: "lint".into(),
                place: None
            },
            PoolJob {
                repo: None,
                pool: "central-macos".into(),
                name: "native check".into(),
                place: None
            },
        ]
    );
    assert_eq!(status.starting[0].place.as_deref(), Some("ci-kvm-1"));
    assert_eq!(status.starting[0].pool, "central-linux-kvm");
}

#[test]
fn drained_and_unknown_lines_are_kept_or_skipped() {
    let status = parse_status(
        "place a on m (kvm, 2 cores, 1024 MB) drained (for \"x\")\nplace b on m (kvm, 2 cores, 1024 MB) napping\nnot a status line\ndrain \"x\" of a for \"y\"\n",
    );
    assert_eq!(status.places[0].state, PlaceState::Drained);
    assert_eq!(status.places[1].state, PlaceState::Unknown);
    assert_eq!(status.places.len(), 2);
}

#[test]
fn counts_places_jobs_and_vms_per_pool() {
    let report = parse_read(&fixture::output(), None).unwrap();
    let view = pools_view(&runner_up(&[]), &report, None, None);
    let pool = |name: &str| view.pools.iter().find(|p| p.name == name).unwrap().clone();
    assert_eq!(
        view.pools.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["central-linux", "central-linux-kvm", "central-macos"]
    );
    let linux = pool("central-linux");
    assert_eq!((linux.places, linux.free, linux.busy, linux.waiting_jobs), (2, 1, 1, 1));
    let kvm = pool("central-linux-kvm");
    assert_eq!((kvm.places, kvm.free, kvm.starting_jobs), (1, 1, 1));
    let mac = pool("central-macos");
    assert_eq!(
        (mac.places, mac.busy, mac.unavailable, mac.running_vms, mac.waiting_jobs),
        (2, 1, 1, 1, 1)
    );
    let vms = view.tart_vms.unwrap();
    assert_eq!(vms.len(), 2);
    assert_eq!(vms[0].place.as_deref(), Some("mac-1"));
    assert_eq!(vms[1].place, None);
}

#[test]
fn inventory_pools_filter_and_name_missing_pools() {
    let report = parse_read(&fixture::output(), None).unwrap();
    let view = pools_view(&runner_up(&["central-macos", "central-gpu"]), &report, None, None);
    assert_eq!(view.pools.len(), 2);
    assert_eq!(view.pools[1].name, "central-gpu");
    assert_eq!(view.pools[1].places, 0);
    assert!(
        view.places
            .iter()
            .all(|p| p.pools.contains(&"central-macos".to_string()))
    );
}

#[test]
fn tart_may_be_missing_or_read_separately() {
    let without_tart = format!("{}{CONFIG_MARKER}\n{}", fixture::STATUS, fixture::CONFIG);
    assert_eq!(parse_read(&without_tart, None).unwrap().tart, None);
    let tart_read = format!("\n{TART_MARKER}\n{}", fixture::TART);
    let report = parse_read(&without_tart, Some(&tart_read)).unwrap();
    assert_eq!(report.tart.unwrap().len(), 2);
    assert!(parse_read("runner-up: command not found", None).is_err());
}

#[test]
fn read_script_quotes_paths_and_only_reads() {
    let mut config = runner_up(&[]);
    config.config = "~/runner up/it's.json".into();
    let script = read_script(&config, true);
    assert!(script.contains(r#"'runner-up' status --config "$HOME"/'runner up/it'\''s.json' || exit $?"#));
    assert!(script.contains("tart list --format json"));
    for verb in ["serve", "drain", "borrow", "update", "rm ", "delete", "stop"] {
        assert!(!script.contains(verb), "{verb}");
    }
    assert!(!read_script(&config, false).contains("tart"));
}
