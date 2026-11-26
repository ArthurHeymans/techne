;;; Headless editor tests for Slice 5: Org Babel execution (C-c C-c),
;;; tangling (C-c C-v t), result removal (C-c C-v k), and :eval no.
(import (techne test))
(test-file #:environment 'editor)
(require "../main.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../modes.scm")
(require "../api.scm")
(require "../builtins.scm")

(define-test "Org Babel execute, tangle, remove-result, and :eval no"
(call-with-temporary-directory
 (lambda (tmp)
   (let* ((org-path (string-append tmp "/demo.org"))
          (sample (string-append
                "* Babel Demo\n"
                "#+begin_src scheme :results value\n"
                "(* 6 7)\n"
                "#+end_src\n\n"
                "#+begin_src sh :tangle out.sh\n"
                "echo hello-from-sh\n"
                "#+end_src\n\n"
                "#+begin_src sh :eval no\n"
                "echo never\n"
                "#+end_src\n")))
  (make-directory tmp)
  (replace-file org-path sample)
  (let* ((d (file-document org-path))
         (s (start-session (make-view d "user") "emacs")))
    ;; 1. C-c C-c inside scheme block evaluates in-process and inserts #+RESULTS: : 42
    (let ((pos (string-contains (document-string d) "(* 6 7)")))
      (view-set-ranges! (session-view s) (list (list pos pos)) 0)
      (press-keys s "C-c C-c")
      (check-true "C-c C-c evaluated scheme block"
                  (and (string-contains (document-string d) "#+RESULTS:\n: 42\n") #t))
      ;; 2. C-c C-v k removes #+RESULTS:
      (press-keys s "C-c C-v k")
      (check "C-c C-v k removed #+RESULTS:" #f (string-contains (document-string d) "#+RESULTS:")))
    ;; 3. C-c C-c inside sh block runs subprocess and inserts result
    (let ((pos (string-contains (document-string d) "echo hello-from-sh")))
      (view-set-ranges! (session-view s) (list (list pos pos)) 0)
      (press-keys s "C-c C-c")
      (check-true "C-c C-c evaluated sh block"
                  (and (string-contains (document-string d) "#+RESULTS:\n: hello-from-sh\n") #t)))
    ;; 4. :eval no refuses execution
    (let ((pos (string-contains (document-string d) "echo never")))
      (view-set-ranges! (session-view s) (list (list pos pos)) 0)
      (press-keys s "C-c C-c")
      (check ":eval no refused" "Evaluation disabled by :eval no" (echo-line s)))
    ;; 5. C-c C-v t tangles out.sh next to demo.org
    (press-keys s "C-c C-v t")
    (let ((tangled (string-append tmp "/out.sh")))
      (check-true "tangled out.sh exists" (path-exists? tangled))
      (check "tangled out.sh content" "echo hello-from-sh\n" (file->string tangled))))))))
