import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { FleetView } from "../../bindings/FleetView";
import demo from "../../fixtures/demo-fleet.json";
import { retiringRepos } from "../../lib/rollout";
import { RetiringDetail } from "./Rollout";

describe("retiring view", () => {
  it("calls an old runner with no linked activity unlinked, not absent from GitHub", () => {
    const view = structuredClone(demo) as unknown as FleetView;
    const repo = retiringRepos(view.rollout)[0];
    // An ambiguous name (two runners share it) leaves the runner unlinked.
    repo.oldRunners[0] = { ...repo.oldRunners[0], runnerKey: null, activity: null };
    const html = renderToStaticMarkup(<RetiringDetail view={view} />);
    expect(html).toContain("Not linked");
    expect(html).not.toContain("Not listed by GitHub");
  });
});
