;;; Pure-Scheme tests for Slice 5: Org Babel header inheritance, :noweb,
;;; :tangle, Scheme execution, and lossless #+RESULTS: byte-splices.
(import (techne test))
(require "../org/syntax.scm")
(require "../org/edit.scm")
(require "../org/babel.scm")

(define-test "header-arg parsing and inheritance"
(let* ((doc (string-append
             "#+PROPERTY: header-args :tangle no :results output\n"
             "#+PROPERTY: header-args:scheme :results value\n"
             "* Parent\n"
             ":PROPERTIES:\n"
             ":header-args: :dir /tmp\n"
             ":END:\n"
             "** Child\n"
             "#+NAME: add-two\n"
             "#+HEADER: :var x=40\n"
             "#+begin_src scheme :tangle yes\n"
             "(+ 2 40)\n"
             "#+end_src\n"))
       (ol (org-parse-outline doc))
       (blks (org-babel-blocks doc ol))
       (b (car blks))
       (ps (org-src-block-params b)))
  (check "found 1 src block" 1 (length blks))
  (check "block name" "add-two" (org-src-block-name b))
  (check "block lang" "scheme" (org-src-block-lang b))
  (check "file lang-specific :results overrides generic" "value" (cdr (assoc ":results" ps)))
  (check "inherited ancestor :dir" "/tmp" (cdr (assoc ":dir" ps)))
  (check "inline :tangle overrides file" "yes" (cdr (assoc ":tangle" ps)))
  (check "HEADER :var preserved" "x=40" (cdr (assoc ":var" ps)))))

(define-test "tangling with :tangle, :shebang, and :noweb"
(let* ((doc (string-append
             "* Config\n"
             "#+NAME: helper\n"
             "#+begin_src elisp :tangle no\n"
             "(setq helper-loaded t)\n"
             "#+end_src\n"
             "#+begin_src elisp :tangle config.el :noweb yes\n"
             "<<helper>>\n"
             "(setq a 1)\n"
             "#+end_src\n"
             "#+begin_src elisp :tangle config.el\n"
             "(setq b 2)\n"
             "#+end_src\n"
             "#+begin_src bash :tangle run.sh :shebang \"#!/usr/bin/env bash\"\n"
             "echo hi\n"
             "#+end_src\n"))
       (ol (org-parse-outline doc))
       (plan (org-babel-tangle-plan doc ol #:source-path "/tmp/demo/init.org")))
  (check "2 tangle targets" 2 (length plan))
  (check "config.el path resolved relative to org file" "/tmp/demo/config.el" (car (car plan)))
  (check "config.el expanded noweb + joined both blocks"
         "(setq helper-loaded t)\n(setq a 1)\n\n(setq b 2)\n"
         (cdr (car plan)))
  (check "run.sh has shebang"
         "#!/usr/bin/env bash\necho hi\n"
         (cdr (cadr plan)))))

(define-test "Scheme evaluation, #+RESULTS: splicing, and flags"
(let* ((doc (string-append
             "* Eval\n"
             "#+begin_src scheme :results value\n"
             "(+ 20 22)\n"
             "#+end_src\n"))
       (ol (org-parse-outline doc))
       (blk (car (org-babel-blocks doc ol)))
       (out (org-babel-eval-scheme (org-src-block-body blk) (org-src-block-params blk)))
       (doc2 (org-apply-edits doc (org-babel-results-edits doc blk out))))
  (check "scheme value eval" "42" out)
  (check-true "inserted #+RESULTS: : 42" (and (string-contains doc2 "#+RESULTS:\n: 42\n") #t))
  ;; Re-evaluating replaces existing #+RESULTS: cleanly
  (let* ((ol2 (org-parse-outline doc2))
         (blk2 (car (org-babel-blocks doc2 ol2)))
         (doc3 (org-apply-edits doc2 (org-babel-results-edits doc2 blk2 "99"))))
    (check-true "replaced #+RESULTS: with 99" (and (string-contains doc3 "#+RESULTS:\n: 99\n") #t))
    (check "old 42 gone" #f (string-contains doc3 ": 42"))
    ;; Removing result deletes #+RESULTS: cleanly
    (let* ((ol3 (org-parse-outline doc3))
           (blk3 (car (org-babel-blocks doc3 ol3)))
           (doc4 (org-apply-edits doc3 (org-babel-remove-result-edits doc3 blk3))))
      (check "removed #+RESULTS:" #f (string-contains doc4 "#+RESULTS:")))))

;; :results output captures stdout; :results drawer wraps in :RESULTS: ... :END:
(let* ((doc "* Out\n#+begin_src scheme :results output drawer\n(display \"hello babel\")\n#+end_src\n")
       (ol (org-parse-outline doc))
       (blk (car (org-babel-blocks doc ol)))
       (out (org-babel-eval-scheme (org-src-block-body blk) (org-src-block-params blk)))
       (doc2 (org-apply-edits doc (org-babel-results-edits doc blk out))))
  (check "scheme output eval" "hello babel" out)
  (check-true "drawer result" (and (string-contains doc2 "#+RESULTS:\n:RESULTS:\nhello babel\n:END:\n") #t)))

;; :async and :eval no detection
(let* ((doc (string-append
             "* Flags\n"
             "#+begin_src sh :async\necho ok\n#+end_src\n"
             "#+begin_src sh :eval no\nrm -rf /\n#+end_src\n"))
       (ol (org-parse-outline doc))
       (blks (org-babel-blocks doc ol)))
  (check "detects bare :async" #t (org-babel-async? (car blks)))
  (check "detects :eval no" #t (org-babel-eval-disabled? (cadr blks)))))

(define-test "result removal preserves following heading, tangle safety, and Scheme isolation"
  ;; Removing #+RESULTS: immediately above a heading must NOT delete the heading!
  (let* ((doc "* One\n#+begin_src scheme\n42\n#+end_src\n\n#+RESULTS:\n: 42\n* Two\nKeep me\n")
         (ol (org-parse-outline doc))
         (blk (car (org-babel-blocks doc ol)))
         (out (org-apply-edits doc (org-babel-remove-result-edits doc blk #:outline ol))))
    (check-true "following heading * Two preserved" (and (string-contains out "* Two\nKeep me\n") #t))
    (check "old #+RESULTS: removed" #f (string-contains out "#+RESULTS:")))
  ;; :header-args:scheme: in heading :PROPERTIES: parses and inherits properly
  (let* ((doc "* H\n:PROPERTIES:\n:header-args:scheme: :results drawer\n:END:\n#+begin_src scheme\n1\n#+end_src\n")
         (ol (org-parse-outline doc))
         (blk (car (org-babel-blocks doc ol))))
    (check "heading :header-args:scheme: inherited" "drawer"
           (cdr (assoc ":results" (org-src-block-params blk)))))
  ;; Tangle merges ./out.sh and out.sh into one group and refuses overwriting source
  (let* ((doc "* T\n#+begin_src sh :tangle out.sh\na\n#+end_src\n#+begin_src sh :tangle ./out.sh\nb\n#+end_src\n")
         (ol (org-parse-outline doc))
         (plan (org-babel-tangle-plan doc ol #:source-path "/tmp/demo/init.org")))
    (check "out.sh and ./out.sh merged into 1 target" 1 (length plan))
    (check "merged content has both blocks" "a\n\nb\n" (cdr (car plan))))
  (let* ((doc "* T\n#+begin_src sh :tangle init.org\nboom\n#+end_src\n")
         (ol (org-parse-outline doc))
         (refused? (guard (e (#t #t))
                     (org-babel-tangle-plan doc ol #:source-path "/tmp/demo/init.org")
                     #f)))
    (check "refuses tangling over source Org file" #t refused?))
  ;; Scheme eval isolates definitions across independent runs
  (org-babel-eval-scheme "(define %babel-leak-test 123) %babel-leak-test" '((":results" . "value")))
  (let ((leaked? (guard (e (#t #f))
                   (org-babel-eval-scheme "%babel-leak-test" '((":results" . "value")))
                   #t)))
    (check "Scheme eval definitions do not leak across runs" #f leaked?))
  ;; Invalid :var name rejected
  (let ((bad-var? (guard (e (#t #t))
                    (org-babel-command-for
                     (make-org-src-block #f "sh" '((":var" . "a;rm -rf /=1")) "echo hi" 0 1 0 1 #f))
                    #f)))
    (check "rejects shell injection in :var name" #t bad-var?)))
