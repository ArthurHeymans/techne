;;; The editor's interface for extensions, the library (techne editor):
;;;
;;;   (import (techne editor))
;;;
;;; Commands (define-command), keymaps, buffers, major modes (define-mode)
;;; and minor modes with highlight layers (define-minor-mode), options
;;; (define-option, set-option!), hooks (add-hook!), sessions and what
;;; commands act on. It is the library the application (main.scm) is
;;; built with, not the application: importing it loads none of the
;;; editor's own features or key bindings.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")
(require "lens.scm")
(require "live.scm")
(require "requests.scm")

(provide define-command register-command! command command-names run-command message!
         define-mode register-mode! define-minor-mode register-minor-mode! find-mode mode-names mode-map mode-on? toggle-mode!
         mode-name mode-doc mode-parent mode-chain derived-mode? mode-for-file
         define-option register-option! set-option! unset-option! option explain-option
         define-hook add-hook! remove-hook! run-hook!
         buffer? buffer-document buffer-name buffer-mode buffer-state document-buffer current-buffer
         make-keymap define-key! name-prefix! lookup-key keymap-sequences kbd emacs-map minibuffer-map
         sget sset! session-view session-document session-panes current-session
         doc ranges point move! edit! insert-text! search! region-text replace-region! search-all goto-next!
         kill-ring kill-save! yank-text current-prefix
         file-document show-document! show-buffer! visit! buffer-list add-buffer! eval-region!
         completing-read candidate candidate-text candidate-annotation candidate-target take-target
         target target? target-kind target-value target-at define-action actions-for act-on!
         location file-location location? location-document location-position line-candidate
         default-directory file-name show-lens! row excerpt define-view show-view! present! row-at row-target
         error-text make-generated-buffer! buffer-named set-buffer-state! session-buffer display-buffer!
         make-request-slot request! request-pending? cancel-request!
         keymap-sources explain-key)

(%name-library '(techne editor))
