import type { RunnerUpPoolsView } from "../../bindings/RunnerUpPoolsView";
import { plural } from "../../lib/format";
import { Card, Section, TimeAgo } from "../ui";

/** runner-up central pools: ephemeral places per pool, read from the coordinator. */
export function Pools({ pools }: { pools: RunnerUpPoolsView }) {
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
    </Section>
  );
}
