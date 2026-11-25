import { describe, expect, it } from "vitest";
import { jobBoard } from "./board";
import { previewFleet } from "./preview";

describe("job board", () => {
  it("shows each busy runner and each queued job once", () => {
    const view = previewFleet();
    const board = jobBoard(view);
    expect(board.running).toHaveLength(view.summary.runnersBusy);
    expect(board.upNext.length + board.stuck.length).toBe(view.summary.queuedJobs);
    const ids = [...board.upNext, ...board.stuck].map((q) => q.job.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("covers every stuck reason in the demo data", () => {
    const reasons = jobBoard(previewFleet()).stuck.map((q) => q.stuck?.reason).sort();
    expect(reasons).toEqual(["noLabelMatch", "otherRepository", "runnersOffline", "waitedPastLimit"]);
  });
});
