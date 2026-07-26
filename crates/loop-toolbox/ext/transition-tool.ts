/**
 * transition-tool.ts — the one tool the harness injects into every Worker spawn.
 *
 * Calling it ends the stage: the worker declares where it wants to go next (or
 * that it's blocked), with a rationale and any artifacts to hand off. The
 * harness reads the call arguments off the JSONL event stream, writes a
 * `transition_proposed` ledger event, and runs the guard checks.
 *
 * Two schema modes, chosen per machine (see docs/02-how-it-works.md):
 *   - CONSTRAINED (default): `to` is an enum of the current state's reachable
 *     neighbors, injected via LOOP_REACHABLE. The model literally cannot name an
 *     invalid edge; the Navigator only fires on explicit `blocked`.
 *   - OPEN: `to` is a free string; the harness routes unknown targets to the
 *     Navigator. More faithful to "the CLI decides validity, else reconcile".
 *
 * A thin, validated wrapper the harness
 * fully controls, so the important decision travels as structured data, never
 * as free-text the harness has to parse out of prose.
 */

import { Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// Injected by the harness at spawn time.
const REACHABLE = (process.env.LOOP_REACHABLE ?? "").split(",").filter(Boolean);
const MODE = process.env.LOOP_TRANSITION_MODE ?? "constrained"; // "constrained" | "open"

export default function (pi: ExtensionAPI) {
  const toSchema =
    MODE === "constrained" && REACHABLE.length > 0
      ? Type.Union(REACHABLE.map((s) => Type.Literal(s)), {
          description: "The next state to move to. Must be a reachable neighbor.",
        })
      : Type.String({ description: "The next state to move to." });

  pi.registerTool({
    name: "transition",
    label: "transition",
    description:
      "End this stage and declare the next state. Call exactly once, when the " +
      "stage's goal is met. If you cannot make progress, set blocked=true with a " +
      "precise rationale and the harness will route you or escalate.",
    parameters: Type.Object({
      to: Type.Optional(toSchema),
      blocked: Type.Optional(
        Type.Boolean({ description: "True if you cannot reach a valid next state; requires a rationale." }),
      ),
      rationale: Type.String({ description: "Why this is the right next step, or precisely what blocks you." }),
      artifacts: Type.Optional(
        Type.Array(
          Type.Object({
            name: Type.String(),
            path: Type.String({ description: "Path to a file to hand off to later stages." }),
          }),
          { description: "Outputs later stages should receive (diffs, reports, samples)." },
        ),
      ),
    }),
    async execute(_id, params) {
      const p = params as {
        to?: string;
        blocked?: boolean;
        rationale: string;
        artifacts?: { name: string; path: string }[];
      };
      if (!p.blocked && !p.to) {
        throw new Error("transition requires either `to` or `blocked: true`.");
      }
      // The harness watches for this marker on the event stream, records the
      // proposal, and takes over control flow. Emitting it as the tool result
      // keeps the contract explicit and greppable.
      const payload = JSON.stringify({
        to: p.to ?? null,
        blocked: !!p.blocked,
        rationale: p.rationale,
        artifacts: p.artifacts ?? [],
      });
      return {
        content: [{ type: "text" as const, text: `LOOP_TRANSITION ${payload}` }],
        details: undefined,
      };
    },
  });
}
