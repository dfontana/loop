;; Overrides only the worker model and the usd budget; everything else must
;; keep loop_core::Config::defaults' value.
{:worker {:model "claude-opus-5"}
 :budgets {:usd 25}}
