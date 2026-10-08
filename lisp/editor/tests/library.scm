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

(test-failures)
