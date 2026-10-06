;;; techne-test.el --- Tests for techne.el  -*- lexical-binding: t; -*-

;; Run (from the repository root, after building techne-node):
;;   TECHNE_NODE=target/release/techne-node emacs -Q --batch -L editors/emacs \
;;     -l editors/emacs/techne-test.el -f ert-run-tests-batch-and-exit

;;; Code:

(require 'ert)
(require 'techne)

(defun techne-test--wait (predicate &optional seconds)
  "Process input until PREDICATE holds (or SECONDS pass); return its value."
  (let ((deadline (+ (float-time) (or seconds 10))) result)
    (while (and (not (setq result (funcall predicate))) (< (float-time) deadline))
      (accept-process-output nil 0.05))
    result))

(defun techne-test--connect ()
  "Connect to a server started for these tests (once)."
  (unless (and techne--connection (process-live-p (techne--conn-process techne--connection)))
    (let ((techne-node-program (or (getenv "TECHNE_NODE") "techne-node"))
          (default-directory (make-temp-file "techne-test" t)))
      (techne-jack-in))
    (add-hook 'kill-emacs-hook #'techne-disconnect)))

(defun techne-test--eval (code)
  "Evaluate CODE and return its value string (nil if it has none)."
  (techne--field (techne-request-sync `(("op" . "eval") ("code" . ,code))) "value"))

(ert-deftest techne-bencode ()
  (let ((encoded (techne--bencode '(("op" . "eval") ("code" . "λ") ("id" . 3)))))
    (should (equal encoded (encode-coding-string "d4:code2:λ2:idi3e2:op4:evale" 'utf-8)))
    (should (equal (car (techne--bdecode encoded 0)) '(("code" . "λ") ("id" . 3) ("op" . "eval"))))
    (should-not (techne--bdecode (substring encoded 0 -1) 0))
    (should (equal (car (techne--bdecode "l1:ai2ee" 0)) '("a" 2)))))

(ert-deftest techne-eval-and-repl ()
  (techne-test--connect)
  (should (equal (techne-test--eval "(+ 1 2)") "3"))
  (with-current-buffer (techne--repl-buffer)
    (goto-char (point-max))
    (insert "(display \"hi\") (* 6 7)")
    (techne-repl-return)
    (should (techne-test--wait (lambda () (string-match-p "hi42\n" (buffer-string)))))
    ;; A new prompt follows the result.
    (should (techne-test--wait (lambda () (string-suffix-p "λ> " (buffer-string)))))))

(ert-deftest techne-completion-eldoc-xref ()
  (techne-test--connect)
  (let ((file (make-temp-file "techne-test" nil ".scm" "\n\n(define (area w h)\n  \"Area of a W by H rectangle.\"\n  (* w h))\n")))
    (with-current-buffer (find-file-noselect file)
      (techne-mode 1)
      (goto-char (point-max))
      (techne-eval-defun)
      (should (techne-test--wait (lambda () (techne--info "area"))))
      ;; Completion.
      (insert "(string-app")
      (let ((capf (techne-completion-at-point)))
        (should (member "string-append" (nth 2 capf))))
      ;; Eldoc for the call being written.
      (insert ")\n(area ")
      (let (doc)
        (techne-eldoc-function (lambda (s &rest _) (setq doc s)))
        (should (equal (techne-test--wait (lambda () doc)) "(area w h)")))
      ;; xref: the definition's file and line.
      (let ((def (car (xref-backend-definitions 'techne "area"))))
        (should (equal (xref-file-location-file (xref-item-location def)) file))
        (should (= (xref-file-location-line (xref-item-location def)) 3)))
      (set-buffer-modified-p nil)
      (kill-buffer))))

(ert-deftest techne-debugger ()
  (techne-test--connect)
  (techne-test--eval "(define (risky x) (restart-case (+ 1 (error \"boom\" x)) (use-value (v) v) (skip () 'skipped)))")
  (let (value)
    (techne-eval-string "(list (risky 5) 'after)" (lambda (v) (setq value v)))
    (should (techne-test--wait (lambda () (get-buffer "*techne-debug*"))))
    (with-current-buffer "*techne-debug*"
      (should (string-match-p "boom 5" (buffer-string)))
      (should (string-match-p "\\[0\\] use-value (v)" (buffer-string)))
      (should (string-match-p "\\[1\\] skip" (buffer-string))))
    ;; Evaluation still works while paused.
    (should (equal (techne-test--eval "(* 6 7)") "42"))
    (techne-debug-invoke-restart 0 "41")
    (should (equal (techne-test--wait (lambda () value)) "(41 after)"))
    (should-not (get-buffer "*techne-debug*"))))

(ert-deftest techne-inspector ()
  (techne-test--connect)
  (techne-inspect "(list 1 (vector 2 \"three\"))")
  (with-current-buffer "*techne-inspect*"
    (should (string-match-p "list of 2 elements" (buffer-string)))
    (should (string-match-p "1: #(2 \"three\")" (buffer-string))))
  (techne-inspector-part 1)
  (with-current-buffer "*techne-inspect*"
    (should (string-match-p "vector of 2 elements" (buffer-string)))
    (should (string-match-p "(depth 2)" (buffer-string))))
  (techne-inspector-pop)
  (with-current-buffer "*techne-inspect*"
    (should (string-match-p "list of 2 elements" (buffer-string)))))

;;; techne-test.el ends here
