;; A machine written against the old `:when` guard tier. The key is gone, and
;; ignoring it would silently leave the edge unguarded — so loading must fail
;; with a message that names the replacement.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:stage-prompt "a"}}
 :transitions [{:from "a" :to "a" :when "qa.result == 'pass'"}]}
