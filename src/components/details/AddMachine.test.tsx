import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { previewFleet } from "../../lib/preview";
import { AddMachine } from "./AddMachine";

describe("Add machine", () => {
  it("tells the user to check a device isn't already in the inventory before pasting its entry", () => {
    const view = previewFleet();
    const html = renderToStaticMarkup(<AddMachine view={view} />);
    expect(html).toContain("Devices on your tailnet that no inventory host names.");
    expect(html).toContain("isn&#x27;t already in the inventory under another name before you paste its entry");
    expect(html).toContain(view.tailnetDevices[0].inventoryEntry.split("\n")[0]);
    expect(html).toContain("Copy entry");
  });

  it("says why it lists no devices after a failed Tailscale poll", () => {
    const view = previewFleet();
    const failed = { ...view, tailnetDevices: [], tailnet: { ...view.tailnet, error: "Tailscale CLI failed" } };
    const html = renderToStaticMarkup(<AddMachine view={failed} />);
    expect(html).toContain("Tailscale couldn&#x27;t be read just now, so no devices are listed.");
    expect(html).not.toContain("Copy entry");
  });

  it("puts the failed read before the MagicDNS message", () => {
    const view = previewFleet();
    const both = { ...view, tailnetDevices: [], tailnet: { ...view.tailnet, error: "Tailscale CLI failed", magicDns: false } };
    const html = renderToStaticMarkup(<AddMachine view={both} />);
    expect(html).toContain("Tailscale couldn&#x27;t be read just now");
    expect(html).not.toContain("MagicDNS is off");
  });

  it("says MagicDNS is off and offers no entry", () => {
    const view = previewFleet();
    const off = { ...view, tailnetDevices: [], tailnet: { ...view.tailnet, magicDns: false } };
    const html = renderToStaticMarkup(<AddMachine view={off} />);
    expect(html).toContain("MagicDNS is off on this tailnet, so no entries are offered.");
    expect(html).not.toContain("Copy entry");
  });
});
