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
                self_names: Default::default(),
                local_online: Some(true),
                tailnet_name: None,
                peers: vec![peer("nSelf", "my-mac", "macOS", true)],
            },
        ));
        f
    }

    fn gh_runners(&mut self, at: DateTime<Utc>, runners: Vec<GhRunner>) -> &mut Self {
        self.obs
            .github
            .entry("acme".into())
            .or_default()
            .set_runners(Timed::new(at, runners));
        self
    }

    fn jobs(&mut self, at: DateTime<Utc>, jobs: Vec<GhJob>) -> &mut Self {
        self.obs.github.entry("acme".into()).or_default().set_jobs(Timed::new(
            at,
            JobScan {
                started_at: at,
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
    assert_eq!(mac_job.stuck, None);
    assert_eq!(
        gpu_job.stuck.as_ref().map(|s| (s.reason, s.detail.as_str())),
        Some((
            QueueStuckReason::NoLabelMatch,
            "No runner has the labels self-hosted, gpu."
        ))
    );
    // The API keeps a self-hosted job no online runner can take as a queue finding.
    assert_eq!(v.summary.attention, 1);
}

fn in_repo(repository: &str, runner: GhRunner) -> GhRunner {
    GhRunner {
        repository: Some(repository.into()),
        ..runner
    }
}

fn created(minutes: i64, job: GhJob) -> GhJob {
    GhJob {
        created_at: Some(ago(minutes * 60)),
        ..job
    }
}

#[test]
fn a_repository_runner_takes_only_its_own_repositorys_jobs() {
    let mut f = Fixture::new();
    let homelab = &["self-hosted", "homelab-ci"];
    f.gh_runners(
        ago(10),
        vec![
            in_repo("acme/api", gh(1, "api-deps", "online", false, &["dependabot"])),
            in_repo("Acme/Web", gh(2, "web-ci", "online", false, homelab)),
            in_repo("acme/api", gh(3, "api-ci", "online", false, homelab)),
            in_repo("acme/api", gh(4, "api-ci-2", "online", false, homelab)),
        ],
    )
    .jobs(
        ago(20),
        vec![
            created(18 * 60, job(10, None, "queued", &["dependabot"], None)),
            job(11, None, "queued", homelab, None),
        ],
    );
    let v = f.view();
    let dependabot = v.queue.jobs.iter().find(|q| q.job.id == 10).unwrap();
    assert!(dependabot.compatible_runners.is_empty());
    assert_eq!(
        dependabot.stuck.as_ref().map(|s| (s.reason, s.detail.as_str())),
        Some((
            QueueStuckReason::OtherRepository,
            "Only runners registered to other repositories have its labels."
        ))
    );
    assert_eq!(
        v.queue.other_queued, 0,
        "a label match before the repository check keeps the job on the queue"
    );
    let homelab_job = v.queue.jobs.iter().find(|q| q.job.id == 11).unwrap();
    assert_eq!(
        homelab_job.compatible_runners,
        ["gh:acme/web:2"],
        "repository case is ignored"
    );
    assert_eq!((homelab_job.compatible_online, homelab_job.compatible_idle), (1, 1));
    assert_eq!(homelab_job.stuck, None);
}

#[test]
fn an_organization_runner_matches_any_repository_by_labels() {
    let mut f = Fixture::new();
    f.gh_runners(
        ago(10),
        vec![gh(1, "org-ci", "online", false, &["self-hosted", "linux"])],
    )
    .jobs(ago(20), vec![job(10, None, "queued", &["self-hosted", "Linux"], None)]);
    let v = f.view();
    assert_eq!(v.queue.jobs[0].compatible_runners, ["gh:acme:1"]);
    assert_eq!(v.queue.jobs[0].compatible_idle, 1);
    assert_eq!(v.queue.jobs[0].stuck, None);
}

#[test]
fn a_job_is_stuck_after_waiting_past_the_limit_only_while_a_matching_runner_is_idle() {
    let limit = STUCK_AFTER.num_minutes();
    let mut f = Fixture::new();
    // Two runner polls: the runner was already idle before every scan below began.
    f.gh_runners(ago(145), vec![gh(1, "mac-idle", "online", false, MAC)])
        .gh_runners(ago(10), vec![gh(1, "mac-idle", "online", false, MAC)])
        .jobs(
            ago(20),
            vec![
                created(limit - 1, job(10, None, "queued", MAC, None)),
                created(limit + 1, job(11, None, "queued", MAC, None)),
            ],
        );
    let v = f.view();
    assert_eq!(v.queue.stuck_after_secs, 30 * 60);
    let stuck = |v: &FleetView, id| v.queue.jobs.iter().find(|q| q.job.id == id).unwrap().stuck.clone();
    assert_eq!(stuck(&v, 10), None);
    assert_eq!(
        stuck(&v, 11).map(|s| (s.reason, s.detail)),
        Some((
            QueueStuckReason::WaitedPastLimit,
            "An idle runner matches it, but it has waited over 30 min.".into()
        ))
    );

    // The wait counts up to when the last job scan began, since the job may have started after it.
    f.jobs(
        ago(6 * 60),
        vec![created(limit + 4, job(12, None, "queued", MAC, None))],
    );
    assert_eq!(stuck(&f.view(), 12), None);
    f.jobs(ago(20), vec![created(limit + 4, job(12, None, "queued", MAC, None))]);
    assert_eq!(
        stuck(&f.view(), 12).map(|s| s.reason),
        Some(QueueStuckReason::WaitedPastLimit)
    );
    // A slow scan that finished past the limit but began before it.
    f.jobs(ago(20), vec![created(limit + 1, job(12, None, "queued", MAC, None))]);
    f.obs
        .github
        .get_mut("acme")
        .unwrap()
        .jobs
        .as_mut()
        .unwrap()
        .value
        .started_at = ago(140);
    assert_eq!(stuck(&f.view(), 12), None);
    f.jobs(
        ago(20),
        vec![
            created(limit - 1, job(10, None, "queued", MAC, None)),
            created(limit + 1, job(11, None, "queued", MAC, None)),
        ],
    );

    // Every matching runner is working: the job waits its turn.
    f.gh_runners(ago(10), vec![gh(1, "mac-busy", "online", true, MAC)]);
    assert_eq!(stuck(&f.view(), 11), None);
    // The runner is offline: nothing can take the job.
    f.gh_runners(ago(10), vec![gh(1, "mac-off", "offline", false, MAC)]);
    assert_eq!(
        stuck(&f.view(), 10).map(|s| s.reason),
        Some(QueueStuckReason::RunnersOffline)
    );
}

#[test]
fn a_runner_that_went_idle_after_the_job_scan_does_not_make_the_job_stuck() {
    let mut f = Fixture::new();
    let waiting = || {
        vec![created(
            STUCK_AFTER.num_minutes() + 5,
            job(10, None, "queued", MAC, None),
        )]
    };
    let stuck = |f: &Fixture| f.view().queue.jobs[0].stuck.as_ref().map(|s| s.reason);
    f.gh_runners(ago(60), vec![gh(1, "mac", "online", true, MAC)])
        .jobs(ago(40), waiting());
    assert_eq!(stuck(&f), None, "every match is working");

    // The job may have run on this runner and finished since the scan.
    f.gh_runners(ago(20), vec![gh(1, "mac", "online", false, MAC)]);
    let v = f.view();
    assert_eq!(v.queue.jobs[0].compatible_idle, 1);
    assert_eq!(stuck(&f), None);

    // A later scan still sees the job queued while the runner is idle.
    f.jobs(ago(10), waiting());
    assert_eq!(stuck(&f), Some(QueueStuckReason::WaitedPastLimit));
    f.gh_runners(ago(5), vec![gh(1, "mac", "online", false, MAC)]);
    assert_eq!(
        stuck(&f),
        Some(QueueStuckReason::WaitedPastLimit),
        "still idle since before the scan"
    );
}

#[test]
fn an_idle_run_restarts_after_a_gap_a_label_change_or_a_job_the_scan_saw() {
    let waiting = || created(STUCK_AFTER.num_minutes() + 5, job(10, None, "queued", MAC, None));
    let stuck = |f: &Fixture| {
        let v = f.view();
        v.queue
            .jobs
            .iter()
            .find(|q| q.job.id == 10)
            .unwrap()
            .stuck
            .as_ref()
            .map(|s| s.reason)
    };

    // Runner polls failed long enough for the list to go stale. The runner may have run the job unseen.
    let mut f = Fixture::new();
    f.gh_runners(ago(400), vec![gh(1, "mac", "online", false, MAC)])
        .jobs(ago(40), vec![waiting()]);
    assert_eq!(stuck(&f), Some(QueueStuckReason::RunnersOffline));
    f.gh_runners(ago(20), vec![gh(1, "mac", "online", false, MAC)]);
    assert_eq!(stuck(&f), None);

    // The runner gained the job's labels after the scan.
    let mut f = Fixture::new();
    f.gh_runners(ago(100), vec![gh(1, "mac", "online", false, &["self-hosted", "macOS"])])
        .jobs(ago(40), vec![waiting()]);
    assert_eq!(stuck(&f), Some(QueueStuckReason::NoLabelMatch));
    f.gh_runners(ago(20), vec![gh(1, "mac", "online", false, MAC)]);
    assert_eq!(stuck(&f), None);

    // The runner list said idle, but the newer scan saw the runner working.
    let mut f = Fixture::new();
    f.gh_runners(ago(100), vec![gh(1, "mac", "online", false, MAC)])
        .gh_runners(ago(50), vec![gh(1, "mac", "online", false, MAC)])
        .jobs(
            ago(30),
            vec![waiting(), job(11, Some(1), "in_progress", MAC, Some(ago(40)))],
        );
    assert_eq!(stuck(&f), None);
    // Running now and Up next both follow the runner list for whether it is idle.
    let v = f.view();
    assert_eq!((v.summary.runners_busy, v.queue.jobs[0].compatible_idle), (0, 1));
    // A later scan that can't read that work doesn't make the runner free.
    f.jobs(ago(20), vec![waiting()]);
    assert_eq!(stuck(&f), None);
    f.gh_runners(ago(10), vec![gh(1, "mac", "online", false, MAC)]);
    assert_eq!(stuck(&f), None);
    f.jobs(ago(5), vec![waiting()]);
    assert_eq!(stuck(&f), Some(QueueStuckReason::WaitedPastLimit));

    // The scan that saw the runner working stays the evidence after it goes stale.
    for scanned in [200, 400] {
        let mut f = Fixture::new();
        for at in [450, 330, 210, 90, 10] {
            f.gh_runners(ago(at), vec![gh(1, "mac", "online", false, MAC)]);
        }
        f.jobs(
            ago(scanned),
            vec![
                created(STUCK_AFTER.num_minutes() + 15, job(10, None, "queued", MAC, None)),
                job(11, Some(1), "in_progress", MAC, Some(ago(scanned + 10))),
            ],
        );
        assert_eq!(stuck(&f), None, "scan {scanned} s old");
    }

    // The same labels in another order or case keep the idle time.
    let mut f = Fixture::new();
    f.gh_runners(ago(100), vec![gh(1, "mac", "online", false, MAC)])
        .jobs(ago(40), vec![waiting()]);
    assert_eq!(stuck(&f), Some(QueueStuckReason::WaitedPastLimit));
    f.gh_runners(
        ago(20),
        vec![gh(1, "mac", "online", false, &["arm64", "MACOS", "self-hosted"])],
    );
    assert_eq!(stuck(&f), Some(QueueStuckReason::WaitedPastLimit));
}

#[test]
fn without_a_runner_list_a_self_hosted_job_waits_with_matching_unknown() {
    let mut f = Fixture::new();
    f.jobs(
        ago(10),
        vec![
            job(10, None, "queued", &["self-hosted", "gpu"], None),
            job(11, None, "queued", &["ubuntu-latest"], None),
        ],
    );
    let v = f.view();
    assert_eq!(v.queue.jobs.len(), 1);
    assert_eq!(v.queue.other_queued, 1);
    let q = &v.queue.jobs[0];
    assert!(!q.runners_known);
    assert_eq!(q.stuck, None, "no runner list is not a label mismatch");

    // An empty list from GitHub is a real answer: nothing has the labels.
    f.gh_runners(ago(5), vec![]);
    let v = f.view();
    let q = &v.queue.jobs[0];
    assert!(q.runners_known);
    assert_eq!(q.stuck.as_ref().map(|s| s.reason), Some(QueueStuckReason::NoLabelMatch));
}

#[test]
fn the_board_counts_each_runner_and_job_once() {
    let (config, obs, plan) = demo();
    let view = derive(&config, &obs, &plan, now(), DataSource::Demo);
    let running = view
        .machines
        .iter()
        .flat_map(|m| &m.runners)
        .chain(&view.pool_runners)
        .chain(&view.unmatched_runners)
        .filter(|r| matches!(r.activity, RunnerActivity::Busy { .. }))
        .count();
    assert_eq!(running, view.summary.runners_busy as usize);
    let (stuck, up_next): (Vec<_>, Vec<_>) = view.queue.jobs.iter().partition(|q| q.stuck.is_some());
    assert_eq!(up_next.len() + stuck.len(), view.summary.queued_jobs as usize);
    assert!(!up_next.is_empty());

    let reason = |id| {
        view.queue
            .jobs
            .iter()
            .find(|q| q.job.id == id)
            .unwrap()
            .stuck
            .as_ref()
            .map(|s| s.reason)
    };
    assert_eq!(reason(9003), None);
    assert_eq!(reason(9006), None);
    assert_eq!(reason(9004), Some(QueueStuckReason::NoLabelMatch));
    assert_eq!(reason(9007), Some(QueueStuckReason::OtherRepository));
    assert_eq!(reason(9008), Some(QueueStuckReason::RunnersOffline));
    assert_eq!(reason(9009), Some(QueueStuckReason::WaitedPastLimit));

    // Queue findings stay in the API, and the desktop hides that one category.
    let queue_findings = view
        .issues
        .iter()
        .filter(|i| stuck.iter().any(|q| i.title.contains(&q.job.name)))
        .inspect(|i| assert_eq!(i.category, crate::issues::FleetIssueCategory::Queue, "{}", i.title))
        .count();
    assert!(queue_findings > 0);
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
    assert_eq!(view.queue.jobs.len(), 6);
    assert!(view.machines.iter().all(|m| m.desired.is_some()));
    let names: Vec<_> = view.machines.iter().map(|m| m.display_name.as_str()).collect();
    assert_eq!(
        names,
        [
            "build-linux-01",
            "dev-laptop",
            "mac-mini-02",
            "mac-mini-03",
            "studio",
            "win-builder"
        ]
    );
    // Peers no inventory host claims are only offered to Add machine. A phone fits no platform,
    // and this Mac's own node is never offered.
    let devices: Vec<_> = view.tailnet_devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(devices, ["home-nas", "spare-ubuntu"]);
    // This Mac, and tailnet peers no inventory host claims, never raise an issue.
    let issues = serde_json::to_string(&view.issues).unwrap();
    let quiet = ["dev-laptop", "home-nas", "spare-ubuntu", "iphone"];
    assert!(quiet.iter().all(|name| !issues.contains(name)), "{issues}");
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
    assert!(!view.tailnet_devices.iter().any(|d| d.node_id == "nMini"));
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

#[path = "derive_pool_tests.rs"]
mod pools;

#[path = "derive_add_machine_tests.rs"]
mod add_machine;
