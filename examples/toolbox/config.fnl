;; ~/.config/loop/config.fnl — global toolbox defaults.
;;
;; Every key is optional; whatever you leave out keeps its built-in default.
;; A per-ticket .loop/machine.fnl overrides anything here, and may only
;; *tighten* the budgets, never loosen them.

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

 ;; Baseline tools every stage gets before its own allowlist is added.
 :default-tools ["read" "bash"]

 ;; Installed pi-extension packages activated per spawn. These are NOT files
 ;; loop ships — they live in your pi extension settings. loop points
 ;; scoped-tools and mcp at the staged agent dir via PI_AGENT_DIR.
 :pi-extensions ["scoped-tools" "mcp" "review-model-selector"]

 ;; Hard stops the harness enforces. Not suggestions to the agent.
 :budgets {:usd 15 :wallclock-s 7200 :max-transitions 60}

 ;; How much prior context each stage sees: "digest" (a rolling summary the
 ;; harness assembles) or "full" (every worker output verbatim; expensive).
 :context "digest"
 :digest-last-n 8

 ;; "constrained" — the transition tool's `to` is an enum of the current
 ;; state's neighbors, so a worker cannot name an invalid edge.
 ;; "open" — `to` is a free string and unknown targets go to the Navigator.
 :transition-mode "constrained"}
