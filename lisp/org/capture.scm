;;; Org capture templates and pure byte-splice helpers for dailies,
;;; meetings, new ID-nodes, and capturing into existing nodes.

(require "date.scm")
(require "syntax.scm")
(require "mem.scm")

(define (org-capture-daily-path daily-dir day)
  "Return the daily note file path in DAILY-DIR for civil day DAY (`YYYY-MM-DD.org`)."
  (path-join-clean daily-dir (string-append (date->string day) ".org")))

(define (org-capture-meeting-path daily-dir day title)
  "Return the meeting note file path in DAILY-DIR for DAY and meeting TITLE."
  (path-join-clean daily-dir
                   (string-append (date->string day) "-meeting-" (org-slugify title) ".org")))

(define (org-capture-daily-header day #:id [id #f])
  "Return initial file content for a new daily/meeting file on DAY with optional ID."
  (org-new-node-content (date->string day) #:id id))

(define (format-inactive-now now-pair)
  (let* ((day (car now-pair))
         (mins (cdr now-pair))
         (hh (quotient mins 60))
         (mm (modulo mins 60)))
    (string-append "[" (date->string day) " " (weekday-short-name day) " "
                   (pad2 hh) ":" (pad2 mm) "]")))

(define (org-capture-meeting-entry title participants now-pair)
  "Return formatted Org meeting entry text for TITLE, PARTICIPANTS, and NOW-PAIR."
  (string-append
   "* Meeting: " title "\n"
   ":PROPERTIES:\n"
   ":Date: " (format-inactive-now now-pair) "\n"
   ":Participants: " participants "\n"
   ":END:\n"
   "** Agenda\n-\n\n"
   "** Notes\n- \n\n"
   "** Action Items\n- [ ]\n"))

(define (org-capture-append-edits text content #:heading [h #f])
  "Return byte-splice edits inserting CONTENT at EOF of TEXT or end of heading H."
  (let* ((pos (if h (org-heading-subtree-end h) (org-utf8-length text)))
         (needs-nl? (and (> pos 0)
                         (not (string-suffix? "\n" (org-slice-bytes text (max 0 (- pos 1)) pos)))))
         (payload (string-append (if needs-nl? "\n" "") content
                                 (if (string-suffix? "\n" content) "" "\n"))))
    (list (list pos pos payload))))
