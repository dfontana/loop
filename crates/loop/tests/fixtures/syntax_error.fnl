;; Deliberately unbalanced: the `(fn ...)` form is never closed before the
;; enclosing table literal is. Should surface a Fennel parse/compile error
;; naming this file and a line number.
{:ticket "PROJ-BAD"
 :entry "a"
 :states {:a {:stage-prompt "a"}}
 :transitions [{:from "a" :to "a"
                :when (fn [v] (= v.qa.result "pass")}]}
