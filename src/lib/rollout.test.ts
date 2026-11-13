import { describe, expect, it } from "vitest";
import type { FleetView } from "../bindings/FleetView";
import demo from "../fixtures/demo-fleet.json";
import { movingRepos, retiringRepos } from "./rollout";

describe("rollout lists", () => {
  const rollout = (demo as unknown as FleetView).rollout;
  it("keeps unmerged repositories in the moving list", () => {
    expect(movingRepos(rollout).map((r) => r.repo)).toEqual(["acme-demo/ios-app", "acme-demo/infra", "acme-demo/ml"]);
  });
  it("lists only repositories with old runners as retiring", () => {
    expect(retiringRepos(rollout).map((r) => r.repo)).toEqual(["acme-demo/web", "acme-demo/ios-app", "acme-demo/infra"]);
  });
});
