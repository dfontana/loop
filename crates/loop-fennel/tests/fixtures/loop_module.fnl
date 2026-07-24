;; Exercises the `loop` runtime module (`transient?`, `real?`, `mins`, `secs`)
;; that eval::install_loop_module registers into package.preload.
(local {: transient? : real? : mins : secs} (require :loop))

{:transient (transient? {:error_class "transient"})
 :real-on-transient (real? {:error_class "transient"})
 :real-on-real (real? {:error_class "http_5xx"})
 :mins (mins 2)
 :secs (secs 45)}
