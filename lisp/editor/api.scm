;;; The editor's interface for extensions, the library (techne editor):
;;;
;;;   (import (techne editor))
;;;
;;; Commands (define-command), keymaps, minor modes with highlight layers
;;; (define-mode), sessions and what commands act on. The runtime loads it
;;; with the editor (lisp/editor/main.scm).

(require "session.scm")
(require "commands.scm")
(require "emacs.scm")
(require "main.scm")

(provide define-command register-command! command command-names run-command message!
         define-mode register-mode! find-mode mode-names mode-on? toggle-mode!
         make-keymap define-key! lookup-key emacs-map
         sget sset! session-view session-document session-panes current-session
         doc ranges point move! edit! insert-text! search! region-text replace-region! search-all goto-next!
         find-file eval-region!)

(%name-library '(techne editor))
