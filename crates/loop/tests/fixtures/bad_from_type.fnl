;; `:from` is a number instead of a string — an unknown/wrong key value type.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "a"}}
 :transitions [{:from 42 :to "a"}]}
