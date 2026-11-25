//! Pure view behavior: data freshness, source authority, discovery and runner joins.

use crate::derive::*;
use crate::model::*;
use chrono::{DateTime, Duration, TimeZone, Utc};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap()
}

fn ago(s: i64) -> DateTime<Utc> {
    now() - Duration::seconds(s)
}

struct Fixture {
    config: AppConfig,
    obs: Observations,
    plan: crate::fleet_plan::FleetPlanInputs,
}

impl Fixture {
    fn new() -> Self {
        let mut f = Fixture {
            config: AppConfig {
                orgs: vec![OrgConfig {
                    login: "Acme".into(),
                    api_base: default_api_base(),
                    repo_scope: RepoScope::Recent { max_repos: 10 },
                    added_at: ago(9999),
                    auth: OrgAuth::GhCli,
                    kind: Default::default(),
                }],
                ..Default::default()
            },
            obs: Observations::default(),
            plan: crate::fleet_plan::FleetPlanInputs::default(),
        };
        f.obs.tailscale = Some(Timed::new(
            ago(5),
            TailnetSnapshot {
                self_node_id: Some("nSelf".into()),
                local_online: Some(true),
                tailnet_name: None,
                peers: vec![peer("nSelf", "my-mac", "macOS", true)],
            },
        ));
        f
    }

    fn gh_runners(&mut self, at: DateTime<Utc>, runners: Vec<GhRunner>) -> &mut Self {
        self.obs.github.entry("acme".into()).or_default().runners = Some(Timed::new(at, runners));
        self
    }

    fn jobs(&mut self, at: DateTime<Utc>, jobs: Vec<GhJob>) -> &mut Self {
        self.obs.github.entry("acme".into()).or_default().jobs = Some(Timed::new(
            at,
            JobScan {
                repos_scanned: vec!["web".into()],
                truncated: false,
                jobs,
            },
        ));
        self
    }

    fn view(&self) -> FleetView {
        derive(&self.config, &self.obs, &self.plan, now(), DataSource::Live)
    }
}

fn peer(id: &str, host: &str, os: &str, online: bool) -> TailnetPeer {
    TailnetPeer {
        node_id: id.into(),
        hostname: host.into(),
        dns_name: Some(format!("{host}.ts.net")),
        os: os.into(),
        ips: vec![],
        online,
        last_seen: (!online).then(|| ago(3600)),
        tags: vec![],
        ssh_host_keys: vec![],
    }
}

/// The node IDs Add machine offers, or `None` while it waits or has no inventory.
fn offered(view: &FleetView) -> Option<Vec<&str>> {
    match &view.add_machine {
        crate::add_machine::AddMachineView::Ready { devices } => {
            Some(devices.iter().map(|d| d.node_id.as_str()).collect())
        }
        _ => None,
    }
}

fn gh(id: u64, name: &str, status: &str, busy: bool, labels: &[&str]) -> GhRunner {
    GhRunner {
        id,
        name: name.into(),
        os: "macOS".into(),
        status: status.into(),
        busy,
        labels: labels.iter().map(ToString::to_string).collect(),
        ephemeral: None,
        version: None,
        runner_group_id: None,
        repository: None,
    }
}

fn job(id: u64, runner: Option<u64>, status: &str, labels: &[&str], started: Option<DateTime<Utc>>) -> GhJob {
    GhJob {
        id,
        run_id: 1,
        repo: "acme/web".into(),
        name: format!("job-{id}"),
        workflow_name: Some("CI".into()),
        status: status.into(),
        labels: labels.iter().map(ToString::to_string).collect(),
        runner_id: runner,
        runner_name: None,
        created_at: Some(ago(600 + i64::try_from(id).unwrap())),
        started_at: started,
        html_url: None,
        head_branch: None,
    }
}

const MAC: &[&str] = &["self-hosted", "macOS", "ARM64"];

#[test]
fn queue_counts_label_compatible_runners() {
    let mut f = Fixture::new();
    f.gh_runners(
        ago(10),
        vec![
            gh(1, "mac-busy", "online", true, MAC),
            gh(2, "mac-idle", "online", false, MAC),
            gh(3, "mac-off", "offline", false, MAC),
            gh(4, "linux", "online", false, &["self-hosted", "Linux"]),
        ],
    )
    .jobs(
        ago(20),
        vec![
            job(10, None, "queued", &["self-hosted", "macos"], None),
            job(11, None, "queued", &["self-hosted", "gpu"], None),
            job(12, None, "queued", &["ubuntu-latest"], None),
            job(13, None, "waiting", &["self-hosted"], None),
            job(14, Some(1), "in_progress", MAC, Some(ago(10))),
        ],
    );
    let v = f.view();
    assert_eq!(v.queue.jobs.len(), 2);
    assert_eq!(v.queue.other_queued, 1);
    assert!(v.queue.complete);
    let mac_job = v.queue.jobs.iter().find(|q| q.job.id == 10).unwrap();
    assert_eq!(mac_job.compatible_runners.len(), 3);
    assert_eq!(mac_job.compatible_online, 2);
    assert_eq!(mac_job.compatible_idle, 1);
    let gpu_job = v.queue.jobs.iter().find(|q| q.job.id == 11).unwrap();
    assert!(gpu_job.compatible_runners.is_empty());
    // Oldest first.
    assert_eq!(v.queue.jobs[0].job.id, 11);
    // A self-hosted job no online runner can take needs attention.
    assert_eq!(v.summary.attention, 1);
}

#[test]
fn queue_incomplete_without_fresh_job_data() {
    let mut f = Fixture::new();
    f.gh_runners(ago(10), vec![]);
    assert!(!f.view().queue.complete);
    f.jobs(ago(3600), vec![]);
    assert!(!f.view().queue.complete);
    f.jobs(ago(10), vec![]);
    assert!(f.view().queue.complete);
}

#[test]
fn queue_incomplete_until_every_selected_repo_is_scanned() {
    let mut f = Fixture::new();
    f.gh_runners(ago(10), vec![]).jobs(ago(10), vec![]);
    f.config.orgs[0].repo_scope = RepoScope::Selected {
        repos: vec!["Web".into()],
    };
    assert!(f.view().queue.complete, "a scan of the selected repo is complete");
    f.config.orgs[0].repo_scope = RepoScope::Selected {
        repos: vec!["web".into(), "api".into()],
    };
    assert!(
        !f.view().queue.complete,
        "a repository added since the last scan was never read"
    );
}

const JOIN_INVENTORY: &str = r#"
[organization]
name = "acme"
url = "https://github.com/acme"
runner_group = "trusted"

[[hosts]]
id = "mini"
platform = "darwin-arm64"
ssh = "builder@mini.example.ts.net"
root = "~/gh-runners/acme"
service_mode = "launchagent"
labels = ["fleet", "macos-build", "host-mini"]
runners = [
  { name = "mini-01", directory = "runner-01", replaces = { name = "Old-Mini", directory = "~/actions-runner" } },
  { name = "mini-02", directory = "runner-02" },
  { name = "mini-03", directory = "runner-03", decommission = true },
]
"#;

fn with_inventory(f: &mut Fixture) {
    let inventory = crate::desired::inventory::parse_inventory(JOIN_INVENTORY, &Default::default()).unwrap();
    f.plan.inventory = Some(Ok(inventory));
    f.obs.tailscale.as_mut().unwrap().value.peers.push(TailnetPeer {
        dns_name: Some("mini.example.ts.net".into()),
        ..peer("nMini", "Mac Mini", "macOS", true)
    });
}

#[test]
fn inventory_and_github_runners_join_their_tailnet_machine() {
    let mut f = Fixture::new();
    with_inventory(&mut f);
    f.gh_runners(
        ago(10),
        vec![
            gh(1, "mini-01", "online", true, MAC),
            // Not declared, but it carries the host's label.
            gh(2, "stray", "online", false, &["self-hosted", "host-mini"]),
            gh(3, "depot-x", "online", false, &["self-hosted"]),
        ],
    );
    let v = f.view();
    assert_eq!(v.machines.len(), 1);
    let mini = &v.machines[0];
    assert_eq!((mini.id.as_str(), mini.display_name.as_str()), ("ts:nMini", "mini"));
    let runners: Vec<(&str, &str)> = mini
        .runners
        .iter()
        .map(|r| {
            let role = match r.desired.as_ref().map(|d| &d.role) {
                Some(crate::fleet_plan::DesiredRunnerRole::Declared) => "declared",
                Some(crate::fleet_plan::DesiredRunnerRole::Undeclared) => "undeclared",
                other => panic!("unexpected role {other:?}"),
            };
            (r.name.as_str(), role)
        })
        .collect();
    // mini-01 from GitHub; mini-02 declared but nowhere yet; the stray by label.
    // The retired legacy runner and the finished decommission aren't listed.
    assert_eq!(
        runners,
        [
            ("mini-01", "declared"),
            ("mini-02", "declared"),
            ("stray", "undeclared")
        ]
    );
    assert!(matches!(mini.runners[0].activity, RunnerActivity::Busy { .. }));
    assert_eq!(mini.runners[0].key, "gh:acme:1");
    assert_eq!(mini.runners[1].key, "inv:mini:mini-02");
    // Only the pool runner stays unlinked.
    let unmatched: Vec<&str> = v.unmatched_runners.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(unmatched, ["depot-x"]);
}

#[test]
fn legacy_and_decommissioned_runners_show_while_they_still_exist() {
    let mut f = Fixture::new();
    with_inventory(&mut f);
    f.gh_runners(
        ago(10),
        vec![
            gh(1, "Old-Mini", "online", false, MAC),
            gh(2, "mini-03", "offline", false, MAC),
        ],
    );
    let v = f.view();
    let names: Vec<&str> = v.machines[0].runners.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["mini-01", "mini-02", "mini-03", "Old-Mini"]);
}

#[test]
fn without_live_github_runners_carry_the_last_checks_github_state() {
    let mut f = Fixture::new();
    with_inventory(&mut f);
    let mut snapshot = crate::desired::model::FleetSnapshot::default();
    let r = crate::desired::model::GithubRunner::new(7, "mini-01", "online", true, &["fleet"], Some(true));
    snapshot.github.insert("mini-01".into(), r);
    f.plan.snapshot = Some(snapshot);
    f.plan.checked_at = Some(ago(600));
    let v = f.view();
    let mini_01 = &v.machines[0].runners[0];
    assert_eq!(mini_01.key, "gh:acme:7", "keyed as the live entry will be");
    let g = mini_01.github.as_ref().unwrap();
    assert_eq!((g.status.as_str(), g.busy, g.stale), ("online", true, true));
    // Stale facts never become a current state.
    assert_eq!(mini_01.activity, RunnerActivity::Unknown);
    // A fresh check counts as current.
    f.plan.checked_at = Some(ago(30));
    assert!(matches!(
        f.view().machines[0].runners[0].activity,
        RunnerActivity::Busy { .. }
    ));
}

fn demo() -> (AppConfig, Observations, crate::fleet_plan::FleetPlanInputs) {
    let (config, observations) = crate::demo::dataset(now());
    (config, observations, crate::demo_desired::plan_inputs(now()))
}

#[test]
fn failed_ssh_keeps_health_and_local_facts_with_their_original_age() {
    let (config, mut obs, plan) = demo();
    let before = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let studio = before.machines.iter().find(|m| m.display_name == "studio").unwrap();
    let old_health = studio.health.clone().unwrap();
    obs.machines.get_mut("inv:studio").unwrap().last_error = Some(Timed::new(now(), "SSH timed out.".into()));
    let after = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let studio = after.machines.iter().find(|m| m.display_name == "studio").unwrap();
    assert_eq!(
        studio.health,
        Some(HealthView {
            stale: true,
            ..old_health
        })
    );
    assert_eq!(
        studio.connection,
        MachineConnection::Stale {
            checked_at: ago(6),
            reason: "SSH timed out.".into()
        }
    );
    assert!(studio.runners.iter().all(|r| r.local.as_ref().unwrap().stale));
    assert_eq!(
        studio.desired.as_ref().unwrap().error.as_deref(),
        Some("SSH timed out.")
    );
    assert_eq!(studio.runners[0].installed, Tri::Unknown);
}

#[test]
fn a_service_running_does_not_establish_busy_or_idle_when_github_is_stale() {
    let (config, mut obs, mut plan) = demo();
    obs.github.get_mut("acme-demo").unwrap().runners.as_mut().unwrap().at = ago(600);
    plan.checked_at = Some(ago(600));
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let studio = view.machines.iter().find(|m| m.display_name == "studio").unwrap();
    assert_eq!(studio.runners[0].activity, RunnerActivity::Unknown);
    assert_eq!(studio.runners[0].connected, Tri::Unknown);
    assert_eq!(studio.runners[0].installed, Tri::Yes);
}

#[test]
fn job_details_are_linked_by_github_and_long_jobs_are_not_called_stuck() {
    let (config, mut obs, plan) = demo();
    obs.github
        .get_mut("acme-demo")
        .unwrap()
        .jobs
        .as_mut()
        .unwrap()
        .value
        .jobs[0]
        .started_at = Some(ago(24 * 3600));
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let runner = view
        .machines
        .iter()
        .flat_map(|m| &m.runners)
        .find(|r| r.name == "studio-1")
        .unwrap();
    assert_eq!(runner.current_job.as_ref().unwrap().id, 9001);
    assert_eq!(
        runner.activity,
        RunnerActivity::Busy {
            since: Some(ago(24 * 3600))
        }
    );
    assert!(runner.warnings.iter().all(|w| !w.contains("stuck")));
    obs.github.get_mut("acme-demo").unwrap().jobs = None;
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let runner = view
        .machines
        .iter()
        .flat_map(|m| &m.runners)
        .find(|r| r.name == "studio-1")
        .unwrap();
    assert_eq!(runner.current_job, None);
    assert_eq!(runner.activity, RunnerActivity::Busy { since: None });
}

#[test]
fn repository_runners_sharing_an_id_stay_separate_and_keep_their_own_jobs() {
    let mut f = Fixture::new();
    let in_repo = |name: &str, repository: &str| GhRunner {
        repository: Some(repository.into()),
        ..gh(7, name, "online", true, MAC)
    };
    let org_runner = gh(7, "org-7", "online", true, MAC);
    f.gh_runners(
        ago(10),
        vec![in_repo("web-7", "Acme/web"), in_repo("api-7", "Acme/api"), org_runner],
    );
    let web_job = GhJob {
        runner_name: Some("web-7".into()),
        ..job(1, Some(7), "in_progress", MAC, Some(ago(60)))
    };
    f.jobs(ago(10), vec![web_job]);
    let view = f.view();
    let runner = |name: &str| view.unmatched_runners.iter().find(|r| r.name == name).unwrap();

    assert_eq!(runner("web-7").key, "gh:acme/web:7");
    assert_eq!(runner("api-7").key, "gh:acme/api:7");
    assert_eq!(runner("org-7").key, "gh:acme:7");
    assert_eq!(runner("web-7").current_job.as_ref().map(|j| j.id), Some(1));
    assert_eq!(runner("api-7").current_job, None, "the job ran in acme/web");
    assert_eq!(runner("org-7").current_job, None, "the job ran on web-7");
}

#[test]
fn changed_ssh_target_never_inherits_old_health() {
    let (config, obs, mut plan) = demo();
    let inv = plan.inventory.as_mut().unwrap().as_mut().unwrap();
    inv.hosts.iter_mut().find(|h| h.id == "studio").unwrap().ssh_target = Some("different-host".into());
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let studio = view.machines.iter().find(|m| m.display_name == "studio").unwrap();
    assert_eq!(studio.health, None);
    assert_eq!(studio.diagnostics, None);
    assert_eq!(studio.connection, MachineConnection::Pending);
}

#[test]
fn discovery_and_demo_cover_current_stale_offline_and_failed_ssh() {
    let (config, obs, plan) = demo();
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let kinds: Vec<_> = view.machines.iter().map(|m| &m.connection).collect();
    assert!(kinds.iter().any(|c| matches!(c, MachineConnection::Connected { .. })));
    assert!(kinds.iter().any(|c| matches!(c, MachineConnection::Stale { .. })));
    assert!(kinds.iter().any(|c| matches!(c, MachineConnection::Offline { .. })));
    assert!(kinds.iter().any(|c| matches!(c, MachineConnection::Error { .. })));
    assert_eq!(view.unmatched_runners.len(), 1);
    assert_eq!(view.queue.jobs.len(), 2);
    assert!(view.machines.iter().all(|m| m.desired.is_some()));
    let names: Vec<_> = view.machines.iter().map(|m| m.display_name.as_str()).collect();
    assert_eq!(
        names,
        ["build-linux-01", "mac-mini-02", "mac-mini-03", "studio", "win-builder"]
    );
    // Peers no inventory host claims are only offered to Add machine; a phone fits no platform.
    let crate::add_machine::AddMachineView::Ready { devices } = &view.add_machine else {
        panic!("every demo host's device is known: {:?}", view.add_machine)
    };
    let devices: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(devices, ["dev-laptop", "home-nas", "spare-ubuntu"]);
    let issues = serde_json::to_string(&view.issues).unwrap();
    assert!(devices.iter().all(|name| !issues.contains(name)), "{issues}");
}

#[test]
fn add_machine_names_the_hosts_it_waits_for() {
    use crate::add_machine::{AddMachineView, AddMachineWait};
    let (config, obs, mut plan) = demo();
    let add_machine =
        |plan: &crate::fleet_plan::FleetPlanInputs| derive(&config, &obs, plan, now(), DataSource::Demo).add_machine;
    let waiting = |plan: &crate::fleet_plan::FleetPlanInputs| match add_machine(plan) {
        AddMachineView::Waiting { hosts } => hosts,
        other => panic!("expected Add machine to wait: {other:?}"),
    };
    assert!(matches!(add_machine(&plan), AddMachineView::Ready { devices } if devices.len() == 3));
    // An unresolved OpenSSH alias may still claim a peer its written name doesn't match.
    plan.ssh_hosts.remove("mac-mini-02");
    let [AddMachineWait { host_id, reason }] = &waiting(&plan)[..] else {
        panic!("one host waits")
    };
    assert_eq!(host_id, "mac-mini-02");
    assert!(reason.contains("SSH hasn't resolved mac-mini-02"), "{reason}");
    plan.ssh_hosts
        .insert("mac-mini-02".into(), "mac-mini-02.demo.ts.net".into());
    // `tailscale ssh` doesn't use SSH resolution, so a Tailscale host never waits for it.
    plan.ssh_hosts.remove("builder@build-linux-01.demo.ts.net");
    assert!(matches!(add_machine(&plan), AddMachineView::Ready { .. }));
    // A disabled host whose `ssh = "${VAR}"` is unset still owns its device, so offers wait.
    let inventory = plan.inventory.as_mut().unwrap().as_mut().unwrap();
    let host = inventory.hosts.iter_mut().find(|h| h.id == "studio").unwrap();
    host.ssh_target = None;
    host.ssh_setting = "${STUDIO_SSH}".into();
    host.enabled = false;
    let [AddMachineWait { host_id, reason }] = &waiting(&plan)[..] else {
        panic!("one host waits")
    };
    assert_eq!(host_id, "studio");
    assert!(reason.contains("ssh = \"${STUDIO_SSH}\" is unset"), "{reason}");
    // A broken inventory says nothing about which peers are enrolled.
    plan.inventory = Some(Err("TOML: expected a table".into()));
    assert_eq!(add_machine(&plan), AddMachineView::NoInventory);
}

#[test]
fn add_machine_waits_until_a_host_names_its_device() {
    let text = r#"
[organization]
name = "acme"
url = "https://github.com/acme"
runner_group = "trusted"

[[hosts]]
id = "builder"
platform = "linux-x64"
ssh = "builder.lan"
root = "~/gh-runners/acme"
service_mode = "systemd"
labels = ["fleet", "linux-build"]
"#;
    let a = TailnetPeer {
        dns_name: Some("build-a.ts.net".into()),
        ..peer("nA", "builder", "linux", true)
    };
    let b = TailnetPeer {
        dns_name: Some("builder.ts.net".into()),
        ..peer("nB", "other", "linux", true)
    };
    // `builder.lan` names no device for sure, and the snapshot order says nothing about which
    // one LAN DNS reaches, so neither is offered. Nor is a device no name fits at all.
    let c = peer("nC", "compile-01", "linux", true);
    for order in [vec![a.clone(), b.clone()], vec![b, a], vec![c]] {
        let mut f = Fixture::new();
        f.plan.inventory = Some(Ok(crate::desired::inventory::parse_inventory(
            text,
            &Default::default(),
        )
        .unwrap()));
        f.plan.ssh_hosts.insert("builder.lan".into(), "builder.lan".into());
        f.obs.tailscale.as_mut().unwrap().value.peers.extend(order);
        let view = f.view();
        let crate::add_machine::AddMachineView::Waiting { hosts } = &view.add_machine else {
            panic!("{:?}", view.add_machine)
        };
        let [wait] = &hosts[..] else { panic!("one host waits") };
        assert_eq!(wait.host_id, "builder");
        assert!(
            wait.reason
                .contains("SSH connects to builder.lan, and no device on this tailnet has"),
            "{}",
            wait.reason
        );
    }
}

#[test]
fn this_mac_proves_its_own_host_and_a_shared_label_proves_nothing() {
    let text = |ssh: &str, transport: &str| {
        format!(
            "[organization]\nname = \"acme\"\nurl = \"https://github.com/acme\"\nrunner_group = \"trusted\"\n\n\
             [[hosts]]\nid = \"here\"\nplatform = \"darwin-arm64\"\nssh = \"{ssh}\"\nssh_transport = \"{transport}\"\n\
             root = \"~/gh-runners/acme\"\nservice_mode = \"launchagent\"\nlabels = [\"fleet\", \"macos-build\"]\n"
        )
    };
    let view = |text: &str, peers: Vec<TailnetPeer>| {
        let mut f = Fixture::new();
        f.plan.inventory = Some(Ok(crate::desired::inventory::parse_inventory(
            text,
            &Default::default(),
        )
        .unwrap()));
        f.plan.ssh_hosts.insert("my-mac.ts.net".into(), "my-mac.ts.net".into());
        f.obs.tailscale.as_mut().unwrap().value.peers.extend(peers);
        f.view()
    };
    // The inventory lists this Mac (nSelf, my-mac.ts.net). It is proven, and it is never offered.
    // A device that only shares its short name stays eligible.
    let other = peer("nOther", "other", "linux", true);
    let twin = TailnetPeer {
        dns_name: Some("my-mac-1.ts.net".into()),
        ..peer("nTwin", "my-mac", "macOS", true)
    };
    let here = view(&text("my-mac.ts.net", "openssh"), vec![other.clone(), twin.clone()]);
    assert_eq!(offered(&here), Some(vec!["nTwin", "nOther"]));
    // Pasting the twin's entry gives two machines: this Mac by its inventory ID, the twin by its node.
    let crate::add_machine::AddMachineView::Ready { devices } = &here.add_machine else {
        unreachable!()
    };
    let entry = &devices.iter().find(|d| d.node_id == "nTwin").unwrap().inventory_entry;
    let entry = entry.replace("[[hosts]]", "[[hosts]]\nlabels = [\"fleet\", \"macos-build\"]");
    let enrolled = view(
        &format!("{}\n{entry}", text("my-mac.ts.net", "openssh")),
        vec![other.clone(), twin],
    );
    let ids: Vec<_> = enrolled.machines.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["inv:here", "ts:nTwin"]);
    // A LAN name never guesses this Mac, so its plan isn't blocked by a host that is.
    let lan = "\n[[hosts]]\nid = \"lan\"\nplatform = \"linux-x64\"\nssh = \"my-mac.lan\"\n\
               root = \"~/gh-runners/acme\"\nservice_mode = \"systemd\"\nlabels = [\"fleet\", \"linux-build\"]\n";
    let mut f = Fixture::new();
    f.plan.inventory = Some(Ok(crate::desired::inventory::parse_inventory(
        &(text("my-mac.ts.net", "openssh") + lan),
        &Default::default(),
    )
    .unwrap()));
    for target in ["my-mac.ts.net", "my-mac.lan"] {
        f.plan.ssh_hosts.insert(target.into(), target.into());
    }
    let view_lan = f.view();
    assert!(
        view_lan
            .machines
            .iter()
            .all(|m| !matches!(m.connection, MachineConnection::Error { .. })),
        "{:?}",
        view_lan.machines
    );
    // Two hosts that both reach this Mac are one machine.
    let second = "\n[[hosts]]\nid = \"also-here\"\nplatform = \"darwin-arm64\"\nssh = \"my-mac\"\n\
                  ssh_transport = \"tailscale\"\nroot = \"~/gh-runners/other\"\nservice_mode = \"launchagent\"\n\
                  labels = [\"fleet\", \"macos-build\"]\n";
    let both = view(&(text("my-mac.ts.net", "openssh") + second), vec![other.clone()]);
    assert!(
        both.machines.iter().all(
            |m| matches!(&m.connection, MachineConnection::Error { detail } if detail.contains("also-here, here"))
        ),
        "{:?}",
        both.machines
    );
    // `tailscale ssh builder` could reach either device named builder, so offers wait.
    let shared = TailnetPeer {
        dns_name: Some("builder.other.ts.net".into()),
        ..peer("nShared", "builder", "linux", true)
    };
    let mine = TailnetPeer {
        dns_name: Some("builder.mine.ts.net".into()),
        ..peer("nMine", "builder", "linux", true)
    };
    let ambiguous = view(&text("builder", "tailscale"), vec![shared, mine, other]);
    let crate::add_machine::AddMachineView::Waiting { hosts } = &ambiguous.add_machine else {
        panic!("{:?}", ambiguous.add_machine)
    };
    assert!(
        hosts[0].reason.contains("names more than one device"),
        "{}",
        hosts[0].reason
    );
}

#[test]
fn a_tailscale_host_claims_the_peer_tailscale_ssh_reaches() {
    let mut f = Fixture::new();
    let text = r#"
[organization]
name = "acme"
url = "https://github.com/acme"
runner_group = "trusted"

[[hosts]]
id = "alpha"
platform = "linux-x64"
ssh = "builder@alpha"
ssh_transport = "tailscale"
root = "~/gh-runners/acme"
service_mode = "systemd"
labels = ["fleet", "linux-build"]
runners = [{ name = "alpha-01", directory = "runner-01" }]
"#;
    let inventory = crate::desired::inventory::parse_inventory(text, &Default::default()).unwrap();
    f.plan.inventory = Some(Ok(inventory));
    // OpenSSH maps `alpha` to a different peer, but `tailscale ssh` resolves it through MagicDNS.
    f.plan.ssh_hosts.insert("builder@alpha".into(), "beta.ts.net".into());
    // A device reporting the same hostname comes first, but MagicDNS names `alpha` only once.
    let peers = &mut f.obs.tailscale.as_mut().unwrap().value.peers;
    peers.push(TailnetPeer {
        dns_name: Some("alpha-1.ts.net".into()),
        ..peer("nAlphaTwin", "alpha", "linux", true)
    });
    peers.push(peer("nAlpha", "alpha", "linux", true));
    peers.push(peer("nBeta", "beta", "linux", true));
    let view = f.view();
    let alpha = view.machines.iter().find(|m| m.display_name == "alpha").unwrap();
    assert_eq!(alpha.id, "ts:nAlpha");
    assert_eq!(offered(&view), Some(vec!["nAlphaTwin", "nBeta"]));

    // Pasting the offered `beta` entry gives two machines, not a shared SSH destination.
    let crate::add_machine::AddMachineView::Ready { devices } = &view.add_machine else {
        unreachable!()
    };
    let beta = &devices.iter().find(|d| d.node_id == "nBeta").unwrap().inventory_entry;
    let beta = beta.replace("[[hosts]]", "[[hosts]]\nlabels = [\"fleet\", \"linux-build\"]");
    let enrolled = format!("{text}\n{beta}");
    f.plan.inventory = Some(Ok(crate::desired::inventory::parse_inventory(
        &enrolled,
        &Default::default(),
    )
    .unwrap()));
    let view = f.view();
    let ids: Vec<_> = view
        .machines
        .iter()
        .map(|m| (m.display_name.as_str(), m.id.as_str()))
        .collect();
    assert_eq!(ids, [("alpha", "ts:nAlpha"), ("beta", "ts:nBeta")]);
    assert!(
        view.machines
            .iter()
            .all(|m| m.desired.as_ref().unwrap().error.is_none())
    );
}

#[test]
fn a_newer_explicit_check_supersedes_old_poll_runner_facts_and_errors() {
    let (config, mut obs, mut plan) = demo();
    let observation = obs.machines.get_mut("inv:studio").unwrap();
    observation.last_error = Some(Timed::new(ago(5), "old failure".into()));
    plan.host_checked_at.insert("studio".into(), now());
    let runner = plan.snapshot.as_mut().unwrap().local.get_mut("studio-1").unwrap();
    runner.version = Some("2.999.0".into());
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let studio = view.machines.iter().find(|m| m.display_name == "studio").unwrap();
    assert_eq!(studio.desired.as_ref().unwrap().error, None);
    let local = studio
        .runners
        .iter()
        .find(|r| r.name == "studio-1")
        .unwrap()
        .local
        .as_ref()
        .unwrap();
    assert_eq!(
        (local.version.as_deref(), local.as_of, local.stale),
        (Some("2.999.0"), now(), false)
    );
    // The explicit runner check doesn't refresh the separate health sample.
    assert!(studio.health.as_ref().unwrap().stale);
}

#[test]
fn changing_inventory_paths_invalidates_machine_facts() {
    let (config, obs, mut plan) = demo();
    let inv = plan.inventory.as_mut().unwrap().as_mut().unwrap();
    inv.hosts.iter_mut().find(|h| h.id == "studio").unwrap().root = "~/different-runners".into();
    plan.snapshot = None; // The engine drops its snapshot when inventory content changes.
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let studio = view.machines.iter().find(|m| m.display_name == "studio").unwrap();
    assert_eq!(studio.connection, MachineConnection::Pending);
    assert!(studio.runners.iter().all(|r| r.local.is_none()));
}

#[test]
fn a_peer_label_survives_becoming_an_inventory_machine() {
    let (mut config, obs, plan) = demo();
    config.machine_preferences.insert(
        "ts:nMini".into(),
        MachinePreferences {
            display_name: Some("Office Mini".into()),
            description: Some("Under the desk".into()),
        },
    );
    let view = derive(&config, &obs, &plan, now(), DataSource::Live);
    let machine = view.machines.iter().find(|m| m.id == "ts:nMini").unwrap();
    assert_eq!(
        (machine.display_name.as_str(), machine.description.as_deref()),
        ("Office Mini", Some("Under the desk"))
    );
    assert_eq!(machine.desired.as_ref().unwrap().host.id, "mac-mini-02");
}

#[test]
fn conflicting_inventory_hosts_keep_stable_unique_identity_and_separate_runners() {
    let mut f = Fixture::new();
    let inventory_text = format!(
        "{JOIN_INVENTORY}{}",
        r#"
[[hosts]]
id = "second"
platform = "darwin-arm64"
ssh = "another-user@mini.example.ts.net"
root = "~/gh-runners/other"
service_mode = "launchagent"
labels = ["fleet", "macos-build", "host-second"]
runners = [{ name = "second-01", directory = "runner-01" }]
"#
    );
    // Both host declarations are individually valid inventory entries.
    let inventory = crate::desired::inventory::parse_inventory(&inventory_text, &Default::default()).unwrap();
    f.plan.inventory = Some(Ok(inventory));
    for target in ["builder@mini.example.ts.net", "another-user@mini.example.ts.net"] {
        f.plan.ssh_hosts.insert(target.into(), "mini.example.ts.net".into());
    }
    f.obs.tailscale.as_mut().unwrap().value.peers.push(TailnetPeer {
        dns_name: Some("mini.example.ts.net".into()),
        ..peer("nMini", "Mac Mini", "macOS", true)
    });
    f.gh_runners(
        ago(10),
        vec![
            gh(1, "mini-01", "online", false, MAC),
            gh(2, "second-01", "online", false, MAC),
            gh(3, "stray", "online", false, &["host-mini"]),
        ],
    );
    f.plan.snapshot = Some(crate::desired::model::FleetSnapshot::default());
    f.config.machine_preferences.insert(
        "ts:nMini".into(),
        MachinePreferences {
            display_name: Some("Shared peer label".into()),
            description: None,
        },
    );
    let view = f.view();
    let identities: Vec<_> = view
        .machines
        .iter()
        .map(|m| (m.id.as_str(), m.display_name.as_str()))
        .collect();
    assert_eq!(identities, [("inv:mini", "mini"), ("inv:second", "second")]);
    assert!(!offered(&view).unwrap().contains(&"nMini"));
    for machine in &view.machines {
        let MachineConnection::Error { detail } = &machine.connection else {
            panic!("conflict must be an error")
        };
        assert!(detail.contains("mini, second"));
        assert!(detail.contains("same machine or SSH destination"));
        assert_eq!(machine.desired.as_ref().unwrap().error.as_ref(), Some(detail));
        assert_eq!(machine.desired.as_ref().unwrap().pending_changes, 0);
        assert!(machine.desired.as_ref().unwrap().plan.iter().all(|p| p.blocked));
        assert_eq!(machine.health, None);
        assert_eq!(machine.tailnet, None);
        assert!(
            machine
                .runners
                .iter()
                .all(|r| r.machine_id.as_deref() == Some(machine.id.as_str()) && r.local.is_none())
        );
        assert!(
            machine
                .runners
                .iter()
                .flat_map(|r| &r.desired.as_ref().unwrap().plan)
                .all(|p| p.blocked)
        );
    }
    assert_eq!(
        view.machines[0]
            .runners
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["mini-01", "mini-02"]
    );
    assert_eq!(
        view.machines[1]
            .runners
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["second-01"]
    );
    assert_eq!(
        view.unmatched_runners
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["stray"]
    );
    f.plan.inventory.as_mut().unwrap().as_mut().unwrap().hosts.reverse();
    assert_eq!(f.view().machines, view.machines);
    // Conflicts remain actionable errors even when tailnet discovery is unavailable.
    f.obs.tailscale = None;
    let without_peers = f.view();
    assert_eq!(without_peers.machines, view.machines);
}

#[test]
fn separate_github_errors_remain_visible_and_cached_jobs_are_incomplete() {
    let mut f = Fixture::new();
    f.gh_runners(ago(10), vec![])
        .jobs(ago(10), vec![job(10, None, "queued", MAC, None)]);
    let org = f.obs.github.get_mut("acme").unwrap();
    org.error = Some(Timed::new(now(), "Runner polling failed.".into()));
    org.jobs_error = Some(Timed::new(now(), "Job polling failed.".into()));
    let view = f.view();
    assert_eq!(
        view.orgs[0].error.as_deref(),
        Some("Runners: Runner polling failed. Jobs: Job polling failed.")
    );
    assert!(!view.queue.complete);
    assert_eq!(view.queue.jobs.len(), 1, "retain the last good job scan");
    f.obs.github.get_mut("acme").unwrap().error = None;
    assert_eq!(f.view().orgs[0].error.as_deref(), Some("Job polling failed."));
    f.obs.github.get_mut("acme").unwrap().jobs_error = None;
    assert!(f.view().queue.complete);
}

#[test]
fn identity_conflicts_do_not_reuse_previously_checked_local_facts() {
    let (config, obs, mut plan) = demo();
    let before = derive(&config, &obs, &plan, now(), DataSource::Live);
    let studio = before.machines.iter().find(|m| m.display_name == "studio").unwrap();
    assert!(studio.health.is_some());
    assert!(studio.runners.iter().any(|r| r.local.is_some()));
    let inventory = plan.inventory.as_mut().unwrap().as_mut().unwrap();
    let mut duplicate = inventory.hosts.iter().find(|h| h.id == "studio").unwrap().clone();
    duplicate.id = "studio-duplicate".into();
    for runner in &mut duplicate.runners {
        runner.name = format!("duplicate-{}", runner.name);
        runner.replaces = None;
    }
    inventory.hosts.push(duplicate);
    let after = derive(&config, &obs, &plan, now(), DataSource::Live);
    let studio = after.machines.iter().find(|m| m.id == "inv:studio").unwrap();
    assert!(matches!(studio.connection, MachineConnection::Error { .. }));
    assert_eq!(studio.health, None);
    assert_eq!(studio.diagnostics, None);
    assert!(studio.runners.iter().all(|r| r.local.is_none()));
    assert!(studio.desired.as_ref().unwrap().plan.iter().all(|item| item.blocked));
}
