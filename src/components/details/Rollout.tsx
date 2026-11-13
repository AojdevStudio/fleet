import type { FleetView } from "../../bindings/FleetView";
import type { OldRunnerView } from "../../bindings/OldRunnerView";
import type { RolloutRepoView } from "../../bindings/RolloutRepoView";
import { plural } from "../../lib/format";
import { movingRepos, retiringRepos, ROLLOUT_STATES, rolloutStateLabel } from "../../lib/rollout";
import type { Tone } from "../../lib/status";
import { useUi } from "../../store";
import { Card, EmptyLine, Pill, Section, Stat, StatStrip } from "../ui";
import { PageHeader } from "./shared";

/** Repositories moving their workflows to the central pools, from the inventory's rollout list. */
export function MovingDetail({ view }: { view: FleetView }) {
  const moving = movingRepos(view.rollout);
  const count = (state: string) => view.rollout.filter((r) => r.state === state).length;
  return (
    <div>
      <PageHeader title="Moving to central" subtitle={`${plural(moving.length, "repository", "repositories")} still moving · from the inventory's rollout list`} />
      <div className="mt-8">
        <StatStrip>
          {ROLLOUT_STATES.map((state) => {
            const { label, tone } = rolloutStateLabel(state);
            return <Stat key={state} label={label} value={count(state)} tone={tone} />;
          })}
        </StatStrip>
      </div>
      <Section title="Repositories" count={view.rollout.length}>
        <Card className="divide-y divide-line">
          {view.rollout.length === 0 ? <EmptyLine>No repositories in the rollout list.</EmptyLine> : view.rollout.map((r) => <RepoRow key={r.repo} repo={r} />)}
        </Card>
      </Section>
    </div>
  );
}

function RepoRow({ repo }: { repo: RolloutRepoView }) {
  const { label, tone } = rolloutStateLabel(repo.state);
  return (
    <div className="flex flex-wrap items-center gap-2 px-4 py-2.5 text-[13px]">
      <span className="min-w-0 flex-1 truncate font-medium selectable">{repo.repo}</span>
      <span className="text-[12px] text-ink-2">{repo.oldRunners.length ? plural(repo.oldRunners.length, "old runner") : "no old runners"}</span>
      <Pill tone={tone}>{label}</Pill>
    </div>
  );
}

/** Long-lived runners that moved repositories leave behind, and whether each repository is done with them. */
export function RetiringDetail({ view }: { view: FleetView }) {
  const retiring = retiringRepos(view.rollout);
  const safe = retiring.filter((r) => r.safeToRetire).length;
  return (
    <div>
      <PageHeader
        title="Retiring"
        subtitle={`${plural(retiring.length, "repository", "repositories")} with old runners · ${safe} safe to retire · read-only`}
      />
      <Section title="Old runners" count={retiring.reduce((n, r) => n + r.oldRunners.length, 0)}>
        <Card className="divide-y divide-line">
          {retiring.length === 0 ? <EmptyLine>No repository lists old runners.</EmptyLine> : retiring.map((r) => (
            <div key={r.repo} className="px-4 py-2.5 text-[13px]">
              <div className="flex flex-wrap items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-medium selectable">{r.repo}</span>
                <Pill tone={rolloutStateLabel(r.state).tone}>{rolloutStateLabel(r.state).label}</Pill>
                {r.safeToRetire ? <Pill tone="ok">Safe to retire</Pill> : <Pill tone="warn">Keep for now</Pill>}
              </div>
              <ul className="mt-1.5 space-y-1">
                {r.oldRunners.map((o) => <OldRunner key={o.name} runner={o} />)}
              </ul>
            </div>
          ))}
        </Card>
      </Section>
      <p className="mt-3 text-[12px] text-ink-2">
        Safe to retire is set by hand in the inventory. Retire each old runner on its own machine after its repositories no longer need it.
      </p>
    </div>
  );
}

function oldRunnerStatus(o: OldRunnerView): { label: string; tone: Tone } {
  switch (o.activity?.kind) {
    case undefined:
      return { label: "Not listed by GitHub", tone: "muted" };
    case "busy":
      return { label: "Working", tone: "busy" };
    case "idle":
      return { label: "Idle", tone: "ok" };
    case "offline":
      return { label: "Offline", tone: "bad" };
    case "unknown":
      return { label: "Unknown", tone: "muted" };
  }
}

function OldRunner({ runner }: { runner: OldRunnerView }) {
  const select = useUi((s) => s.select);
  const status = oldRunnerStatus(runner);
  const key = runner.runnerKey;
  return (
    <li className="flex items-center gap-2 text-[12.5px]">
      {key ? (
        <button className="truncate text-accent hover:underline" onClick={() => select({ kind: "runner", key })}>{runner.name}</button>
      ) : (
        <span className="truncate">{runner.name}</span>
      )}
      <span className="ml-auto"><Pill tone={status.tone} hollow={!key}>{status.label}</Pill></span>
    </li>
  );
}
