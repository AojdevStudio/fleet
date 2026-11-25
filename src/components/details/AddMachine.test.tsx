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
});
