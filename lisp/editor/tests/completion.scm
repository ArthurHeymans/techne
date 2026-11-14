;;; M-SPC separates completion parts, matched in any order, as Corfu's.
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../modes.scm")
(require "../minibuffer.scm")
(require "../emacs.scm")
(require "../completion.scm")

;; A fixed source makes the expected matches independent of what names
;; happen to be loaded. Include names matching only one part, both orders,
;; and a name that the third part must remove.
(define-mode test-completion-mode "A fixed completion source for checks."
  #:keys '(("C-M-i" completion-at-point))
  #:complete (lambda (b p)
               (cons (car (identifier-span (buffer-document b) p))
                     (map candidate '("vector-ref" "ref-vector" "ref-vec" "list-ref" "vector-set!")))))

(define d (make-document ""))
(remember-buffer! (make-buffer d "*completion-test*" 'test-completion-mode))
(define s (make-session d "user" emacs-profile))
(set-current-session! s)

(define (text) (document-string d))
(define (shown)
  (let ((popup (editor-completion s)))
    (if popup
        (map (lambda (row)
               (apply string-append (map (lambda (run) (if (string? run) run (car run))) (car row))))
             (caddr popup))
        '())))

;; Open explicitly; auto-opening and its delay have separate coverage.
(type-text s "(ref")
(press-keys s "C-M-i")
(check "initial matches" '("vector-ref" "ref-vector" "ref-vec" "list-ref") (shown))
(press-keys s "M-SPC")
(check "the separator is inserted" "(ref " (text))
(check "the separator keeps the popup open"
       '("vector-ref" "ref-vector" "ref-vec" "list-ref") (shown))
(type-text s "vec")
(check "every part matches, in either order" '("vector-ref" "ref-vector" "ref-vec") (shown))

;; After M-SPC an ordinary space continues completing too. Check the
;; exact nonempty set, rather than a predicate that could pass on none.
(type-text s " tor")
(check "a typed space adds another part" '("vector-ref" "ref-vector") (shown))
(press-keys s "C-n RET")
(check "acceptance replaces every part, keeping the surrounding text" "(vector-ref" (text))
(check "acceptance closes the popup" #f (editor-completion s))

;; A new popup has not inherited the separator state of the old one.
(type-text s " (vec")
(press-keys s "C-M-i")
(check "completion opens again" '("vector-ref" "ref-vector" "ref-vec" "vector-set!") (shown))
(type-text s " ")
(check "without a separator a space still ends completion" #f (editor-completion s))
(check "the ordinary space stays in the document" "(vector-ref (vec " (text))
(check "no command errors" #f (sget s 'message))
(check "no contract violations" '() (take-violations! s))

(test-failures)
