import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { QueuedJobView } from "../../bindings/QueuedJobView";
import { previewFleet } from "../../lib/preview";
import { JobBoard } from "./JobBoard";

function board(view: ReturnType<typeof previewFleet>): string {
  return renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}><JobBoard view={view} /></QueryClientProvider>);
}

function copies(job: QueuedJobView, n: number): QueuedJobView[] {
  return Array.from({ length: n }, (_, i) => ({ ...job, job: { ...job.job, id: job.job.id * 100 + i, name: `${job.job.name}-${i}` } }));
}

describe("job board", () => {
  it("renders every queued job in a bounded column that scrolls", () => {
    const view = previewFleet();
    const waiting = view.queue.jobs.find((q) => !q.stuck)!;
    const stuck = view.queue.jobs.find((q) => q.stuck)!;
    view.queue.jobs = [...copies(waiting, 7), ...copies(stuck, 7)];
    const html = board(view);
    for (const q of view.queue.jobs) expect(html).toContain(`${q.job.name}`);
    expect(html.split(stuck.stuck!.detail).length - 1).toBe(7);
    for (const title of ["Running now", "Up next", "Stuck"]) {
      expect(html).toMatch(new RegExp(`<div role="region" aria-label="${title}" tabindex="0" data-arrow-scroll="true" class="max-h-\\[360px\\][^"]*overflow-y-auto`));
    }
  });

  it("says matching is unknown before the runner list arrives", () => {
    const view = previewFleet();
    const waiting = view.queue.jobs.find((q) => !q.stuck)!;
    view.queue.jobs = [{ ...waiting, compatibleRunners: [], compatibleOnline: 0, compatibleIdle: 0, runnersKnown: false }];
    const html = board(view);
    expect(html).toContain("Runner matching isn&#x27;t available yet.");
    expect(html).not.toContain("all working");
  });
});
