/**
 * verdict-tool.ts — the ONLY tool the Judge agent gets.
 *
 * This is a loop-specific harness extension (NOT one of the pi-extensions in
 * ~/opencode/pi-extensions — those are installed packages the Worker uses;
 * transition/verdict/choose are loop's own, vendored in the toolbox's ext/).
 *
 * The Judge is spawned with `--no-builtin-tools -e verdict-tool.ts` (see
 * docs/05-orchestration.md), so it has no way to read/edit/deploy — only to
 * return a pass/fail on the transition's `criteria`, given the worker-output
 * digest and artifact paths passed in the kickoff message. Keeping the verdict a
 * structured tool call (not free prose) is what lets the harness gate on it.
 */

import { Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.registerTool({
    name: "verdict",
    label: "verdict",
    description:
      "Return your independent verdict on whether the stated criteria are met by " +
      "the evidence provided. Call exactly once. Base it only on the artifacts and " +
      "outputs given — you have no tools to gather more.",
    parameters: Type.Object({
      pass: Type.Boolean({
        description: "True only if every part of the criteria is satisfied by the evidence.",
      }),
      rationale: Type.String({
        description: "Cite the specific evidence (artifact, command output, diff hunk) behind the verdict.",
      }),
    }),
    async execute(_id, params) {
      const p = params as { pass: boolean; rationale: string };
      // The harness scrapes this marker into a `guard_checked` ledger event.
      const payload = JSON.stringify({ pass: !!p.pass, rationale: p.rationale });
      return {
        content: [{ type: "text" as const, text: `LOOP_VERDICT ${payload}` }],
        details: undefined,
      };
    },
  });
}
