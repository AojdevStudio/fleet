import type { RolloutRepoView } from "../bindings/RolloutRepoView";
import type { RolloutState } from "../bindings/RolloutState";
import type { Tone } from "./status";

/** Repositories still moving: every state before merged. */
export function movingRepos(repos: RolloutRepoView[]): RolloutRepoView[] {
  return repos.filter((r) => r.state !== "merged");
}

/** Repositories that leave long-lived runners behind. */
export function retiringRepos(repos: RolloutRepoView[]): RolloutRepoView[] {
  return repos.filter((r) => r.oldRunners.length > 0);
}

export const ROLLOUT_STATES: RolloutState[] = ["registered", "pr-open", "green", "merged"];

export function rolloutStateLabel(state: RolloutState): { label: string; tone: Tone } {
  switch (state) {
    case "registered":
      return { label: "Registered", tone: "muted" };
    case "pr-open":
      return { label: "PR open", tone: "busy" };
    case "green":
      return { label: "Green", tone: "ok" };
    case "merged":
      return { label: "Merged", tone: "ok" };
  }
}
