import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { FleetIssue } from "../../bindings/FleetIssue";
import type { FleetIssueCategory } from "../../bindings/FleetIssueCategory";
import type { FleetIssueFix } from "../../bindings/FleetIssueFix";
import { Attention, ClearedList } from "./Attention";

function issue(id: string, detail: string, fix: FleetIssueFix | null = null, category: FleetIssueCategory = "inventory", cleared = false): FleetIssue {
  return { id, detail, title: id, action: `Do something about ${id}.`, fingerprint: detail, category, machineId: null, runnerKey: null,
    stale: false, muted: false, fix, cleared };
}

function render(node: ReactNode): string {
  return renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}>{node}</QueryClientProvider>);
}

describe("attention rows", () => {
  it("say what to do, and offer Clear but no link when there is nowhere to go", () => {
    const html = render(<Attention issues={[issue("inventory", "Inventory is invalid"), issue("tailnet", "Tailscale unavailable")]} />);
    expect(html).toContain("Inventory is invalid");
    expect(html).toContain("Do something about tailnet.");
    expect(html.match(/>Clear</g)).toHaveLength(2);
    expect(html.match(/<button/g)).toHaveLength(2);
  });

  it("keep actionable findings as keyboard accessible buttons beside Clear", () => {
    const html = render(<Attention issues={[issue("github", "Missing auth", "organizationSignIn"), issue("org", "No organization", "addOrganization")]} />);
    expect(html.match(/<button type="button"/g)).toHaveLength(4);
    expect(html).toContain("Add organization");
  });

  it("leave alerts, stuck jobs, history storage, the running change and cleared items to their own sections", () => {
    const issues = [issue("alert", "Offline", null, "alert"), issue("queue", "No runner", null, "queue"), issue("storage", "Can't save", null, "storage"),
      issue("operation", "Failed", null, "operation"), issue("gone", "Cleared", null, "inventory", true)];
    expect(render(<Attention issues={issues} />)).toBe("");
  });
});

describe("cleared list", () => {
  it("lists every cleared item, alerts included, with Bring back", () => {
    const issues = [issue("a", "Offline", null, "alert", true), issue("b", "Still here"), issue("c", "Hidden", null, "inventory", true)];
    const html = render(<ClearedList issues={issues} />);
    expect(html.match(/Bring back/g)).toHaveLength(2);
    expect(html).not.toContain("Still here");
    expect(render(<ClearedList issues={[issue("b", "Still here")]} />)).toBe("");
  });
});
