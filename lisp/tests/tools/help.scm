;;; Run by Cargo in a VM without file/process grants, not by the full-grant CLI.
(require "support.scm")
(import (techne tools))
(define (help-fixture x) "A live help fixture." x)

(tool-test-case "help is bounded, live, and needs no file/process grants"
  (lambda ()
    (define h (tool-help))
    (expect #t (< (string-length (tool-show h)) 1800))
    (expect #t (and (string-contains (tool-show h) "\n(tool-read project PATH") #t))
    (expect #t (and (string-contains (tool-ref h 'cli) "techne tool [--text] ROOT FORM") #t))
    (expect #f (tool-ref h 'recipes))
    (expect #t (pair? (tool-ref h 'signatures)))
    (expect 23 (length (tool-help 'recipes)))
    (expect #t (and (tool-ref (tool-help 'guide) 'limits) #t))
    (expect #t (string? (tool-ref h 'purpose)))

    (expect "A live help fixture." (tool-ref (tool-help 'help-fixture) 'text))
    (expect #t (pair? (tool-ref (tool-help "help fixture") 'items)))
    ;; Words in names and first doc lines outrank every word somewhere in
    ;; a long doc: scope-procedure's mentions processes, runs and a command.
    (define found (map (lambda (i) (tool-ref i 'name)) (tool-ref (tool-help "process run command") 'items)))
    ;; Which comes first depends on the world's bindings (a node's
    ;; node-processes ties with tool-run), but scope-procedure trails them.
    (for-each (lambda (name) (expect #t (and (memq name found) #t))) '(tool-run tool-process-result tool-check))
    (expect #t (every (lambda (name) (> (length (memq name found)) (length (or (memq 'scope-procedure found) '()))))
                      '(tool-run tool-process-result tool-check)))
    ;; A word starts a word: run is not in truncated.
    (expect #f (and (memq 'tool-describe (map (lambda (i) (tool-ref i 'name)) (tool-ref (tool-discover "run" #:limit 50) 'items))) #t))
    (expect #t (fails (lambda () (tool-project "/"))))))
(test-summary)
