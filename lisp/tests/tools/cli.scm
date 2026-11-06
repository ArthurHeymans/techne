;;; Exercise the REAL CLI in child processes, not a substitute renderer.
;;; Usage: techne run lisp/tests/tools/cli.scm /absolute/path/to/techne
(require "support.scm")
(import (techne tools))

(define binary
  (if (= 2 (length (command-line))) (cadr (command-line))
      (error "pass the techne binary to tools/cli.scm")))

(define (written datum)
  (call-with-output-string (lambda (port) (write datum port))))

(define (occurrences text needle)
  (let loop ((rest text) (n 0))
    (let ((at (string-contains rest needle)))
      (if at
          (loop (substring rest (+ (string-cursor->index rest at) (string-length needle)) (string-length rest)) (+ n 1))
          n))))

(define (with-cli-session name body)
  (tool-test-case name
    (lambda ()
      (call-with-tool-directory
        (lambda (root)
          (define p (tool-project root))
          (define node-dir (string-append root "/node"))
          (define session "scheme-tool-test")
          (create-directory node-dir #o700)
          (define (invoke args)
            (let ((result (tool-process-result
                    (tool-run p binary args #:environment (list (cons "TECHNE_NODE_DIR" node-dir))
                              #:retain 16000 #:keep 1000000 #:timeout 30)
                    #:chars 8000)))
              (unless (eqv? 0 (tool-ref result 'status)) (error "tool CLI failed" (tool-show result)))
              (tool-ref (tool-ref result 'stdout) 'text)))
          (define (query form) (string-trim (invoke (list "tool" root form session))))
          (define (text form) (invoke (list "tool" "--text" root form session)))
          ;; Explicit shutdown waits for the node before deleting its socket directory.
          (fixture-write root "cleanup.scm"
            (written `(begin
              (define n (node-connect (list ,binary "node" "--session" ,session)))
              (node-eval n "0")
              (node-shutdown n)
              (guard (e (#t #t)) (node-eval n "0")))))
          (dynamic-wind (lambda () #t) (lambda () (body root query text))
                        (lambda () (invoke (list "run" (string-append root "/cleanup.scm"))))))))))

(with-cli-session "CLI renders source replies, batches and immutable Unicode expansions"
  (lambda (root query text)
    (fixture-write root "source.rs" "zero\né😀\nlast")
    (query "(define saved (tool-read project \"source.rs\" #:from 2 #:lines 1 #:chars 1))")
    (define handle (query "(tool-ref saved 'handle)"))
    (define shown (text "saved"))
    (expect #t (string-prefix? "[snapshot file-lines: 3; \"source.rs\"; revision " shown))
    (expect #t (and (string-contains shown "]\n2: é\n") (not (string-contains shown "source.rs:2:"))))
    (define revision (read (open-input-string (query "(tool-ref saved 'revision)"))))
    (expect #t (and (string-contains shown revision) #t))
    (expect #t (and (string-contains shown "[truncated region:") (string-contains shown "#:offset 1") #t))
    (define batch (text "(list (tool-read project \"source.rs\" #:from 1 #:lines 1) (tool-read project \"source.rs\" #:from 3 #:lines 1))"))
    (expect 2 (occurrences batch "[snapshot file-lines: 3;"))
    (expect #t (and (string-contains batch "]\n1: zero") (string-contains batch "]\n3: last") #t))
    (define mixed (text "(list (tool-read project \"source.rs\" #:from 3 #:lines 1) \"tail\")"))
    (expect #t (and (string-contains mixed "]\n3: last") (string-suffix? "\ntail\n" mixed)))
    (expect "zero\né😀\nlast\n" (text "(tool-show (tool-read project \"source.rs\" #:lines 3) #:numbered #f)"))
    (define structured (query "(tool-read project \"source.rs\" #:from 2 #:lines 1)"))
    (expect #t (and (string-prefix? "((path " structured) (string-contains structured "(text \"é😀\\n\")") #t))
    (fixture-write root "source.rs" "changed")
    (define expanded (text (string-append "(tool-expand project " handle " #:offset 1)")))
    (expect #t (and (string-contains expanded "]\n2: 😀") (not (string-contains expanded "changed"))))
    (expect #t (and (string-contains expanded revision) #t))))

(with-cli-session "CLI preserves ordinary data and process labels"
  (lambda (root query text)
    (expect "42\n" (text "42"))
    (expect "#t\n" (text "#t"))
    (expect "(#t 7 \"ok\")\n" (text "'( #t 7 \"ok\")"))
    (expect "((path \"source.rs\") (text \"not a snapshot\"))\n"
      (text "'((path \"source.rs\") (text \"not a snapshot\"))"))
    (expect "((x 1 2))\n" (text "'((x 1 2))"))
    (expect "plain\ntext\n" (text "\"plain\\ntext\""))
    (define output (text "(let* ((job (tool-run project \"printf\" '(\"captured\\n\") #:keep 64)) (result (tool-process-result job #:wait 2))) (tool-expand project (tool-ref (tool-ref result 'stdout) 'handle)))"))
    (expect #t (and (string-contains output "(file-lines #f)") (string-contains output "(text \"captured\\n\")") #t))
    (expect #f (and (string-contains output "[snapshot file-lines:") #t))))
(test-summary)
