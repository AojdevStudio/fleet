import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { previewFleet } from "../../lib/preview";
import { AddMachine } from "./AddMachine";

describe("Add machine", () => {
  it("says why it is unavailable, names the host, and offers no entry to copy", () => {
    const view = previewFleet();
    view.addMachine = { state: "waiting", hosts: [{ hostId: "studio", reason: "ssh = \"${STUDIO_SSH}\" is unset in the app's environment." }] };
    const html = renderToStaticMarkup(<AddMachine view={view} />);
    expect(html).toContain("Unavailable until the app knows which tailnet device each inventory host is");
    expect(html).toContain("studio");
    expect(html).toContain("STUDIO_SSH");
    expect(html).not.toContain("Copy entry");
  });
});
