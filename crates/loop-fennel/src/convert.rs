//! Lua table → `loop-core` IR.
//!
//! # The authored `machine.fnl` shape (v1)
//!
//! A machine file evaluates to a table. Keyword keys, kebab-case state ids,
//! guards as plain functions of one argument (the vars table).
//!
//! ```fennel
//! {:ticket "PROJ-1487"
//!  :task "task.md"                  ; path relative to machine.fnl, or inline prose
//!  :plan "plan.md"
//!  :qa-cases [{:id "pipeline" :desc "…"}
//!             {:id "contract" :desc "…"}]
//!
//!  :defaults {:model "claude-sonnet-5" :thinking "medium" :tools ["read" "bash"]}
//!  :budgets  {:usd 8 :wallclock-s 5400 :max-transitions 40}
//!  :judge     {:model "claude-haiku-4-5" :thinking "low"}
//!  :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}
//!
//!  :entry "implement"
//!  :terminals ["done" "blocked"]
//!  :escalation-state "blocked"
//!  :transition-mode "constrained"   ; or "open"
//!
//!  :states
//!  {:implement  {:playbook "implement" :thinking "high"
//!                :tools ["edit" "write" "spark_build"]
//!                :description "Implement the plan; keep the build green."}
//!   :qa-staging {:playbook "qa" :thinking "high"
//!                :tools ["staging_deploy" "spark_run" "fetch_job_output"]
//!                :exclude-tools ["edit" "write"]}}
//!
//!  :transitions
//!  [{:from "implement" :to "review"
//!    :criteria "The plan's items are addressed, the build is green, no TODOs remain."
//!    :on-fail "retry"}
//!   {:from "qa-staging" :to "validate-contract"
//!    :when (fn [v] (= v.qa.result "pass"))}
//!   {:from "qa-staging" :to "qa-staging"
//!    :when (fn [v] (and (= v.qa.result "fail") (= v.qa.error_class "transient")))
//!    :backoff-s 30 :on-fail "abort"}
//!   {:from "qa-staging" :to "debug"
//!    :when (fn [v] (and (= v.qa.result "fail") (not= v.qa.error_class "transient")))}]
//!
//!  :loops
//!  [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}]}
//! ```
//!
//! Notes that matter for the conversion:
//! - `:when` is a **function**, not a string. It receives the vars table and
//!   must return a boolean. It is stored in the Lua registry behind a
//!   [`loop_core::GuardRef`]; `when-src` is best-effort source text for the
//!   ledger (`string.dump` is not readable, so record the transition's index
//!   and any `:when-doc` string the author supplied).
//! - `:on-fail` is `"retry"` | `"abort"` | `{:route "state-id"}`.
//! - `:model`/`:thinking`/`:provider` are all optional at every level — leave
//!   them `None` and let [`loop_core::Machine::resolve_model`] stack the layers.
//!   Do **not** eagerly fill in defaults here; the playbook frontmatter layer
//!   sits between the state and the machine defaults.
//! - Missing `:entry` defaults to the first key of `:states` only if there is
//!   exactly one; otherwise it is an error. Silent guessing is worse than a
//!   clear message.
//!
//! # `config.fnl`
//!
//! Same idea, matching [`loop_core::Config`]:
//!
//! ```fennel
//! {:provider "anthropic"
//!  :worker    {:model "claude-sonnet-5"  :thinking "medium"}
//!  :judge     {:model "claude-haiku-4-5" :thinking "low"}
//!  :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}
//!  :default-tools ["read" "bash"]
//!  :pi-extensions ["scoped-tools" "mcp" "review-model-selector"]
//!  :budgets {:usd 15 :wallclock-s 7200 :max-transitions 60}
//!  :context "digest" :digest-last-n 8
//!  :transition-mode "constrained"}
//! ```
//!
//! Every key is optional; whatever is absent keeps its [`loop_core::Config::defaults`]
//! value. Kebab-case in Fennel maps to snake_case in Rust (`:max-invocations` →
//! `navigator_max_invocations`).

use std::path::Path;

use loop_core::{Config, Machine, Result};

/// Convert an evaluated machine table into the IR, resolving `:task`/`:plan`
/// paths against `machine_dir` and registering guards.
///
/// TASK T2.
pub fn machine_from_table(
    table: &mlua::Table,
    machine_dir: &Path,
    source_hash: String,
    source_path: &Path,
    config: &Config,
) -> Result<Machine> {
    let _ = (table, machine_dir, source_hash, source_path, config);
    todo!("T2")
}

/// Overlay an evaluated config table onto [`Config::defaults`].
///
/// TASK T2.
pub fn config_from_table(table: &mlua::Table, base: Config) -> Result<Config> {
    let _ = (table, base);
    todo!("T2")
}
