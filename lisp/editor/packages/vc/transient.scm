;;; Transient menus, as Magit's: a prefix key shows, below the panes,
;;; the commands that can follow it in groups, and the switches and
;;; options that change what they do. One key runs a command with the
;;; arguments set; C-g or q leaves. The focused pane keeps the focus, so
;;; a command acts on what point is on, as it would without the menu.
;;;
;;; A menu is data: (title group ...), a group (heading entry ...), an
;;; entry one of
;;;
;;;   (suffix KEY DESCRIPTION ACTION [ENABLED?])  ACTION a command name or
;;;       (lambda (session args)); ENABLED? a predicate of the session;
;;;   (switch KEY DESCRIPTION ARGUMENT [ON?])     a flag such as "--all";
;;;   (option KEY DESCRIPTION ARGUMENT [VALUE])   ARGUMENT ends in "=", its
;;;       value is read in the minibuffer;
;;;   (choice KEY DESCRIPTION (ARGUMENT ...) [INDEX]) one of exclusive flags.
;;;
;;; KEY is typed key by key: "-a" is - then a. A suffix's command reads
;;; the arguments set with (vc-transient-args s).

(import (techne editor))

(provide vc-transient! vc-transient-args vc-transient-arg? vc-transient-value vc-transient-open?
         vc-read-char-choice!)

(define vc-transient! transient!)
(define vc-transient-args transient-args)
(define vc-transient-arg? transient-arg?)
(define vc-transient-value transient-value)
(define vc-transient-open? transient-open?)
(define vc-read-char-choice! read-char-choice!)
