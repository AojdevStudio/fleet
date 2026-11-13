//! runner-up central pools, read-only.
//!
//! runner-up (<https://github.com/kunchenguid/runner-up>, v0.1.12) runs each job
//! in a fresh place: a container, a KVM or tart VM, or the machine itself. Its
//! coordinator prints one line per place with `runner-up status --config FILE`:
//!
//! ```text
//! place box-1 on box (docker, 2 cores, 4096 MB) free
//! place box-2 on box (docker, 2 cores, 4096 MB) busy (job): octo-org/app "verify" running
//! place far-1 on far (docker, 2 cores, 4096 MB) resting (agent unreachable since 2026-01-02T03:04:05Z)
//! waiting octo-org/app ru-linux "suite" since 2026-01-02T03:04:05Z
//! starting octo-org/app ru-linux "build" in box-1
//! ```
//!
//! The status text doesn't say which pools a place serves, so the coordinator's
//! config file (JSON, never a token) is read beside it. Tart clones are named
//! `runner-up-<place>`, from `tart list --format json` on the Mac.
//!
//! The app reads all of it over the existing SSH path to the coordinator host.
//! Nothing here changes a machine.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::desired::model::{Host, RunnerUp};
use crate::desired::remote::{RemoteClient, RemoteError};
use crate::model::{Observations, count_u32};

/// Separates the status text from the config file in the script's output.
const CONFIG_MARKER: &str = "@@RUNNER_UP_CONFIG@@";
/// Separates the config file from `tart list` in the script's output.
const TART_MARKER: &str = "@@RUNNER_UP_TART@@";
/// Tart clones of runner-up places carry this prefix before the place id.
const CLONE_PREFIX: &str = "runner-up-";

/// What a place is doing, from its status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export, rename = "RunnerUpPlaceState")]
#[serde(rename_all = "camelCase")]
pub enum PlaceState {
    Free,
    /// Holds a job or a borrow, or its machine is full.
    Busy,
    /// Its machine's agent is unreachable.
    Resting,
    /// Paused for a drain.
    Drained,
    /// Configured but missing from the status, or a line this parser doesn't know.
    Unknown,
}

/// One place's status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceStatus {
    pub id: String,
    pub machine: String,
    pub provider: String,
    pub cores: u32,
    pub memory_mb: u64,
    pub state: PlaceState,
    /// The rest of the line after the sizes, such as `busy (job): o/r "build" running`.
    pub detail: String,
}

/// A waiting or starting job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolJob {
    /// `None` for a borrow.
    pub repo: Option<String>,
    pub pool: String,
    pub name: String,
    /// The place a starting job is in.
    pub place: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusReport {
    pub places: Vec<PlaceStatus>,
    pub waiting: Vec<PoolJob>,
    pub starting: Vec<PoolJob>,
}

/// The pool labels each configured place serves, from runner-up's config file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigFacts {
    pub pools: Vec<String>,
    /// Place id → (machine, provider, pools).
    pub places: BTreeMap<String, (String, String, Vec<String>)>,
}

/// One VM from `tart list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export, rename = "RunnerUpTartVm")]
#[serde(rename_all = "camelCase")]
pub struct TartVm {
    pub name: String,
    pub running: bool,
    /// The runner-up place this VM is a clone for, when it is one.
    pub place: Option<String>,
}

/// One read of the coordinator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolsReport {
    pub status: StatusReport,
    pub config: ConfigFacts,
    /// `None` when tart couldn't be read.
    pub tart: Option<Vec<TartVm>>,
}

/// The pools as the UI and the local API show them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export, rename = "RunnerUpPoolsView")]
#[serde(rename_all = "camelCase")]
pub struct PoolsView {
    /// The inventory host that runs the coordinator.
    pub coordinator: String,
    /// When the coordinator was last read; `None` before the first read.
    pub checked_at: Option<DateTime<Utc>>,
    /// The latest read error, shown beside the last good data.
    pub error: Option<String>,
    pub pools: Vec<PoolView>,
    pub places: Vec<PlaceView>,
    /// Tart VMs on the Mac; `None` when tart couldn't be read.
    pub tart_vms: Option<Vec<TartVm>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export, rename = "RunnerUpPoolView")]
#[serde(rename_all = "camelCase")]
pub struct PoolView {
    pub name: String,
    /// Ephemeral places that take this pool's jobs.
    pub places: u32,
    pub free: u32,
    pub busy: u32,
    /// Resting, drained or unknown places.
    pub unavailable: u32,
    pub waiting_jobs: u32,
    pub starting_jobs: u32,
    /// Running tart VMs cloned for this pool's places.
    pub running_vms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export, rename = "RunnerUpPlaceView")]
#[serde(rename_all = "camelCase")]
pub struct PlaceView {
    pub id: String,
    pub machine: String,
    pub provider: String,
    pub pools: Vec<String>,
    pub state: PlaceState,
    pub detail: Option<String>,
}

/// Reads the coordinator host. Tests replace it; the app uses SSH.
pub trait PoolSource: Send + Sync {
    /// Blocks until the SSH read completes or times out. Call off the async runtime.
    fn read(&self, host: &Host, script: &str) -> Result<String, RemoteError>;
}

impl PoolSource for RemoteClient {
    fn read(&self, host: &Host, script: &str) -> Result<String, RemoteError> {
        self.run_script(host, script, None)
    }
}

/// A POSIX shell word for a path; a leading `~/` stays relative to `$HOME`.
fn shell_path(path: &str) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", quote(rest)),
        None => quote(path),
    }
}

/// The read-only script for the coordinator: its status, its config, and
/// `tart list` when `with_tart`. The status must succeed; tart may be missing.
pub fn read_script(runner_up: &RunnerUp, with_tart: bool) -> String {
    let program = shell_path(&runner_up.program);
    let config = shell_path(&runner_up.config);
    let mut script = format!(
        "export PATH=\"$HOME/.local/bin:$HOME/go/bin:/opt/homebrew/bin:/usr/local/bin:$PATH\"\n\
         {program} status --config {config} || exit $?\n\
         printf '%s\\n' '{CONFIG_MARKER}'\n\
         cat {config} || exit $?\n"
    );
    if with_tart {
        script.push_str(&tart_script());
    }
    script
}

/// Lists tart VMs; prints only the marker when tart is missing.
pub fn tart_script() -> String {
    format!(
        "export PATH=\"/opt/homebrew/bin:/usr/local/bin:$PATH\"\n\
         printf '\\n%s\\n' '{TART_MARKER}'\n\
         command -v tart >/dev/null 2>&1 && tart list --format json 2>/dev/null || true\n"
    )
}

/// Parses the output of [`read_script`], with the tart section possibly from another read.
pub fn parse_read(output: &str, tart_output: Option<&str>) -> Result<PoolsReport, String> {
    let (status, rest) = output
        .split_once(CONFIG_MARKER)
        .ok_or("runner-up printed no status; check the program and config paths.")?;
    let (config, tart) = match rest.split_once(TART_MARKER) {
        Some((config, tart)) => (config, Some(tart)),
        None => (rest, None),
    };
    let tart = tart_output
        .and_then(|t| t.split_once(TART_MARKER).map(|(_, t)| t))
        .or(tart);
    Ok(PoolsReport {
        status: parse_status(status),
        config: parse_config(config)?,
        tart: tart.and_then(parse_tart_list),
    })
}

/// Parses `runner-up status` text. Lines it doesn't recognize are skipped.
pub fn parse_status(text: &str) -> StatusReport {
    let mut report = StatusReport::default();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("place ") {
            if let Some(place) = parse_place(rest) {
                report.places.push(place);
            }
        } else if let Some(rest) = line.strip_prefix("waiting ") {
            if let Some(job) = parse_job(rest, " since ") {
                report.waiting.push(job);
            }
        } else if let Some(rest) = line.strip_prefix("starting ")
            && let Some(job) = parse_job(rest, " in ")
        {
            report.starting.push(job);
        }
    }
    report
}

/// `<id> on <machine> (<provider>, <n> cores, <m> MB) <state>`
fn parse_place(rest: &str) -> Option<PlaceStatus> {
    let (id, rest) = rest.split_once(" on ")?;
    let (machine, rest) = rest.split_once(" (")?;
    let (sizes, detail) = rest.split_once(") ")?;
    let mut sizes = sizes.split(", ");
    let provider = sizes.next()?.to_string();
    let cores = sizes.next()?.strip_suffix(" cores")?.parse().ok()?;
    let memory_mb = sizes.next()?.strip_suffix(" MB")?.parse().ok()?;
    let state = match detail.split([' ', ':']).next().unwrap_or_default() {
        "free" => PlaceState::Free,
        "busy" => PlaceState::Busy,
        "resting" => PlaceState::Resting,
        "drained" => PlaceState::Drained,
        _ => PlaceState::Unknown,
    };
    Some(PlaceStatus {
        id: id.to_string(),
        machine: machine.to_string(),
        provider,
        cores,
        memory_mb,
        state,
        detail: detail.to_string(),
    })
}

/// `<repo> <pool> "<name>"<tail>` or `borrow <pool> "<name>"<tail>`, where the
/// tail starts with `separator` (` since ` or ` in `).
fn parse_job(rest: &str, separator: &str) -> Option<PoolJob> {
    let (head, quoted) = rest.split_once(" \"")?;
    let (name, tail) = quoted.rsplit_once(&format!("\"{separator}"))?;
    let (repo, pool) = head.split_once(' ')?;
    Some(PoolJob {
        repo: (repo != "borrow").then(|| repo.to_string()),
        pool: pool.to_string(),
        name: name.replace("\\\"", "\""),
        place: (separator == " in ").then(|| tail.trim().to_string()),
    })
}

/// Pools and places from runner-up's JSON config. Other keys are ignored.
pub fn parse_config(text: &str) -> Result<ConfigFacts, String> {
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        pools: Vec<Pool>,
        #[serde(default)]
        machines: Vec<Machine>,
    }
    #[derive(Deserialize)]
    struct Pool {
        label: String,
    }
    #[derive(Deserialize)]
    struct Machine {
        name: String,
        #[serde(default)]
        places: Vec<Place>,
    }
    #[derive(Deserialize)]
    struct Place {
        id: String,
        provider: String,
        #[serde(default)]
        pools: Vec<String>,
    }
    let config: Config =
        serde_json::from_str(text.trim()).map_err(|e| format!("Couldn't read runner-up's config: {e}"))?;
    let mut places = BTreeMap::new();
    for machine in config.machines {
        for place in machine.places {
            places.insert(place.id, (machine.name.clone(), place.provider, place.pools));
        }
    }
    Ok(ConfigFacts {
        pools: config.pools.into_iter().map(|p| p.label).collect(),
        places,
    })
}

/// `tart list --format json`; `None` when tart printed nothing or something else.
pub fn parse_tart_list(text: &str) -> Option<Vec<TartVm>> {
    #[derive(Deserialize)]
    struct Vm {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Running", default)]
        running: bool,
    }
    let vms: Vec<Vm> = serde_json::from_str(text.trim()).ok()?;
    Some(
        vms.into_iter()
            .map(|vm| TartVm {
                place: vm.name.strip_prefix(CLONE_PREFIX).map(str::to_string),
                running: vm.running,
                name: vm.name,
            })
            .collect(),
    )
}

/// The pools to show: the inventory's list, or every configured pool.
pub fn pools_view(
    runner_up: &RunnerUp,
    report: &PoolsReport,
    checked_at: Option<DateTime<Utc>>,
    error: Option<String>,
) -> PoolsView {
    let wanted = if runner_up.pools.is_empty() {
        report.config.pools.clone()
    } else {
        runner_up.pools.clone()
    };
    let status: BTreeMap<&str, &PlaceStatus> = report.status.places.iter().map(|p| (p.id.as_str(), p)).collect();
    let places: Vec<PlaceView> = report
        .config
        .places
        .iter()
        .filter(|(_, (_, _, pools))| pools.iter().any(|p| wanted.contains(p)))
        .map(|(id, (machine, provider, pools))| {
            let line = status.get(id.as_str());
            PlaceView {
                id: id.clone(),
                machine: machine.clone(),
                provider: provider.clone(),
                pools: pools.clone(),
                state: line.map_or(PlaceState::Unknown, |l| l.state),
                detail: line.map(|l| l.detail.clone()),
            }
        })
        .collect();
    let running_vm = |place: &str| {
        report
            .tart
            .iter()
            .flatten()
            .any(|vm| vm.running && vm.place.as_deref() == Some(place))
    };
    let pools = wanted
        .iter()
        .map(|name| {
            let serving: Vec<&PlaceView> = places.iter().filter(|p| p.pools.contains(name)).collect();
            let count = |state: PlaceState| count_u32(serving.iter().filter(|p| p.state == state).count());
            let jobs = |jobs: &[PoolJob]| count_u32(jobs.iter().filter(|j| &j.pool == name).count());
            PoolView {
                name: name.clone(),
                places: count_u32(serving.len()),
                free: count(PlaceState::Free),
                busy: count(PlaceState::Busy),
                unavailable: count(PlaceState::Resting) + count(PlaceState::Drained) + count(PlaceState::Unknown),
                waiting_jobs: jobs(&report.status.waiting),
                starting_jobs: jobs(&report.status.starting),
                running_vms: count_u32(serving.iter().filter(|p| running_vm(&p.id)).count()),
            }
        })
        .collect();
    PoolsView {
        coordinator: runner_up.host.clone(),
        checked_at,
        error,
        pools,
        places,
        tart_vms: report.tart.clone(),
    }
}

/// The pools view from the latest observations, when the inventory names a coordinator.
pub fn view(obs: &Observations) -> Option<PoolsView> {
    let pools = obs.pools.as_ref()?;
    let report = pools.report.as_ref();
    Some(pools_view(
        &pools.runner_up,
        report.map(|r| &r.value).unwrap_or(&PoolsReport::default()),
        report.map(|r| r.at),
        pools.error.as_ref().map(|e| e.value.clone()),
    ))
}

/// A fictional coordinator read, for demo data and tests.
pub mod fixture {
    pub const STATUS: &str = r#"place ci-linux-1 on ci-linux (docker, 4 cores, 8192 MB) busy (job): example/app "test / linux" running
place ci-linux-2 on ci-linux (docker, 4 cores, 8192 MB) free
place ci-kvm-1 on ci-linux (kvm, 4 cores, 8192 MB) free
place mac-1 on mac-coordinator (tart, 4 cores, 8192 MB) busy (job): example/ios "build" running
place mac-2 on mac-coordinator (tart, 4 cores, 8192 MB) resting (agent unreachable since 2026-11-01T10:00:00Z)
waiting example/app central-linux "lint" since 2026-11-01T10:02:00Z
waiting borrow central-macos "native check" since 2026-11-01T10:03:00Z
starting example/site central-linux-kvm "e2e" in ci-kvm-1
"#;

    pub const CONFIG: &str = r#"{
  "stateDir": "/var/lib/runner-up",
  "github": { "repos": ["example/app", "example/site", "example/ios"] },
  "pools": [{ "label": "central-linux" }, { "label": "central-linux-kvm" }, { "label": "central-macos" }],
  "machines": [
    { "name": "ci-linux", "ssh": ["ci@192.0.2.10"], "places": [
      { "id": "ci-linux-1", "provider": "docker", "pools": ["central-linux"], "cores": 4, "memoryMB": 8192 },
      { "id": "ci-linux-2", "provider": "docker", "pools": ["central-linux"], "cores": 4, "memoryMB": 8192 },
      { "id": "ci-kvm-1", "provider": "kvm", "pools": ["central-linux-kvm"], "cores": 4, "memoryMB": 8192 }
    ]},
    { "name": "mac-coordinator", "places": [
      { "id": "mac-1", "provider": "tart", "pools": ["central-macos"], "cores": 4, "memoryMB": 8192 },
      { "id": "mac-2", "provider": "tart", "pools": ["central-macos"], "cores": 4, "memoryMB": 8192 }
    ]}
  ]
}
"#;

    pub const TART: &str = r#"[
  {"Name": "runner-up-mac-1", "Source": "local", "Running": true, "State": "running"},
  {"Name": "macos-sequoia-base", "Source": "local", "Running": false, "State": "stopped"}
]"#;

    /// The script output these make together.
    pub fn output() -> String {
        format!(
            "{STATUS}{}\n{CONFIG}\n{}\n{TART}\n",
            super::CONFIG_MARKER,
            super::TART_MARKER
        )
    }
}

#[cfg(test)]
#[path = "runner_up_tests.rs"]
mod tests;
