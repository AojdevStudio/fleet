import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { FleetView } from "../../bindings/FleetView";
import demo from "../../fixtures/demo-fleet.json";
import { Pools } from "./Pools";

describe("central pools", () => {
  it("shows each pool's place counts from the demo coordinator", () => {
    const pools = (demo as unknown as FleetView).pools!;
    const html = renderToStaticMarkup(<Pools pools={{ ...pools, checkedAt: null }} view={demo as unknown as FleetView} />);
    for (const name of ["central-linux", "central-linux-kvm", "central-macos"]) expect(html).toContain(name);
    expect(html).toContain("5 places");
    expect(html).toContain("1 of 2 tart VMs running");
  });

  it("shows starting jobs in their own column, not as waiting", () => {
    const pools = (demo as unknown as FleetView).pools!;
    const html = renderToStaticMarkup(<Pools pools={{ ...pools, checkedAt: null }} view={demo as unknown as FleetView} />);
    expect(html).toContain(">Starting</th>");
    const row = html.split("<tr>").find((r) => r.includes(">central-linux-kvm<"))!;
    const cells = [...row.matchAll(/<td[^>]*>([^<]*)<\/td>/g)].map((m) => m[1]);
    // Pool, Places, Free, Busy, Unavailable, Waiting, Starting, Tart VMs
    expect(cells.slice(5, 7)).toEqual(["0", "1"]);
  });

  it("lists pool runners under the place their name names", () => {
    const view = demo as unknown as FleetView;
    const html = renderToStaticMarkup(<Pools pools={view.pools!} view={view} />);
    const place = html.indexOf(">ci-linux-1<");
    expect(place).toBeGreaterThan(-1);
    expect(html.indexOf("acme-demo/web · runner-up-ci-linux-1-1795573483")).toBeGreaterThan(place);
    expect(html).toContain(">ci-kvm-1<");
    expect(html).not.toContain(">ci-linux-2<");
  });
});
