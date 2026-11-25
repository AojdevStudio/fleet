import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { QueuedJobView } from "../../bindings/QueuedJobView";
import { previewFleet } from "../../lib/preview";
import { JobBoard } from "./JobBoard";

function copies(job: QueuedJobView, n: number): QueuedJobView[] {
  return Array.from({ length: n }, (_, i) => ({ ...job, job: { ...job.job, id: job.job.id * 100 + i, name: `${job.job.name}-${i}` } }));
}

describe("job board", () => {
  it("renders every queued job in its column", () => {
    const view = previewFleet();
    const waiting = view.queue.jobs.find((q) => !q.stuck)!;
    const stuck = view.queue.jobs.find((q) => q.stuck)!;
    view.queue.jobs = [...copies(waiting, 7), ...copies(stuck, 7)];
    const html = renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}><JobBoard view={view} /></QueryClientProvider>);
    for (const q of view.queue.jobs) expect(html).toContain(`${q.job.name}`);
    expect(html.split(stuck.stuck!.detail).length - 1).toBe(7);
  });
});
