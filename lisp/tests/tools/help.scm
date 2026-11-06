;;; Run by Cargo in a VM without file/process grants, not by the full-grant CLI.
(require "support.scm")
(import (techne tools))
(define (help-fixture x) "A live help fixture." x)

(tool-test-case "help is bounded, live, and needs no file/process grants"
  (lambda ()
    (define h (tool-help))
    (expect #t (< (string-length (tool-show h)) 1800))
    (expect #t (and (string-contains (tool-show h) "\n(tool-read project PATH") #t))
    (expect #f (tool-ref h 'recipes))
    (expect #t (pair? (tool-ref h 'signatures)))
    (expect 22 (length (tool-help 'recipes)))
    (expect #t (and (tool-ref (tool-help 'guide) 'limits) #t))
    (expect #t (string? (tool-ref h 'purpose)))

    (expect "A live help fixture." (tool-ref (tool-help 'help-fixture) 'text))
    (expect #t (pair? (tool-ref (tool-help "help fixture") 'items)))
    (expect #t (fails (lambda () (tool-project "/"))))))
(test-summary)
