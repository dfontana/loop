;; A guard that returns a string instead of a boolean — also an authoring bug.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "a"}}
 :transitions [{:from "a" :to "a" :when (fn [v] "yes")}]}
