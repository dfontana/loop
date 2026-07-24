;; A guard that throws at call time — must surface as CoreError::Guard, not a
;; panic and not a silent `false`.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "a"}}
 :transitions [{:from "a" :to "a" :when (fn [v] (error "boom"))}]}
