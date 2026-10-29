;;; The library (techne editor) is not the application: loading it loads
;;; the interfaces, none of the editor's own features or keys.
;;;
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")
(require "../api.scm")

(check "the Emacs profile's own keys" 'forward-char (lookup-key emacs-map (kbd "C-f")))
(check "no application keys" #f (lookup-key emacs-map (kbd "M-x")))
(check "no application commands" #f (memq 'execute-extended-command (command-names)))
(define-command (hello s n) "Say hello." (message! s "hello"))
(check "an extension's command" #t (and (memq 'hello (command-names)) #t))

;;; Faces: declared partial attributes, inheritance, dotted names, and
;;; settings merging attribute by attribute.
(check "a built-in face's colour is a palette name" 'mauve (cdr (assq 'fg (face-attributes 'keyword))))
(check "a dotted name stands for its declared prefix" (face-attributes 'keyword) (face-attributes 'keyword.control.import))
(define-face check-face "A face for checks." #:inherit 'comment #:fg 'red)
(check "a declared face's own attribute" 'red (cdr (assq 'fg (face-attributes 'check-face))))
(check "what it does not set, it inherits" 'italic (cdr (assq 'slant (face-attributes 'check-face))))
(set-face! 'check-face #:fg 'blue)
(check "a setting wins the attribute it sets" 'blue (cdr (assq 'fg (face-attributes 'check-face))))
(check "and leaves the others" 'italic (cdr (assq 'slant (face-attributes 'check-face))))
(check "explain-face says where an attribute comes from" 'setting (cadr (assq 'fg (explain-face 'check-face))))
(unset-face! 'check-face)
(check "unset uncovers the declaration" 'red (cdr (assq 'fg (face-attributes 'check-face))))
(set-face! 'check-face #:slant #f)
(check "a setting of #f clears what the face inherits" #f (cdr (assq 'slant (face-attributes 'check-face))))
(unset-face! 'check-face)
(set-face! 'link.undeclared-check #:fg 'peach)
(check "a setting on an undeclared dotted name takes effect" 'peach (cdr (assq 'fg (face-attributes 'link.undeclared-check))))
(check "over what its prefix gives" 'line (cdr (assq 'underline (face-attributes 'link.undeclared-check))))
(unset-face! 'link.undeclared-check)
(set-face! 'link.extra #:fg 'red)
(define-face link.extra.child "A child of a settings-only face, for checks.")
(check "a settings-only name is a prefix like a declared one" 'red (cdr (assq 'fg (face-attributes 'link.extra.child))))
(unset-face! 'link.extra)
;; A scope takes back only its own setting.
(define theme (make-scope 'check-theme))
(with-scope theme (set-face! 'check-face #:fg 'green))
(unset-face! 'check-face)
(check "unset from a scope with no setting leaves another's" 'green (cdr (assq 'fg (face-attributes 'check-face))))
(with-scope theme (unset-face! 'check-face))
(check "its own scope takes it back" 'red (cdr (assq 'fg (face-attributes 'check-face))))
(check "explaining a face nothing declares says nothing" '() (explain-face 'no-such-face-at-all))

(test-failures)
