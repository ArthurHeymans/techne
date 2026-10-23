;; The closed host-input forms loop. Its bookkeeping stays interpreted;
;; user code retains its ordinary JIT policy. No top-level Scheme execution
;; initializes this helper.
(lambda (forms)
  (letrec ((wind
            (lambda (before body after)
              (before)
              (%push-wind after)
              (let ((value (body)))
                (%pop-handler) (after) value)))
           (load
            (lambda (forms)
              (wind (lambda () (%enter-module forms))
                    (lambda ()
                      (let ((value (evaluate forms)))
                        (%forms-evaluated forms) value))
                    (lambda () (%leave-module) (%module-loaded forms)))))
           (evaluate
            (lambda (forms)
              (let loop ((last (if #f #f)))
                (let ((next (%input-form forms)))
                  (if next
                      (if (pair? next) (begin (load (car next)) (loop last))
                          (loop (next)))
                      last))))))
    (evaluate forms)))
