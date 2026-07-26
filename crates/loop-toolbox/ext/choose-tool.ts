/**
 * choose-tool.ts — the ONLY tool the Navigator agent gets.
 *
 * A loop-specific harness extension (like verdict-tool.ts / transition-tool.ts;
 * NOT a pi-extensions package). The Navigator fires when a Worker proposed an
 * out-of-graph target or set `blocked=true`. It picks a reachable next state and
 * writes a short entry-prompt addendum that becomes `$ENTRY_ADDENDUM` in the next
 * stage (docs/02-how-it-works.md).
 *
 * `to` is a constrained enum of reachable states plus `escalate`, injected via
 * LOOP_REACHABLE — so the Navigator, like the Worker, cannot name an edge that
 * doesn't exist. It routes within the declared graph or escalates; it never
 * invents structure.
 */

import { Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// Injected by the harness: reachable neighbors of the stuck state, plus the
// always-available `escalate` sink.
const CHOICES = [
  ...(process.env.LOOP_REACHABLE ?? "").split(",").filter(Boolean),
  "escalate",
];

export default function (pi: ExtensionAPI) {
  const toSchema = Type.Union(
    CHOICES.map((s) => Type.Literal(s)),
    { description: "The state to route to. Must be a reachable neighbor, or `escalate` if none fits." },
  );

  pi.registerTool({
    name: "choose",
    label: "choose",
    description:
      "Pick the next state for a worker that couldn't route itself, and write a " +
      "short note telling that stage how to get back on track. Call exactly once.",
    parameters: Type.Object({
      to: toSchema,
      entry_prompt: Type.String({
        description:
          "A few sentences appended to the next stage's prompt ($ENTRY_ADDENDUM): what went wrong and what to do differently.",
      }),
    }),
    async execute(_id, params) {
      const p = params as { to: string; entry_prompt: string };
      const payload = JSON.stringify({ to: p.to, entry_prompt: p.entry_prompt });
      return {
        content: [{ type: "text" as const, text: `LOOP_CHOICE ${payload}` }],
        details: undefined,
      };
    },
  });
}
