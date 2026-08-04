;; An empty check command would exit 0 and read as a gate that passed.
{:ticket "PROJ-CHK"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "a"}}
 :transitions [{:from "a" :to "done" :check "   "}]}
