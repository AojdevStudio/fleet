import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
describe("inventory that fails to load", () => {
  it("says why, with the error and the path, on the first screen with and without an organization", async () => {
    const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
    const { previewFleet } = await import("../../lib/preview");
    const { Overview } = await import("./Overview");
    const failed = previewFleet();
    failed.source = "live";
    failed.machines = [];
    failed.plan = { ...failed.plan, inventoryError: "No such file or directory", inventoryPath: "/fake/fleet.toml" };
    for (const view of [failed, { ...failed, orgs: [] }]) {
      const html = renderToStaticMarkup(
        <QueryClientProvider client={new QueryClient()}><Overview view={view} /></QueryClientProvider>,
      );
      expect(html).toContain("The inventory couldn&#x27;t be loaded");
      expect(html).toContain("No such file");
      expect(html).toContain("/fake/fleet.toml");
    }
  });
});
