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

  it("shows starting jobs in their own column, not as waiting", () => {
    const pools = (demo as unknown as FleetView).pools!;
    const html = renderToStaticMarkup(<Pools pools={{ ...pools, checkedAt: null }} />);
    expect(html).toContain(">Starting</th>");
    const row = html.split("<tr>").find((r) => r.includes(">central-linux-kvm<"))!;
    const cells = [...row.matchAll(/<td[^>]*>([^<]*)<\/td>/g)].map((m) => m[1]);
    // Pool, Places, Free, Busy, Unavailable, Waiting, Starting, Tart VMs
    expect(cells.slice(5, 7)).toEqual(["0", "1"]);
  });
});
