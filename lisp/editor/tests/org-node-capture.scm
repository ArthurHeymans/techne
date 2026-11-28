;;; Headless editor tests for Slice 4: org-node-find, org-node-insert-link,
;;; id: link following, org-node-backlinks, org-capture (daily & meeting),
;;; auto #+filetags: todo on save, and dynamic org-agenda discovery.
(import (techne test))
(test-file #:environment 'editor)
(require "../main.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../modes.scm")
(require "../api.scm")
(require "../builtins.scm")
(require "../../org/date.scm")

(define-test "org-node, backlinks, dynamic agenda, capture, and global keys"
(call-with-temporary-directory
 (lambda (tmp)
   (let* ((daily-dir (string-append tmp "/roam/daily"))
          (today-str (date->string (car (local-now))))
          (f-target (string-append tmp "/target.org"))
          (f-work (string-append tmp "/work.org")))
     (make-directory daily-dir)
  (replace-file f-target
                (string-append
                 ":PROPERTIES:\n:ID:       id-target-1\n:END:\n"
                 "#+title: Firmware Notes\n\n"
                 "* Overview\nDetails here.\n"))
  (replace-file f-work
                (string-append
                 ":PROPERTIES:\n:ID:       id-work-1\n:END:\n"
                 "#+title: Work Log\n#+filetags: :todo:\n\n"
                 "* TODO Check [[id:id-target-1][Firmware Notes]] :WORK:\n"
                 "  SCHEDULED: <" today-str " Thu>\n"))
  (let* ((d (file-document f-work))
         (s (start-session (make-view d "user") "emacs")))
    (set-option! 'org-directory tmp)
    (set-option! 'org-daily-directory daily-dir)
    (set-option! 'org-mem-watch-dirs (list tmp))
    (set-option! 'org-default-agenda-files '())
    (set-option! 'org-agenda-files '())
    ;; 1. Follow [[id:id-target-1][Firmware Notes]] via C-c C-o
    (let ((lpos (string-contains (document-string d) "[[id:id-target-1")))
      (view-set-ranges! (session-view s) (list (list lpos lpos)) 0)
      (press-keys s "C-c C-o")
      (check "id: link jumped to target.org" f-target (document-path (session-document s))))
    ;; 2. Open backlinks buffer via C-c n b from target.org
    (press-keys s "C-c n b")
    (check "opened *Org Backlinks*" "*Org Backlinks*" (buffer-name (current-buffer s)))
    (check-true "backlinks lists citing heading Check (not just header)"
                (and (string-contains (document-string (session-document s)) "Check") #t))
    ;; 3. Dynamic org-agenda discovers an UNOPENED #+filetags: todo file on disk and hides :todo:
    (let ((f-unopened (string-append tmp "/unopened.org")))
      (replace-file f-unopened
                    (string-append
                     "#+title: Unopened\n#+filetags: :todo:work:\n\n"
                     "* TODO Unopened disk task :WORK:\n"
                     "  SCHEDULED: <" today-str " Thu>\n"))
      (show-document! s (file-document f-target))
      (press-keys s "C-c a")
      (let ((ag (document-string (session-document s))))
        (check-true "dynamic agenda includes unopened todo-tagged disk file"
                    (and (string-contains ag "Unopened disk task") #t))
        (check "agenda hides :todo: tag" #f (string-contains ag ":todo:"))))
    ;; 4. Auto #+filetags: todo on save when adding a TODO to target.org
    (let ((dt (file-document f-target)))
      (show-document! s dt)
      (let ((vt (session-view s))
            (len (document-length dt)))
        (view-edit-at! vt (document-revision dt) (list (list len len "\n* TODO New task\n")) "new")
        (save-document! dt)
        (check-true "save auto-added #+filetags: :todo:"
                    (and (string-contains (document-string dt) "#+filetags: :todo:") #t))))
    ;; 5. Capture daily note via C-c c -> d -> heading -> editable *Org Capture* draft -> C-c C-c
    (press-keys s "C-c c")
    (type-text s "d")
    (press-keys s "RET")
    (type-text s "Daily standup notes")
    (press-keys s "RET")
    (check "capture opened editable *Org Capture* draft" "*Org Capture*" (buffer-name (current-buffer s)))
    (type-text s "Extra body line\n")
    (press-keys s "C-c C-c")
    (let* ((daily-path (string-append tmp "/roam/daily/" today-str ".org"))
           (daily-doc (file-document daily-path))
           (txt (document-string daily-doc)))
      (check-true "daily file has :ID:, captured heading, and edited body"
                  (and (string-contains txt ":ID:")
                       (string-contains txt "* Daily standup notes")
                       (string-contains txt "Extra body line")
                       #t)))
    ;; 5b. Capture cancel via C-c C-k discards without touching target
    (press-keys s "C-c c")
    (type-text s "t")
    (press-keys s "RET")
    (type-text s "Should not be saved")
    (press-keys s "RET")
    (check "cancel test opened *Org Capture*" "*Org Capture*" (buffer-name (current-buffer s)))
    (press-keys s "C-c C-k")
    (let* ((daily-path (string-append tmp "/roam/daily/" today-str ".org"))
           (txt (document-string (file-document daily-path))))
      (check "cancelled capture did not write to daily file" #f (string-contains txt "Should not be saved")))
    ;; 6. Capture meeting note via C-c c -> m -> title -> participants -> C-c C-c
    (press-keys s "C-c c")
    (type-text s "m")
    (press-keys s "RET")
    (type-text s "Coreboot Sync")
    (press-keys s "RET")
    (type-text s "Arthur Heymans")
    (press-keys s "RET")
    (check "meeting capture opened *Org Capture*" "*Org Capture*" (buffer-name (current-buffer s)))
    (press-keys s "C-c C-c")
    (let* ((mpath (string-append tmp "/roam/daily/" today-str "-meeting-coreboot_sync.org"))
           (txt (document-string (file-document mpath))))
      (check-true "meeting file has Meeting heading, Participants, and Action Items"
                  (and (string-contains txt "* Meeting: Coreboot Sync")
                       (string-contains txt ":Participants: Arthur Heymans")
                       (string-contains txt "** Action Items")
                       #t)))
    ;; 7. Global modal bindings work from a non-org buffer (SPC o A, SPC N g, H in agenda)
    (let* ((plain (make-document "plain text\n"))
           (sm (start-session (make-view plain "user") "modal")))
      (press-keys sm "SPC o A")
      (check "global SPC o A opens *Org Agenda* from text buffer" "*Org Agenda*" (buffer-name (current-buffer sm)))
      ;; Move onto Check row and press H to explain/inspect
      (let* ((txt (document-string (session-document sm)))
             (ci (string-contains txt "Check"))
             (pos (bytevector-length (string->utf8 (substring txt 0 ci)))))
        (view-set-ranges! (session-view sm) (list (list pos pos)) 0)
        (press-keys sm "H")
        (check "H in agenda opens *Org Explain*" "*Org Explain*" (buffer-name (current-buffer sm))))
      ;; Global SPC N g greps org-mem nodes
      (press-keys sm "SPC N g")
      (type-text sm "Details")
      (press-keys sm "RET")
      (check "global SPC N g opens *Org Grep*" "*Org Grep*" (buffer-name (current-buffer sm)))
      (check-true "org-node-grep found Details in Firmware Notes"
                  (and (string-contains (document-string (session-document sm)) "Firmware Notes") #t))))))))
