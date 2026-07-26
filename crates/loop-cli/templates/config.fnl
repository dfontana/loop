;; ~/.config/loop/config.fnl — global toolbox defaults.
;;
;; Every key is optional; whatever you leave out keeps its built-in default.
;; A per-ticket .loop/machine.fnl overrides anything here, and may only
;; *tighten* the budgets, never loosen them.

;; The provider every role falls back to; a role naming its own wins.
{:provider "anthropic"

 ;; The Worker: does the actual stage work. A machine state overrides this.
 :worker {:model "claude-sonnet-5" :thinking "medium"}

 ;; The two cheap agents that guard and reroute. Deliberately small: they
 ;; judge and route, they don't do the work.
 :judge     {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low"
             ;; Cap reconciliations so a stuck run escalates instead of
             ;; ping-ponging between two states.
             :max-invocations 5}

 ;; Skills loaded into every stage, before the machine's and the state's.
 ;; Usually empty: skills are situational, so most belong on the states that
 ;; need them rather than on everything.
 :default-skills []

 ;; MCP servers connected in every stage, by the name they carry in YOUR
 ;; ~/.pi/agent/mcp.json. loop never reads or writes that file — it only names
 ;; servers, and the stage's entry message asks the agent to `mcp({connect})`
 ;; each one, because the mcp extension starts every session with all servers
 ;; off. Usually empty for the same reason :default-skills is.
 :default-mcp []

 ;; What you have installed in pi, declared so `loop validate` can catch a
 ;; mismatch. This does NOT turn anything on: pi has no flag for enabling an
 ;; installed extension by name, and a worker spawn simply leaves pi's own
 ;; discovery alone. `mcp` has to be listed for any :mcp list to mean
 ;; anything; `loop validate` says so if it isn't.
 :pi-extensions ["mcp" "review-model-selector"]

 ;; Hard stops the harness enforces. Not suggestions to the agent.
 :budgets {:usd 15 :wallclock-s 7200 :max-transitions 60}

 ;; How many recent committed transitions the rolling digest lists. That
 ;; digest is the whole continuity channel between stages, and it only
 ;; reaches an agent where a playbook interpolates $LEDGER_DIGEST.
 :digest-last-n 8

 ;; "constrained" — the transition tool's `to` is an enum of the current
 ;; state's neighbors, so a worker cannot name an invalid edge.
 ;; "open" — `to` is a free string and unknown targets go to the Navigator.
 :transition-mode "constrained"}
