import type { FleetView } from "../../bindings/FleetView";
import { useNow } from "../../hooks";
import { plural, timeAgo } from "../../lib/format";
import { isLive } from "../../lib/tauri";
import { Section } from "../ui";
import { activeAlerts } from "../../lib/monitoring";
import { Attention, ClearedList } from "./Attention";
import { OperationCard, operationIssue } from "./changes";
import { InventoryProblem, PageHeader } from "./shared";
import { JobBoard } from "./JobBoard";
import { MachineTable } from "./MachineTable";
import { ActiveAlerts, EventTimeline, MonitoringError } from "./Monitoring";
import { RunnerChecks } from "./RunnerChecks";
import { Pools } from "./Pools";
import { Unlinked } from "./Unlinked";
import { Welcome } from "./Welcome";

export function Overview({ view }: { view: FleetView }) {
  const s = view.summary;
  const machines = view.machines;

  if (view.orgs.length === 0 && machines.length === 0 && view.source === "live") {
    return <Welcome view={view} />;
  }

  const orgs = view.orgs.map((o) => o.login).join(", ");
  const alerts = activeAlerts(view, view.monitoring.alerts);
  return (
    <div>
      <PageHeader
        title="Fleet overview"
        subtitle={`${plural(machines.length, "machine")} · ${plural(s.runners, "runner")}${orgs ? ` · ${orgs}` : ""}`}
      />
      <PlanLine view={view} />
      <InventoryProblem view={view} />
      {view.plan.operation && <OperationCard op={view.plan.operation} issue={operationIssue(view)} />}
      <JobBoard view={view} />

      <Attention issues={view.issues.filter((i) => i.id !== "inventory")} />
      <ClearedList issues={view.issues} />
      <MonitoringError view={view} />
      {alerts.length > 0 && <Section title="Active alerts" count={alerts.length}>
        <ActiveAlerts alerts={alerts} view={view} showMachine />
      </Section>}

      <Section title="Machines" count={machines.length} action={
        <div className="flex items-center gap-3 text-[12px] text-ink-2">
          <Legend color="bg-busy">Working</Legend>
          <Legend color="bg-ok">Idle</Legend>
          <Legend color="border border-dashed border-bad">Offline</Legend>
          <Legend color="border border-dashed border-muted">Unknown</Legend>
        </div>
      }>
        <MachineTable machines={machines} />
      </Section>

      {view.pools && <Pools pools={view.pools} />}
      {view.unmatchedRunners.length > 0 && <Unlinked view={view} />}
      <EventTimeline key={view.source} view={view} latest={view.monitoring.events} />
      {(view.plan.organization || view.plan.controlChecks.length > 0) && (
        <RunnerChecks
          host={null}
          checks={view.plan.controlChecks}
          checkedAt={view.plan.checksAt}
          running={view.plan.doctorRunning}
          live={isLive(view)}
          notRun="Read-only: GitHub access and the inventory here, and each machine over SSH. Each machine's results appear on its page."
        />
      )}
    </div>
  );
}

function Legend({ color, children }: { color: string; children: React.ReactNode }) {
  return <span className="flex items-center gap-1.5"><span aria-hidden className={`h-3 w-1.5 rounded-[2px] ${color}`} />{children}</span>;
}

/** When the fleet was last compared with the inventory. */
function PlanLine({ view }: { view: FleetView }) {
  const now = useNow();
  const p = view.plan;
  if (!p.organization && !p.inventoryError) return null;
  return (
    <p className="mt-1 break-words text-[12px] text-ink-2">
      {p.refreshing
        ? "Checking GitHub and each machine against the inventory…"
        : p.checkedAt
          ? `Compared with ${p.inventoryPath} ${timeAgo(p.checkedAt, now)}.`
          : "Not compared with the inventory yet. Check now reads GitHub and each machine over SSH; it never changes anything."}
    </p>
  );
}
