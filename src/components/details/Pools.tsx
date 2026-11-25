import type { FleetView } from "../../bindings/FleetView";
import type { RunnerUpPoolsView } from "../../bindings/RunnerUpPoolsView";
import { plural } from "../../lib/format";
import { Card, Section, TimeAgo } from "../ui";
import { List, RunnerLine } from "./shared";

/** runner-up central pools: ephemeral places per pool, read from the coordinator, and the runners in each place. */
export function Pools({ pools, view }: { pools: RunnerUpPoolsView; view: FleetView }) {
  const running = pools.tartVms?.filter((vm) => vm.running).length;
  return (
    <Section title="Central pools" count={pools.pools.length}>
      <Card className="overflow-x-auto">
        <table className="w-full min-w-[620px] text-[13px]">
          <thead className="border-b border-line bg-sunken/40 text-left text-[12px] text-ink-2">
            <tr>
              {["Pool", "Places", "Free", "Busy", "Unavailable", "Waiting", "Starting", "Tart VMs"].map((label, i) => (
                <th key={label} scope="col" className={`py-2 font-medium ${i === 0 ? "px-4" : "px-3"}`}>{label}</th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-line">
            {pools.pools.map((p) => (
              <tr key={p.name}>
                <td className="px-4 py-2 font-medium">{p.name}</td>
                <td className="px-3 tabular-nums">{p.places}</td>
                <td className="px-3 tabular-nums">{p.free}</td>
                <td className="px-3 tabular-nums">{p.busy}</td>
                <td className="px-3 tabular-nums">{p.unavailable}</td>
                <td className="px-3 tabular-nums">{p.waitingJobs}</td>
                <td className="px-3 tabular-nums">{p.startingJobs}</td>
                <td className="px-3 tabular-nums">{p.runningVms}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
      <p className="mt-2 text-[12px] text-ink-2">
        Read-only from runner-up on {pools.coordinator}
        {pools.checkedAt ? <> · checked <TimeAgo iso={pools.checkedAt} /></> : " · not read yet"}
        {` · ${plural(pools.places.length, "place")}`}
        {running === undefined ? " · tart VMs unknown" : ` · ${running} of ${plural(pools.tartVms?.length ?? 0, "tart VM")} running`}
      </p>
      {pools.error && <p className="mt-1 break-words text-[12px] text-bad selectable">{pools.error}</p>}
      <PlaceRunners pools={pools} view={view} />
    </Section>
  );
}

/** Pool runners under the place their name names, in the coordinator's place order. */
function PlaceRunners({ pools, view }: { pools: RunnerUpPoolsView; view: FleetView }) {
  const places = pools.places
    .map((place) => ({ place, runners: view.poolRunners.filter((r) => r.place === place.id) }))
    .filter((p) => p.runners.length > 0);
  if (!places.length) return null;
  return (
    <div className="mt-4 space-y-3">
      {places.map(({ place, runners }) => (
        <div key={place.id}>
          <h3 className="mb-1.5 px-0.5 text-[12px] text-ink-2">
            <span className="font-mono font-medium text-ink">{place.id}</span>
            {` · ${place.provider} on ${place.machine} · ${place.pools.join(", ")}`}
          </h3>
          <List>{runners.map((r) => <RunnerLine key={r.key} r={r} view={view} />)}</List>
        </div>
      ))}
    </div>
  );
}
