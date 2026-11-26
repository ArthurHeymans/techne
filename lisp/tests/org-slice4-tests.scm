;;; Pure-Scheme tests for Slice 4: org-mem / org-node index, dynamic
;;; #+filetags: todo maintenance, backlinks, and capture builders.
(import (techne test))
(require "../org/date.scm")
(require "../org/syntax.scm")
(require "../org/edit.scm")
(require "../org/mem.scm")
(require "../org/capture.scm")

(define-test "UUID, slugify, aliases, and node filenames"
(let ((id1 (org-new-id))
      (id2 (org-new-id)))
  (check "UUID length 36" 36 (string-length id1))
  (check "UUIDs are distinct" #f (string=? id1 id2))
  (check "UUID v4 marker" #\4 (string-ref id1 14)))

(check "slugify basic" "firmware_boot_notes" (org-slugify "Firmware: Boot Notes!"))
(check "slugify empty" "untitled" (org-slugify "   !!!  "))
(check "parse ROAM_ALIASES"
       '("Coreboot" "Open Source Firmware" "OSF")
       (org-parse-roam-aliases "Coreboot \"Open Source Firmware\" OSF"))

(let* ((day (days-from-civil 2026 11 27))
       (fn (org-node-filename "/tmp/org" "Coreboot Notes" #:now (cons day 570))))
  (check-true "node filename starts with dir/YYYYMMDD0930"
              (string-prefix? "/tmp/org/202611270930" fn))
  (check-true "node filename ends with -coreboot_notes.org"
              (string-suffix? "-coreboot_notes.org" fn))))

(define-test "auto #+filetags: todo maintenance and UTF-8 slicing"
(let* ((doc ":PROPERTIES:\n:ID:       abc-1\n:END:\n#+title: Project X\n\n* TODO Ship firmware\n")
       (ol (org-parse-outline doc))
       (edits (org-edit-update-todo-filetag doc ol))
       (out (org-apply-edits doc edits)))
  (check-true "adds #+filetags: :todo:" (and (string-contains out "#+filetags: :todo:\n") #t))
  (let ((ol2 (org-parse-outline out)))
    (check "second pass is no-op" '() (org-edit-update-todo-filetag out ol2))))

(let* ((doc "#+title: Project X\n#+filetags: :todo:work:\n\n* DONE Shipped\n")
       (ol (org-parse-outline doc))
       (edits (org-edit-update-todo-filetag doc ol))
       (out (org-apply-edits doc edits)))
  (check "removes todo while preserving work" "#+title: Project X\n#+filetags: :work:\n\n* DONE Shipped\n" out))

;; Multi-line #+filetags: removes todo from every directive
(let* ((doc "#+title: Multi\n#+filetags: :work:\n#+filetags: :todo:urgent:\n\n* DONE Done\n")
       (ol (org-parse-outline doc))
       (edits (org-edit-update-todo-filetag doc ol))
       (out (org-apply-edits doc edits)))
  (check "removes todo from second FILETAGS line"
         "#+title: Multi\n#+filetags: :work:\n#+filetags: :urgent:\n\n* DONE Done\n" out))

;; Non-boundary UTF-8 slice does not leak characters past TO
(check "org-slice-bytes non-boundary [1,2) on éxy" "" (org-slice-bytes "éxy" 1 2))
(check "org-slice-bytes [0,2) on éxy" "é" (org-slice-bytes "éxy" 0 2)))

(define-test "org-mem index, backlinks, and live buffer overlays"
(call-with-temporary-directory
 (lambda (tmp)
   (let* ((sub (string-append tmp "/roam/daily"))
          (f1 (string-append tmp "/alpha.org"))
          (f2 (string-append sub "/2026-11-27.org")))
     (make-directory sub)
  (replace-file f1
                (string-append
                 ":PROPERTIES:\n:ID:       node-alpha\n:ROAM_ALIASES: \"Alpha One\" A1\n:END:\n"
                 "#+title: Alpha Topic\n#+filetags: :todo:work:\n\n"
                 "* TODO Subtask\n:PROPERTIES:\n:ID:       node-sub\n:END:\nBody\n"))
  (replace-file f2
                (string-append
                 ":PROPERTIES:\n:ID:       node-daily\n:END:\n"
                 "#+title: 2026-11-27\n\n"
                 "* Notes\nWorked on [[id:node-alpha][Alpha Topic]] and [[id:node-sub][Subtask]].\n"))
  (let ((idx (make-org-mem (list tmp))))
    (org-mem-sync! idx)
    (check "indexed 2 files" 2 (car (org-mem-stats idx)))
    (check "indexed 3 nodes (2 file + 1 heading)" 3 (cdr (org-mem-stats idx)))
    (let ((nd (org-mem-find-node idx "node-alpha")))
      (check-true "found node-alpha" (org-mem-node? nd))
      (check "node-alpha title" "Alpha Topic" (org-mem-node-title nd))
      (check "node-alpha aliases" '("Alpha One" "A1") (org-mem-node-aliases nd)))
    (let ((sub-nd (org-mem-find-node idx "node-sub")))
      (check-true "found heading node-sub" (org-mem-node? sub-nd))
      (check "heading node level 1" 1 (org-mem-node-level sub-nd)))
    (check "backlinks to node-alpha" 1 (length (org-mem-backlinks idx "node-alpha")))
    (check "backlinks to node-sub" 1 (length (org-mem-backlinks idx "node-sub")))
    (check "todo files discovers only alpha.org" (list f1) (org-mem-todo-files idx))
    ;; Links inside src blocks or comments are NOT indexed as backlinks
    (let* ((f-lit (string-append tmp "/lit.org")))
      (replace-file f-lit
                    (string-append
                     "#+title: Literal\n"
                     "# comment [[id:node-alpha][no]]\n"
                     "#+begin_src org\n[[id:node-alpha][no]]\n#+end_src\n"))
      (org-mem-sync! idx)
      (check "src/comment id: links excluded from backlinks"
             1 (length (org-mem-backlinks idx "node-alpha"))))
    ;; Live buffer overlay overrides disk without touching disk
    (let* ((live-txt ":PROPERTIES:\n:ID:       node-alpha\n:END:\n#+title: Alpha Live\n")
           (live-ol (org-parse-outline live-txt)))
      (org-mem-sync! idx #:live-buffers (list (cons f1 (cons live-txt live-ol))))
      (check "live buffer updates node title" "Alpha Live" (org-mem-node-title (org-mem-find-node idx "node-alpha")))
      (check "live buffer removes todo filetag from todo-files" '() (org-mem-todo-files idx))))))))

(define-test "daily and meeting capture builders"
(let* ((day (days-from-civil 2026 11 27))
       (dp (org-capture-daily-path "/tmp/daily" day))
       (mp (org-capture-meeting-path "/tmp/daily" day "Microsoft Daily"))
       (me (org-capture-meeting-entry "Microsoft Daily" "Arthur Heymans" (cons day 600))))
  (check "daily path" "/tmp/daily/2026-11-27.org" dp)
  (check "meeting path" "/tmp/daily/2026-11-27-meeting-microsoft_daily.org" mp)
  (check-true "meeting has Date inactive timestamp" (and (string-contains me ":Date: [2026-11-27 Fri 10:00]") #t))
  (check-true "meeting has Participants" (and (string-contains me ":Participants: Arthur Heymans") #t))
  (check-true "meeting has Action Items checkbox" (and (string-contains me "** Action Items\n- [ ]") #t))))
