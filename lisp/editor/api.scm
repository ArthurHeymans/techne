;;; The editor's interface for extensions, the library (techne editor):
;;;
;;;   (import (techne editor))
;;;
;;; Commands (define-command), keymaps, buffers, major modes (define-mode)
;;; and minor modes with highlight layers (define-minor-mode), options
;;; (define-option, set-option!), hooks (add-hook!), how forms indent
;;; (set-indent!), sessions and what
;;; commands act on. It is the library the application (main.scm) is
;;; built with, not the application: importing it loads none of the
;;; editor's own features or key bindings.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "projects.scm")
(require "views.scm")
(require "listings.scm")
(require "lens.scm")
(require "live.scm")
(require "indent.scm")
(require "requests.scm")
(require "shell.scm")
(require "remote.scm")

(provide define-command register-command! command command-names run-command message!
         define-mode register-mode! define-minor-mode register-minor-mode! find-mode mode-names mode-map mode-on? toggle-mode!
         mode-name mode-doc mode-parent mode-chain derived-mode? mode-for-file
         define-option register-option! set-option! unset-option! option explain-option
         define-hook add-hook! remove-hook! run-hook!
         buffer? buffer-document buffer-name buffer-mode buffer-state document-buffer current-buffer
         make-keymap define-key! name-prefix! lookup-key keymap-sequences kbd emacs-map modal-map minibuffer-map file-name-map
         sget sset! session-view session-document session-panes current-session document=?
         doc ranges point move! check-editable! edit! insert-text! search! region-text replace-region! search-all goto-next!
         set-indent! indent-lines! line-starts indentation lisp-indent
         kill-ring kill-save! yank-text current-prefix
         file-document show-document! show-buffer! visit! buffer-list add-buffer! eval-region!
         completing-read async-source candidate candidate-text candidate-annotation candidate-target take-target candidate-table minibuffer-history
         target target? target-kind target-value target-at define-action actions-for act-on!
         location file-location location? location-document location-position line-candidate
         default-directory file-name show-lens! row excerpt define-view show-view! present! row-at row-target
         error-text make-generated-buffer! buffer-named set-buffer-state! session-buffer display-buffer!
         make-request-slot request! request-pending? cancel-request! append-bounded-output! log-history
         keymap-sources explain-key
         read-file-name read-directory-name normal-directory abbreviate-path absolute-path project-root
         set-buffer-name! set-buffer-mode! drop-buffer! show-in-other-pane! pop-to-buffer! move-path! delete-path-and-buffers!
         session-focus set-session-panes! session-tree set-session-tree!
         make-listing-buffer! listing-redisplay! listing-row listing-mark listing-marked listing-targets
         listing-key-at listing-goto! listing-get listing-set! listing?
         save-document! reload-document! directory-entries path-exists?
         remote-path? location-route location-path location-prefix location-at define-method method-names
         hop? hop-method hop-user hop-host hop-port
         location-endpoint remote-endpoints call-with-process-at)

(%name-library '(techne editor))
