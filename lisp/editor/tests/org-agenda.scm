;;; Headless editor tests for interactive org-agenda-mode lens, habit logging,
;;; TODO cycling from agenda, and "Why here / Why missing?" explanations.
(import (techne test))
(test-file #:environment 'editor)
(require "../main.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../modes.scm")
(require "../api.scm")
(require "../builtins.scm")
(require "../../org/date.scm")

(define (editing filename text)
  (let ((s (start-session (make-view (make-document text) "user") "emacs"))
        (m (mode-for-file filename)))
    (when (and (eq? m 'org-mode) (not (find-package 'org-runtime)))
      (activate-command-package! 'org-runtime))
    (set-buffer-mode! (current-buffer s) m)
    s))

(define (text s) (document-string (session-document s)))

(define-test "interactive agenda lens, habit logging, explain, and stale refusal"
(let* ((today-str (date->string (car (local-now))))
       (sample (string-append
                "#+title: Workday\n"
                "* Pray :PRAYER:\n"
                ":PROPERTIES:\n"
                ":HABIT: 2/week\n"
                ":END:\n"
                "* TODO Ship agenda :WORK:\n"
                "  SCHEDULED: <" today-str " Thu>\n"
                ":PROPERTIES:\n"
                ":EFFORT: 1:30\n"
                ":END:\n"
                "* HOLD Ignored task :WORK:\n"
                "  SCHEDULED: <" today-str " Thu 11:00-12:00>\n"))
       (s (editing "work.org" sample))
       (src-doc (session-document s)))
  ;; Open *Org Agenda* via C-c a
  (press-keys s "C-c a")
  (check "opened *Org Agenda*" "*Org Agenda*" (buffer-name (current-buffer s)))
  (let ((ag-text (text s)))
    (check-true "agenda shows Habit: ora group" (and (string-contains ag-text "Habit: ora") #t))
    (check-true "agenda shows Pray habit" (and (string-contains ag-text "Pray") #t))
    (check-true "agenda shows Today: labora group" (and (string-contains ag-text "Today: labora") #t))
    (check-true "agenda shows effort budget" (and (string-contains ag-text "Planned 1:30") #t))
    (check "HOLD task discarded from agenda" #f (string-contains ag-text "Ignored task")))
  ;; Move caret onto the Pray row and press 'l' to log a habit completion!
  (let* ((txt (text s))
         (ci (string-contains txt "Pray"))
         (pray-pos (bytevector-length (string->utf8 (substring txt 0 ci)))))
    (view-set-ranges! (session-view s) (list (list pray-pos pray-pos)) 0)
    (press-keys s "l")
    (check "agenda 'l' echo" "Logged habit completion" (echo-line s))
    (check-true "agenda 'l' appended :LOGBOOK: Habit line to source buffer"
                (and (string-contains (document-string src-doc) "- Habit: 1 [") #t))
    ;; Press '?' to explain why Pray is in the agenda
    (press-keys s "?")
    (check "explain opened *Org Explain*" "*Org Explain*" (buffer-name (current-buffer s)))
    (check-true "explanation names Habit: ora" (and (string-contains (text s) "Claimed by group: Habit: ora") #t)))
  ;; Return to work.org on HOLD heading and press C-c ? to explain why it's missing!
  (show-buffer! s (document-buffer src-doc))
  (let ((hold-pos (string-contains (document-string src-doc) "* HOLD")))
    (view-set-ranges! (session-view s) (list (list hold-pos hold-pos)) 0)
    (press-keys s "C-c ?")
    (check-true "explain reports HOLD as discarded" (and (string-contains (text s) "discarded") #t)))
  ;; Stale agenda row refusal: insert a heading above in src-doc without refreshing agenda,
  ;; then pressing 'l' on the stale row must refuse instead of mutating the wrong heading.
  (let ((v-src (make-view src-doc "user")))
    (view-edit-at! v-src (document-revision src-doc) '((0 0 "* Inserted top\n")) "new")
    (let ((after-insert (document-string src-doc)))
      (show-buffer! s (buffer-named "*Org Agenda*"))
      (let* ((txt (text s))
             (ci (string-contains txt "Pray"))
             (pray-pos (bytevector-length (string->utf8 (substring txt 0 ci)))))
        (view-set-ranges! (session-view s) (list (list pray-pos pray-pos)) 0)
        (press-keys s "l")
        (check "stale agenda row refuses edit" "Agenda item is stale; press g to refresh" (echo-line s))
        (check "stale refusal leaves source untouched" after-insert (document-string src-doc)))))))
