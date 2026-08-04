;; Both `:check` spellings, plus an edge with none.
{:ticket "PROJ-CHK"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:stage-prompt "a"} :b {:stage-prompt "b"}}
 :transitions [{:from "a" :to "b" :check "cargo test"}
               {:from "a" :to "done"}
               {:from "b" :to "done" :check {:cmd "sbt -batch compile" :timeout-s 600}}]}
