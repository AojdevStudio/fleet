import type { FleetView } from "../bindings/FleetView";
import type { QueuedJobView } from "../bindings/QueuedJobView";
import type { RunnerView } from "../bindings/RunnerView";
import { allRunners } from "./nav";

/** Live CI work in three columns. Each busy runner and each queued job appears once. */
export interface JobBoard {
  running: RunnerView[];
  /** Queued jobs waiting normally, oldest first. */
  upNext: QueuedJobView[];
  /** Queued jobs the engine marked stuck, oldest first. */
  stuck: QueuedJobView[];
}

export function jobBoard(view: FleetView): JobBoard {
  return {
    running: allRunners(view).filter((r) => r.activity.kind === "busy"),
    upNext: view.queue.jobs.filter((q) => !q.stuck),
    stuck: view.queue.jobs.filter((q) => q.stuck),
  };
}
