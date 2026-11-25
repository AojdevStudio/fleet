import type { ReactNode } from "react";
import type { FleetView } from "../../bindings/FleetView";
import type { JobView } from "../../bindings/JobView";
import type { MachineView } from "../../bindings/MachineView";
import type { QueuedJobView } from "../../bindings/QueuedJobView";
import type { RunnerView } from "../../bindings/RunnerView";
import { useNow } from "../../hooks";
import { jobBoard } from "../../lib/board";
import { plural, shortDuration } from "../../lib/format";
import { allRunners, findMachine } from "../../lib/nav";
import { activityCounts, busySince, machineDot, machineSummary, machineTooltip, type Tone } from "../../lib/status";
import { useUi } from "../../store";
import { calloutSurface, Card, Elapsed, EmptyLine, StatusDot } from "../ui";
import { Icon } from "../icons";
import { GitHubLink, RunnerCells } from "./shared";

/** The first screen: what runs now, what comes next, and what is stuck. */
export function JobBoard({ view }: { view: FleetView }) {
  const board = jobBoard(view);
  const runners = activityCounts(allRunners(view));
  const limit = Math.round(view.queue.stuckAfterSecs / 60);
  const q = view.queue;
  return (
    <section className="@container mt-8" aria-label="Jobs">
      <div className="grid grid-cols-1 gap-3 @min-[860px]:grid-cols-3">
        <Column title="Running now" tone="busy" count={board.running.length}
          note={`${board.running.length} of ${plural(view.summary.runners, "runner")} working · ${runners.idle} idle`}
          empty="No runner is working right now.">
          {board.running.map((r) => <RunningRow key={r.key} r={r} view={view} />)}
        </Column>
        <Column title="Up next" tone="ok" count={board.upNext.length} note="Oldest first"
          empty={q.asOf ? "Nothing is waiting for a runner." : "Waiting for job data from GitHub."}>
          {board.upNext.map((q) => <QueuedRow key={`${q.org}:${q.job.id}`} q={q} />)}
        </Column>
        <Column title="Stuck" tone="warn" count={board.stuck.length}
          note={`No match online, or over ${limit} min with one idle`}
          empty="Nothing is stuck.">
          {board.stuck.map((q) => <QueuedRow key={`${q.org}:${q.job.id}`} q={q} />)}
        </Column>
      </div>
      {(!q.complete || q.otherQueued > 0) && (
        <div className="mt-2 space-y-1 px-0.5 text-[12px] text-ink-2">
          {!q.complete && <p className="text-warn">Job data is partial. Some organizations or repositories have no recent scan.</p>}
          {q.otherQueued > 0 && <p>{plural(q.otherQueued, "queued job")} {q.otherQueued === 1 ? "matches" : "match"} no self-hosted runner and {q.otherQueued === 1 ? "is" : "are"} not shown.</p>}
        </div>
      )}
      <MachineStrip machines={view.machines} />
    </section>
  );
}

function Column({ title, tone, count, note, empty, children }: {
  title: string;
  tone: Tone;
  count: number;
  note: string;
  empty: string;
  children: ReactNode;
}) {
  const alert = tone === "warn" && count > 0;
  return (
    <Card className={`flex min-w-0 flex-col overflow-hidden ${alert ? calloutSurface.warn : ""}`}>
      <div className={`border-b px-4 py-2.5 ${alert ? "border-warn/20" : "border-line"}`}>
        <div className="flex items-center gap-2">
          <StatusDot tone={tone} hollow={count === 0} />
          <h2 className="text-[13px] font-semibold">{title}</h2>
          <span className="font-mono text-[12px] text-ink-2 tabular">{count}</span>
        </div>
        <p className="mt-0.5 truncate text-[12px] text-ink-2" title={note}>{note}</p>
      </div>
      {count === 0 ? <EmptyLine>{empty}</EmptyLine> : (
        // Bounded so a long backlog keeps the rest of the overview in reach. Focusable so arrow keys scroll it.
        <div role="region" aria-label={title} tabIndex={0} data-arrow-scroll
          className={`max-h-[360px] divide-y overflow-y-auto overscroll-contain outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset ${alert ? "divide-warn/15" : "divide-line"}`}>
          {children}
        </div>
      )}
    </Card>
  );
}

function jobTitle(job: JobView): string {
  return job.workflowName ? `${job.workflowName} · ${job.name}` : job.name;
}

function JobPlace({ job }: { job: JobView }) {
  return (
    <div className="truncate font-mono text-[12px] text-ink-2" title={job.repo}>
      {job.repo}{job.branch && <span className="text-ink-3"> · {job.branch}</span>}
    </div>
  );
}

function RunningRow({ r, view }: { r: RunnerView; view: FleetView }) {
  const select = useUi((s) => s.select);
  const job = r.currentJob;
  const since = busySince(r);
  const machine = findMachine(view, r.machineId)?.displayName;
  return (
    <button type="button" onClick={() => select({ kind: "runner", key: r.key })}
      className="flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-hover">
      <div className="min-w-0 flex-1">
        <div className={`truncate font-medium ${job ? "" : "text-ink-2"}`} title={job ? jobTitle(job) : undefined}>
          {job ? jobTitle(job) : "Job details unavailable"}
        </div>
        {job && <JobPlace job={job} />}
        <div className="truncate text-[12px] text-ink-3">{r.name}{machine && ` on ${machine}`}</div>
      </div>
      <span className="shrink-0 pt-px font-mono text-[12px] text-ink-2 tabular">{since ? <Elapsed since={since} /> : "—"}</span>
    </button>
  );
}

/** Who can take a waiting job, or why nobody can. */
function takers(q: QueuedJobView): { tone: Tone; text: string } {
  if (q.stuck) return { tone: "warn", text: q.stuck.detail };
  if (!q.runnersKnown) return { tone: "muted", text: "Runner matching isn't available yet. GitHub hasn't sent the runner list." };
  return q.compatibleIdle > 0
    ? { tone: "ok", text: `${plural(q.compatibleIdle, "idle runner")} ${q.compatibleIdle === 1 ? "matches" : "match"}` }
    : { tone: "busy", text: `${plural(q.compatibleOnline, "runner")} ${q.compatibleOnline === 1 ? "matches" : "match"}, all working` };
}

function QueuedRow({ q }: { q: QueuedJobView }) {
  const now = useNow();
  const who = takers(q);
  const wait = q.createdAt ? shortDuration(Math.round((now - Date.parse(q.createdAt)) / 1000)) : null;
  return (
    <div className="flex items-start gap-3 px-4 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="min-w-0 truncate font-medium" title={jobTitle(q.job)}>{jobTitle(q.job)}</span>
          {q.job.htmlUrl && (
            <GitHubLink url={q.job.htmlUrl} className="shrink-0 text-ink-3 hover:text-accent" label={`Open ${q.job.name} on GitHub`}>
              <Icon name="external" size={11} />
            </GitHubLink>
          )}
        </div>
        <JobPlace job={q.job} />
        <div className={`mt-0.5 flex items-start gap-1.5 text-[12px] ${q.stuck ? "text-ink" : "text-ink-2"}`}>
          <span className="flex h-[1lh] items-center"><StatusDot tone={who.tone} size={6} /></span>
          <span className="min-w-0">{who.text}</span>
        </div>
      </div>
      {wait && (
        <span className={`shrink-0 pt-px font-mono text-[12px] tabular ${q.stuck ? "text-warn" : "text-ink-2"}`} title="Time in the queue">
          {wait}
        </span>
      )}
    </div>
  );
}

/** One compact row: each machine's state and its runners. */
function MachineStrip({ machines }: { machines: MachineView[] }) {
  const select = useUi((s) => s.select);
  if (!machines.length) return null;
  return (
    <Card className="mt-3 flex flex-wrap items-center gap-x-1 gap-y-0.5 px-1.5 py-1">
      {machines.map((m) => {
        const dot = machineDot(m);
        return (
          <button key={m.id} type="button" title={machineTooltip(m)} onClick={() => select({ kind: "machine", id: m.id })}
            className="flex min-h-[32px] items-center gap-2 rounded-md px-2.5 text-left hover:bg-hover">
            <StatusDot tone={dot.tone} hollow={dot.hollow} />
            <span className="text-[12.5px] font-medium whitespace-nowrap">{m.displayName}</span>
            {m.runners.length > 0 && <RunnerCells runners={m.runners} />}
            <span className="font-mono text-[11px] whitespace-nowrap text-ink-2">{machineSummary(m)}</span>
          </button>
        );
      })}
    </Card>
  );
}
