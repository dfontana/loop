;; No `:entry`, and two states, so there is no unambiguous default.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :terminals ["done"]
 :states {:a {:stage-prompt "a"}
          :b {:stage-prompt "b"}}
 :transitions [{:from "a" :to "b"}]}
