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
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "lens.scm")
(require "main.scm")

(provide define-command register-command! command command-names run-command message!
         define-mode register-mode! find-mode mode-names mode-on? toggle-mode!
         make-keymap define-key! lookup-key kbd emacs-map minibuffer-map
         sget sset! session-view session-document session-panes current-session
         doc ranges point move! edit! insert-text! search! region-text replace-region! search-all goto-next!
         file-document show-document! visit! buffer-list buffer-name add-buffer! eval-region!
         completing-read candidate candidate-text candidate-annotation candidate-target take-target
         target target? target-kind target-value define-action actions-for act-on!
         location file-location location? location-document location-position line-candidate
         default-directory file-name show-lens! row define-view)

(%name-library '(techne editor))
