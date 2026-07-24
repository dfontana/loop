;; Old YAML-style stringly-typed `:when` — must be rejected, not silently
;; coerced.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "a"}}
 :transitions [{:from "a" :to "a" :when "qa.result == 'pass'"}]}
