import type { FleetPlanItem } from "../../bindings/FleetPlanItem";
import type { MachineDesired } from "../../bindings/MachineDesired";
import { changeOffReason, changes } from "../../lib/desired";
import { Button, Callout, Card, Section } from "../ui";
import { PlanItems, type Change } from "./changes";

/** What differs from the inventory, each with the action that fixes it. */
export function MachineChanges({ d, change }: { d: MachineDesired; change: Change; live: boolean }) {
  const items = changes(d.plan);
  if (d.error) {
    return (
      <Callout tone="bad" title="Couldn't read this machine">
        {d.error}
        <div className="mt-1 text-ink-3">Its runners show their last known state until a check reaches it.</div>
      </Callout>
    );
  }
  if (!d.checkedAt || items.length === 0) return null;
  const applies = items.some((i) => i.apply && !i.blocked);
  const replacement = (legacy: string) => d.host.runners.find((r) => r.replaces?.name === legacy)?.name;
  const action = (i: FleetPlanItem) => {
    if (i.blocked) return null;
    if (i.action === "retire-legacy") {
      const runner = replacement(i.runner);
      return (
        runner && (
          <Button small variant="danger" disabled title={changeOffReason("retire")} onClick={() => change.open({ kind: "retire", host: d.host.id, runner })}>
            Retire…
          </Button>
        )
      );
    }
    if (i.action === "decommission") {
      return (
        <Button small variant="danger" disabled title={changeOffReason("decommission")} onClick={() => change.open({ kind: "decommission", host: d.host.id, runner: i.runner })}>
          Decommission…
        </Button>
      );
    }
    return null;
  };
  return (
    <Section
      title="Changes"
      count={items.length}
      action={
        applies && (
          <Button small variant="primary" disabled title={changeOffReason("apply")} onClick={() => change.open({ kind: "apply", host: d.host.id, runner: null })}>
            Review apply…
          </Button>
        )
      }
    >
      <Card className="px-4 py-3">
        <PlanItems items={items} action={action} />
      </Card>
      <p className="mt-2 text-[12px] text-ink-2">
        Apply, Retire and Decommission are turned off until the desired-state rewrite. The dashboard only reads runners for now.
      </p>
    </Section>
  );
}
