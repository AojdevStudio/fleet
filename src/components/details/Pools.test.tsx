import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { FleetView } from "../../bindings/FleetView";
import demo from "../../fixtures/demo-fleet.json";
import { Pools } from "./Pools";

describe("central pools", () => {
  it("shows each pool's place counts from the demo coordinator", () => {
    const pools = (demo as unknown as FleetView).pools!;
    const html = renderToStaticMarkup(<Pools pools={{ ...pools, checkedAt: null }} />);
    for (const name of ["central-linux", "central-linux-kvm", "central-macos"]) expect(html).toContain(name);
    expect(html).toContain("5 places");
    expect(html).toContain("1 of 2 tart VMs running");
  });
});
