import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { FleetIssue } from "../../bindings/FleetIssue";
import type { FleetIssueCategory } from "../../bindings/FleetIssueCategory";
import { commands, emitEvent } from "../../lib/tauri";
import { useUi } from "../../store";
import { Button, calloutSurface, Card, ErrorText, Section } from "../ui";
import { Icon } from "../icons";

/** Findings shown elsewhere on the overview: alerts, stuck jobs, history storage and the running change. */
const SHOWN_ELSEWHERE: FleetIssueCategory[] = ["alert", "queue", "storage", "operation"];

/** Clear hides an item while the same problem lasts. Bring back shows it again. */
function useClear(id: string, cleared: boolean) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => (cleared ? commands.restoreIssue(id) : commands.clearIssue(id)),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["fleet"] }),
  });
}

export function ClearButton({ issue }: { issue: FleetIssue }) {
  const clear = useClear(issue.id, issue.cleared);
  return <div className="flex shrink-0 flex-col items-end gap-1">
    <Button small variant="ghost" disabled={clear.isPending} onClick={() => clear.mutate()}
      title={issue.cleared ? "Show this item again" : "Hide this item until the problem changes or comes back"}>
      {issue.cleared ? "Bring back" : "Clear"}
    </Button>
    {clear.isError && <ErrorText error={clear.error} className="max-w-[240px] text-right" />}
  </div>;
}

/** What to do, under the title and detail. A grouped item has one line per problem. */
export function ActionText({ action }: { action: string }) {
  return <p className="mt-1 whitespace-pre-line break-words text-[12.5px] text-ink selectable">{action}</p>;
}

/** The overview's uncleared findings, or one machine's with `machineId`. */
export function Attention({ issues, machineId }: { issues: FleetIssue[]; machineId?: string }) {
  const select = useUi((s) => s.select);
  const items = issues.filter((i) => !i.cleared && !SHOWN_ELSEWHERE.includes(i.category));
  if (items.length === 0) return null;
  return (
    <Section title="Needs attention" count={items.length}>
      <Card className={`divide-y divide-warn/15 overflow-hidden ${calloutSurface.warn}`}>
        {items.map((issue) => {
          const { runnerKey, machineId: target } = issue;
          const open = issue.fix ? () => emitEvent("open-settings")
            : runnerKey ? () => select({ kind: "runner", key: runnerKey })
            : target && target !== machineId ? () => select({ kind: "machine", id: target })
            : undefined;
          const content = <>
            <span className="mt-0.5 text-warn"><Icon name="alert" size={13} /></span>
            <div className="min-w-0 flex-1">
              <div className="truncate font-medium">{issue.title}</div>
              <div className="mt-0.5 line-clamp-2 break-words text-[12.5px] text-ink-2" title={issue.detail}>{issue.detail}</div>
              <ActionText action={issue.action} />
            </div>
            {open && (issue.fix === "addOrganization"
              ? <span className="flex shrink-0 items-center gap-1.5 rounded-md border border-line bg-card px-2.5 py-1 text-[12.5px] font-medium">Add organization<Icon name="chevron" size={12} /></span>
              : <span className="mt-0.5 text-ink-3"><Icon name="chevron" size={11} /></span>)}
          </>;
          const className = "flex min-w-0 flex-1 items-start gap-3 text-left";
          return <div key={issue.id} className="flex items-start gap-2 px-4 py-3">
            {open
              ? <button type="button" onClick={open} className={`${className} rounded-md hover:bg-warn/5`}>{content}</button>
              : <div className={className}>{content}</div>}
            <ClearButton issue={issue} />
          </div>;
        })}
      </Card>
    </Section>
  );
}

/** Every cleared finding, alerts included, in one place so each can be brought back. */
export function ClearedList({ issues, open = false }: { issues: FleetIssue[]; open?: boolean }) {
  const cleared = issues.filter((i) => i.cleared);
  if (cleared.length === 0) return null;
  return (
    <details className="group mt-3" open={open}>
      <summary className="flex w-fit cursor-pointer list-none items-center gap-1.5 text-[12.5px] text-ink-2 hover:text-ink">
        <span className="transition-transform group-open:rotate-90"><Icon name="chevron" size={11} /></span>
        Cleared <span className="font-mono tabular">{cleared.length}</span>
      </summary>
      <p className="mt-2 text-[12px] text-ink-3">Hidden until the problem changes, or until it comes back after it went away.</p>
      <Card className="mt-2 divide-y divide-line overflow-hidden">
        {cleared.map((issue) => <div key={issue.id} className="flex items-start gap-3 px-4 py-3">
          <div className="min-w-0 flex-1">
            <div className="truncate text-[13px] font-medium text-ink-2">{issue.title}</div>
            <div className="mt-0.5 line-clamp-1 break-words text-[12px] text-ink-3" title={issue.detail}>{issue.detail}</div>
          </div>
          <ClearButton issue={issue} />
        </div>)}
      </Card>
    </details>
  );
}
