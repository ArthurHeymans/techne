;;; A native Scheme mail application. IMAP and Maildir share one local cache,
;;; one rich reader, and one composer. No source mail is deleted or changed.
(import (techne editor) (techne mail store) (techne mail mime) (techne mail html) (techne mail address)
        (techne mail imap) (techne mail maildir) (techne mail smtp) (techne mail thread) (techne mail date)
        (techne time) (techne uuid) (techne path) (techne task))

(define-option mail-accounts '()
  "Mail accounts, as a list of alists, configured in Scheme.
An IMAP account has id, kind (imap), host, port, user and mailboxes.
A Maildir account has id, kind (maildir) and path, a tree of folders as
mbsync keeps one; folders lists the only ones to read, skip the ones not
to, and mailbox makes the path one folder of that name.
Sending uses from, smtp-host, smtp-port and smtp-user. The imap-security
and smtp-security settings default to implicit-tls (IMAP 993, SMTP 465).
Set either to starttls for mandatory STARTTLS (IMAP 143, SMTP 587).
Passwords are never stored here: `mail-credential-provider` supplies them."
  #:type 'any)
(define-option mail-credential-provider #f
  "Procedure (account purpose) returning a secret for imap or smtp.
Use a secret manager in this procedure. Secrets are not persisted or logged.
No password is read in the visible minibuffer."
  #:type 'any)
(define-option mail-database #f
  "Local mail SQLite file, or #f for the XDG data directory.
The default is XDG_DATA_HOME/techne/mail/cache.sqlite. Set this before
opening mail in a new runtime. This file holds private messages and drafts."
  #:type 'any)
(define-option mail-prefer-html #t
  "Whether messages with HTML use the private rich reader by default.
Scripts, styles, embedded content and remote images are never loaded."
  #:type 'boolean)

(define-option mail-reader-position 'bottom
  "Position of the mail reader: bottom (the default) or side.
Below the list, rows have the frame's width for subjects. This is a
global workspace preference. Short bottom layouts and narrow side layouts
use one pane; f maximizes/restores a pane. To switch an open workspace
use mail-toggle-reader-position, or set this option in your mail
configuration."
  #:type '(one-of bottom side))

(define-face mail.title "The mail workspace title." #:fg 'blue #:weight 'bold #:height 1.35)
(define-face mail.section "Mail navigation and section labels." #:fg 'subtext #:weight 'bold)
(define-face mail.unread "Unread mail subjects." #:fg 'text #:weight 'bold)
(define-face mail.subject "Read mail subjects." #:fg 'subtext)
(define-face mail.preview "Mail sender and preview text." #:fg 'overlay)
(define-face mail.selected "The selected mail summary." #:bg 'surface0 #:extend #t)
(define-face mail.star "The star of a starred conversation." #:fg 'yellow)
(define-face mail.status "Local mail state and private-view notices." #:fg 'teal)
(define-face mail.count "Message counts on the mail dashboard." #:fg 'overlay)

(define %db #f)
(define %sync-task #f)
(define %deliveries 0)
(define %sync-imported 0)
(define %sync-removed 0)
(define %sync-issues '())
;; What a sync left out on purpose, as (ACCOUNT MAILBOX COUNT): messages
;; over the 16 MiB a message may have. Not a failure: reported, not alarming.
(define %sync-large '())
(define %idle-status "Local cache · no source changes")
(define %sync-status %idle-status)
;; What the last sync, or the one running, did for each account, by id.
(define %account-status (make-hash-table))
;; Nothing to report: no sync has run.
(define (sync-quiet?) (string=? %sync-status %idle-status))
;; The listed messages the query no longer finds, as last read from the
;; cache: read once per change, not once per key.
(define %kept (make-hash-table))
(define %last-change #f)
(define %last-before '())

(define (take-while pred xs)
  (let loop ((xs xs) (out '()))
    (if (and (pair? xs) (pred (car xs))) (loop (cdr xs) (cons (car xs) out)) (reverse out))))
(define (mail-append-map proc . lists) (apply append (apply map proc lists)))

(define (get st key . default) (hash-table-ref/default st key (if (pair? default) (car default) #f)))
(define (put! st key value) (hash-table-set! st key value))
(define (safe-text text)
  ;; Received text must not inject terminal escapes or directional overrides.
  (list->string
   (map (lambda (c)
          (let ((n (char->integer c)))
            (cond ((memv c '(#\newline #\tab)) c)
                  ((< n 32) (integer->char (+ 9216 n)))
                  ((= n 127) #\x2421)
                  ((or (<= 8234 n 8238) (<= 8294 n 8297)) #\xFFFD)
                  (else c)))) (string->list text))))
(define (one-line text)
  (list->string (map (lambda (c) (if (memv c '(#\newline #\tab #\return)) #\space c))
                    (string->list (safe-text text)))))
(define (short text n)
  (let ((text (one-line text))) (if (> (string-length text) n) (string-append (substring text 0 (- n 1)) "…") text)))
(define (ensure-directory! path)
  (unless (file-exists? path)
    (let ((parent (path-directory path)))
      (unless (or (string=? parent path) (string=? parent "")) (ensure-directory! parent))
      (create-directory path #o700))))
(define (database!)
  (or %db
      (let* ((path (or (option #f 'mail-database)
                       (string-append (or (get-environment-variable "XDG_DATA_HOME")
                                          (string-append (or (get-environment-variable "HOME") ".") "/.local/share"))
                                      "/techne/mail/cache.sqlite"))))
        (unless (string=? path ":memory:") (ensure-directory! (path-directory path)))
        (set! %db (mail-store-open path)) %db)))
(define (mail-state s) (or (sget s 'mail) (let ((st (make-hash-table))) (put! st 'group 'unread) (put! st 'query "") (sset! s 'mail st) st)))
(define (configured-accounts)
  (let ((accounts (option #f 'mail-accounts))) (if (list? accounts) accounts '())))
(define (account-by-id id) (find (lambda (a) (equal? (mail-ref a 'id) id)) (configured-accounts)))
(define (secret account purpose)
  (let ((provider (option #f 'mail-credential-provider)))
    (unless (procedure? provider) (error "Set mail-credential-provider in your Scheme configuration"))
    (let ((value (provider account purpose)))
      (unless (string? value) (error "The mail credential provider returned no secret")) value)))
(define (selected-message s)
  (let* ((st (mail-state s))
         (id (if (eq? (buffer-mode (current-buffer s)) 'mail-summary-mode)
                 (get st 'selected) (or (get st 'reader-id) (get st 'selected)))))
    (and id (mail-store-message (database!) id))))
(define (row-id s)
  (let ((t (target-at (doc s) (point s))))
    (and t (eq? (target-kind t) 'mail-message) (target-value t))))
(define (current-id s)
  (or (row-id s)
      (let ((st (mail-state s)))
        (if (eq? (buffer-mode (current-buffer s)) 'mail-summary-mode) (get st 'selected)
            (or (get st 'reader-id) (get st 'selected))))
      (error "Select a message first")))
(define (narrow? s) (< (or (sget s 'viewport-columns) 160) 110))
(define (compact-date date)
  (mail-date-label date #:zone (guard (e ((not (task-cancelled-condition? e)) "UTC")) (local-zone))))
(define (group-label group)
  (case group ((all) "All mail") ((unread) "Unread") ((starred) "Starred") (else "Inbox")))

;;; A view is what a list shows: (GROUP ACCOUNT MAILBOX). A group across
;;; accounts has neither; an account's Inbox is (inbox ID #f), and one of
;;; its folders, all that is in it, (all ID MAILBOX).
(define (view-of st) (list (get st 'group 'inbox) (get st 'account) (get st 'mailbox)))
(define (view-label view)
  (cond ((caddr view) (string-append (cadr view) " · " (caddr view)))
        ((and (cadr view) (eq? (car view) 'inbox)) (string-append (cadr view) " · Inbox"))
        ((cadr view) (cadr view))
        (else (group-label (car view)))))
(define (view-query view query #:limit [limit 500])
  (mail-store-query (database!) query #:group (car view) #:account (cadr view) #:mailbox (caddr view) #:limit limit))
(define (view-count view query)
  (mail-store-count (database!) query #:group (car view) #:account (cadr view) #:mailbox (caddr view)))
(define (folder-rank box)
  ;; Inbox first, then ordinary folders, then Gmail's labels, All Mail
  ;; last: the order folders are listed and fetched in.
  (let ((b (string-downcase box)))
    (cond ((string=? b "inbox") 0) ((string-suffix? "/all mail" b) 3)
          ((string-prefix? "[" b) 2) (else 1))))
(define (by-rank items name)
  ;; ITEMS in `folder-rank` order of their NAME, keeping their order within one.
  (append-map (lambda (rank) (filter (lambda (item) (= (folder-rank (name item)) rank)) items)) '(0 1 2 3)))
(define (grouped n)
  ;; 64234 as "64,234".
  (let loop ((digits (number->string n)) (out ""))
    (let ((k (string-length digits)))
      (if (<= k 3) (string-append digits out)
          (loop (substring digits 0 (- k 3)) (string-append "," (substring digits (- k 3) k) out))))))
(define (clock-label ms)
  (let* ((c (instant->civil ms (guard (e ((not (task-cancelled-condition? e)) "UTC")) (local-zone))))
         (two (lambda (n) (if (< n 10) (string-append "0" (number->string n)) (number->string n)))))
    (string-append (two (list-ref c 3)) ":" (two (list-ref c 4)))))
(define (mail-link action label . args) (list 'link (target 'mail-control (cons action args)) label))
(define (sender-name m)
  ;; The display name, or the address when there is none.
  (let* ((from (one-line (mail-ref m 'sender ""))) (at (string-index from #\<))
         (name (string-trim (if (and at (> at 0)) (substring from 0 at) from))))
    (if (string=? name "") from name)))
(define (long-date date)
  ;; "Thu 5 Nov 2026, 14:32" in the local zone; the header as sent if it does not parse.
  (let ((instant (mail-date->instant date)))
    (if (not instant) (one-line date)
        (let* ((zone (guard (e ((not (task-cancelled-condition? e)) "UTC")) (local-zone)))
               (c (instant->civil instant zone))
               (two (lambda (n) (if (< n 10) (string-append "0" (number->string n)) (number->string n)))))
          (string-append (list-ref '("Sun" "Mon" "Tue" "Wed" "Thu" "Fri" "Sat") (weekday (instant->days instant zone)))
                         " " (number->string (caddr c)) " "
                         (list-ref '("Jan" "Feb" "Mar" "Apr" "May" "Jun" "Jul" "Aug" "Sep" "Oct" "Nov" "Dec") (- (cadr c) 1))
                         " " (number->string (car c)) ", " (two (list-ref c 3)) ":" (two (list-ref c 4)))))))


;; Fixed-column widths keep subjects on one visual row and columns aligned.
(define (char-columns c)
  ;; Two for East Asian wide and fullwidth characters and emoji, else one:
  ;; an ellipsis, a dash or a curly quote is one column, not two.
  (let ((n (char->integer c)))
    (if (or (<= #x1100 n #x115F) (<= #x2E80 n #xA4CF) (<= #xAC00 n #xD7A3) (<= #xF900 n #xFAFF)
            (<= #xFE30 n #xFE4F) (<= #xFF00 n #xFF60) (<= #xFFE0 n #xFFE6)
            (<= #x1F300 n #x1FAFF) (<= #x20000 n #x3FFFD))
        2 1)))
(define (mail-columns text)
  (fold (lambda (c n) (+ n (char-columns c))) 0 (string->list text)))
(define (mail-column text width)
  ;; TEXT on one line in WIDTH columns at most, ending in an ellipsis if cut.
  (let ((text (one-line text)))
    (if (<= (mail-columns text) width) text
        (let loop ((chars (string->list text)) (used 0) (out '()))
          (if (or (null? chars) (> (+ used (char-columns (car chars))) (- width 1)))
              (string-append (list->string (reverse out)) "…")
              (loop (cdr chars) (+ used (char-columns (car chars))) (cons (car chars) out)))))))
(define (mail-pad text width)
  (let ((text (mail-column text width))) (string-append text (make-string (max 0 (- width (mail-columns text))) #\space))))

;;; A keyed list above or beside the rich reader: one row per message, a
;;; reply drawn under what it answers, as a Gnus or mu4e summary does.
(define (summary-rows b)
  (let* ((st (listing-get b 'mail-state)) (messages (get st 'messages '()))
         (group (get st 'group 'inbox)) (query (get st 'query ""))
         ;; Counted when the list is made, not each time a row is selected.
         (total (get st 'view-total (length messages))) (unread (get st 'view-unread 0))
         (cached? (get st 'cached? #t))
         (columns (or (get st 'columns) 160))
         (width (max 16 (- (if (get st 'full-width?) columns (exact (floor (* columns 0.38)))) 6)))
         (wide? (>= width 40))
         (sender-width (min 20 (max 10 (quotient width 4))))
         (date-width 10)
         ;; Mark, dot, star and the gaps between columns take five more.
         (subject-width (if wide? (max 8 (- width sender-width date-width 5)) (- width 4))))
    (append
     (list (row #:key "group" #:target (target 'mail-control '(groups))
                (list (list (short (string-append
                                    (view-label (view-of st)) " · "
                                    (if (> total (length messages))
                                        (string-append (grouped (length messages)) " of " (grouped total) " messages")
                                        (string-append (grouped total) (if (= total 1) " message" " messages")))
                                    (if (or (eq? group 'unread) (= unread 0)) "" (string-append " · " (grouped unread) " unread"))
                                    (if (string=? query "") "" (string-append " · search: " (short query 24)))) width) 'mail.section)))
           (row #:key "header-gap" ""))
     (if (null? messages)
         (let ((caught-up? (and (eq? group 'unread) (string=? query "") cached?)))
           (list (row #:key "empty-2" (list (list (cond ((not (string=? query "")) "No messages match this search.")
                                                         (caught-up? "You're caught up.")
                                                         (else "No mail here yet.")) 'mail.unread)))
                 (row #:key "empty-3" (cond ((not (string=? query "")) "Press / to try another search; C-c C-l clears it.")
                                            (caught-up? "Nothing is unread. q goes back to the dashboard.")
                                            (cached? "q goes back to the dashboard of views and folders.")
                                            (else "Configure mail-accounts, then g fetches mail.")))
                  (row #:key "empty-action" #:target (target 'mail-control (if caught-up? '(all) '(dashboard)))
                       (list (list (if caught-up? "Browse all mail →" "Mail dashboard →") 'link)))))
         (map
          (lambda (m)
            (let* ((id (mail-ref m 'id)) (key (number->string id))
                   (unread? (= (mail-ref m 'unread) 1))
                   ;; Selection is the row's background (`summary-layer`): the
                   ;; text keeps saying whether the message is unread.
                   (face (if unread? 'mail.unread 'mail.subject))
                   (tree (mail-ref m 'tree ""))
                   ;; A reply repeating its parent's subject shows only its place.
                   (subject (if (mail-ref m 'tree-subject? #t)
                                (mail-column (mail-ref m 'subject) (max 4 (- subject-width (mail-columns tree))))
                                "")))
              (listing-row b key (target 'mail-message id)
                (append (list (list (if unread? "●" " ") 'mail.status)
                              (list (if (= (mail-ref m 'starred) 1) "★" " ") 'mail.star) " ")
                        (if wide?
                            (list (list (mail-pad (compact-date (mail-ref m 'date)) date-width) 'mail.preview) " "
                                  (list (mail-pad (sender-name m) sender-width) (if unread? 'mail.unread 'mail.preview)) " ")
                            '())
                        (if (string=? tree "") '() (list (list tree 'mail.preview)))
                        (list (list subject face))))))
          messages))
     ;; Sync status takes rows only while it has something to say.
     (if (sync-quiet?) '()
         (list (row #:key "status-gap" "")
               (row #:key "status" #:target (target 'mail-control '(sync-status))
                    (list (list (short %sync-status width) 'comment))))))))

(define (summary-layer d from to)
  (let* ((b (document-buffer d)) (st (and b (listing-get b 'mail-state))) (id (and st (get st 'selected))))
    (if id
        (filter-map (lambda (key) (let ((r (presentation-row-span d key)))
                                   (and r (list (car r) (min (document-length d) (+ 1 (cadr r))) 'mail.selected))))
                    (list (number->string id)))
        '())))

(define-mode mail-summary-mode
  "The mail list: arrows or n/p select a row, RET reads its message.
A message read stays on its row, no longer bold. Mark messages as in
mu4e, then x does it: r archive, d delete, ! read, ? unread, + star,
- unstar; u unmarks, U unmarks all, m marks several for one of those.
a undoes, until mbsync runs. c marks all of the group read. / searches,
G selects a group, g fetches mail and makes the list again, C composes,
R replies. A Maildir message's flags and folder are changed at the
source, as mu4e changes them."
  #:parent 'listing-mode
  #:layer summary-layer
  #:options '((line-numbers . #f) (eob-marker . "") (cursor-visible . #f))
  #:keys '(("n" mail-next) ("p" mail-previous) ("j" mail-next) ("k" mail-previous)
           ("C-n" mail-next) ("C-p" mail-previous) ("<down>" mail-next) ("<up>" mail-previous) ("RET" mail-read) ("/" mail-search) ("G" mail-groups)
           ("g" mail-sync) ("e" mail-sync-status) ("c" mail-catch-up) ("C" mail-compose) ("D" mail-drafts) ("f" mail-toggle-layout) ("R" mail-reply)
           ("r" mail-mark-archive) ("!" mail-mark-read) ("?" mail-mark-unread) ("+" mail-mark-star) ("-" mail-mark-unstar)
           ("x" mail-execute) ("d" mail-mark-delete) ("S" mail-star) ("A" mail-archive) ("%" mail-mark-pattern) ("T" mail-mark-thread)
           ("a" mail-undo) ("q" mail-dashboard) ("C-c C-l" mail-clear-search) ("C-c C-k" mail-cancel-sync))
  #:normal '(("j" mail-next) ("k" mail-previous) ("n" mail-next) ("p" mail-previous)
             ("C-n" mail-next) ("C-p" mail-previous) ("<down>" mail-next) ("<up>" mail-previous) ("RET" mail-read)
             ("/" mail-search) ("G" mail-groups) ("g r" mail-sync) ("c" mail-catch-up) ("C" mail-compose) ("R" mail-reply)
             ("r" mail-mark-archive) ("!" mail-mark-read) ("?" mail-mark-unread) ("+" mail-mark-star) ("-" mail-mark-unstar)
             ("x" mail-execute) ("d" mail-mark-delete) ("S" mail-star) ("A" mail-archive) ("%" mail-mark-pattern) ("T" mail-mark-thread)
             ("a" mail-undo) ("q" mail-dashboard) ("f" mail-toggle-layout) ("D" mail-drafts)))

(define-mode mail-message-mode
  "Private rich email reader. n/p read adjacent messages; R replies.
The marks of the list work here on the open message, and go on to the
next: r archive, ! read, ? unread, + star, - unstar, u unmark, x does
them. v toggles HTML/plain, TAB folds quoted text, H shows details, s
shows source, b returns to the list and q closes the reader, as mu4e's
q does. Remote content never loads. r archives and d deletes, marked;
x does it, at the source, as in the list."
  #:parent 'rich-mode
  #:options '((read-only . #t) (line-numbers . #f) (eob-marker . ""))
  #:keys '(("n" mail-next) ("p" mail-previous) ("R" mail-reply) ("c" mail-catch-up) ("C" mail-compose) ("v" mail-toggle-body)
           ("TAB" mail-toggle-quotes) ("H" mail-toggle-details) ("s" mail-source) ("b" mail-back) ("q" mail-close-reader)
           ("r" mail-mark-archive) ("!" mail-mark-read) ("?" mail-mark-unread) ("+" mail-mark-star) ("-" mail-mark-unstar)
           ("u" mail-unmark) ("x" mail-execute) ("d" mail-mark-delete) ("S" mail-star) ("A" mail-archive) ("a" mail-undo)
           ("/" mail-search) ("G" mail-groups) ("e" mail-sync-status) ("f" mail-toggle-layout) ("D" mail-drafts) ("SPC" scroll-up-command))
  #:normal '(("n" mail-next) ("p" mail-previous) ("R" mail-reply) ("v" mail-toggle-body)
             ("TAB" mail-toggle-quotes) ("H" mail-toggle-details) ("s" mail-source) ("b" mail-back) ("q" mail-close-reader)
             ("r" mail-mark-archive) ("!" mail-mark-read) ("?" mail-mark-unread) ("+" mail-mark-star) ("-" mail-mark-unstar)
             ("u" mail-unmark) ("x" mail-execute) ("d" mail-mark-delete) ("S" mail-star) ("A" mail-archive) ("a" mail-undo)
             ("f" mail-toggle-layout) ("D" mail-drafts)))

(define (plain-rich text quotes?)
  ;; Literal plain text, not Markdown. Consecutive quoted lines fold as a unit.
  (let loop ((lines (string-split (safe-text (string-join (string-split text "\r\n") "\n")) "\n")) (blocks '()))
    (cond ((null? lines) (reverse blocks))
          ((string=? (string-trim (car lines)) "") (loop (cdr lines) blocks))
          ((string-prefix? ">" (car lines))
           (let* ((quoted (take-while (lambda (l) (or (string-prefix? ">" l) (string=? l ""))) lines))
                  (body (map (lambda (l) (if (string-prefix? ">" l) (string-trim-left (substring l 1 (string-length l))) l)) quoted)))
             (loop (drop lines (length quoted))
                   (cons (if quotes? (cons 'quote (plain-rich (string-join body "\n") #t))
                             (list 'paragraph (mail-link 'quotes (string-append "▸ Show quoted text · " (number->string (length quoted)) " lines")))) blocks))))
          (else
           (let ((paragraph (take-while (lambda (l) (and (not (string=? (string-trim l) "")) (not (string-prefix? ">" l)))) lines)))
             (loop (drop lines (length paragraph))
                   (cons (cons 'paragraph (mail-append-map (lambda (l i) (append (if (= i 0) '() (list '(hard-break))) (list l))) paragraph (iota (length paragraph)))) blocks)))))))

(define (reader-tree tree)
  ;; The standalone HTML parser returns URL strings, not editor objects.
  (cond ((string? tree) (safe-text tree))
        ((not (pair? tree)) tree)
        ((and (eq? (car tree) 'link) (string? (cadr tree)))
         (cons 'link (cons (target 'url (cadr tree)) (map reader-tree (cddr tree)))))
        (else (map reader-tree tree))))

(define (parsed-message s m)
  (let ((st (mail-state s)) (raw (mail-ref m 'raw)) (id (mail-ref m 'id)))
    (if (and (equal? id (get st 'parsed-id)) (equal? raw (get st 'parsed-raw)))
        (get st 'parsed)
        (let ((parsed (mail-parse raw)))
          (put! st 'parsed-id id) (put! st 'parsed-raw raw) (put! st 'parsed parsed)
          (put! st 'html-tree #f) parsed))))
(define (cached-html s html)
  (let ((st (mail-state s)))
    (or (get st 'html-tree)
        (let ((tree (reader-tree (mail-html->rich html)))) (put! st 'html-tree tree) tree))))

(define (message-blocks s m)
  (let* ((st (mail-state s)) (parsed (parsed-message s m))
         (html (mail-html parsed)) (text (mail-text parsed))
         (html? (and html (case (get st 'body-view 'auto)
                            ((plain) #f) ((html) #t) (else (or (option #f 'mail-prefer-html) (not text))))))
         (attachments (mail-attachments parsed)) (id (mail-ref m 'id))
         (details? (get st 'details?))
         ;; The list draws the conversation; details repeat it for a reader
         ;; shown alone, and include what the list's group or search leaves out.
         (thread (if details? (mail-thread-members (mail-store-query (database!) "" #:group 'all #:limit 4000) m) '())))
    (append
     ;; Two rows under the subject, then the message: the body is what the pane is for.
     (list
      (list 'heading 1 (one-line (mail-ref m 'subject)))
      (list 'paragraph (list 'strong (one-line (mail-ref m 'sender))) "  ·  " (long-date (mail-ref m 'date))
            (if (= (mail-ref m 'starred) 1) "  ·  ★" "")
            '(hard-break)
            (mail-link 'details (string-append (if details? "▾ to " "▸ to ") (short (mail-ref m 'recipients) 72)))))
     (if details?
         (list (list 'paragraph
                     (list 'strong "Account  ") (one-line (mail-ref m 'account)) '(hard-break)
                     (list 'strong "Mailbox  ") (one-line (string-join (mail-store-copy-mailboxes (database!) id) ", ")) '(hard-break)
                     (list 'strong "To  ") (one-line (mail-header parsed "to")) '(hard-break)
                     (list 'strong "Cc  ") (one-line (mail-header parsed "cc" "—")) '(hard-break)
                     (list 'strong "Reply-To  ") (one-line (mail-header parsed "reply-to" "From address (no override)")) '(hard-break)
                     (list 'strong "Message-ID  ") (one-line (mail-header parsed "message-id" "—")))) '())
     (if (> (length thread) 1)
         (list (cons 'paragraph
                     (mail-append-map
                      (lambda (item i)
                        (let ((label (string-append (if (= (mail-ref item 'unread) 1) "● " "")
                                                    (mail-ref item 'tree "") (sender-name item)
                                                    " · " (compact-date (mail-ref item 'date)))))
                          (append (if (= i 0) '() (list '(hard-break)))
                                  (list (if (= id (mail-ref item 'id)) (list 'strong label)
                                            (mail-link 'thread label (mail-ref item 'id)))))))
                      (mail-thread-tree (reverse thread)) (iota (length thread)))))
         '())
     (if (pair? (mail-ref parsed 'errors '())) (list '(paragraph "Some MIME parts could not be decoded. Intact parts are shown; Source is available.")) '())
     (guard (e ((not (task-cancelled-condition? e)) (append (list '(paragraph "Rich rendering reached a safety limit. Plain text or Source remains available."))
                           (if text (plain-rich text #f) '()))))
       (cond (html? (cached-html s html))
             (text (plain-rich text (get st 'quotes?)))
             (html (cached-html s html))
             (else (list '(paragraph "This message has no displayable text part.")))))
     (if (null? attachments) '()
         (append (list '(rule) (list 'heading 3 (string-append "Attachments · " (number->string (length attachments)))))
                 (map (lambda (a i)
                        (list 'paragraph (mail-link 'attachment (string-append "↓  " (one-line (mail-ref a 'name "attachment"))) id i)
                              "   " (one-line (mail-ref a 'type "application/octet-stream"))
                              " · " (number->string (bytevector-length (mail-ref a 'bytes))) " bytes")) attachments (iota (length attachments)))))
     ;; What to do with a message comes after reading it.
     (list '(rule)
           (list 'paragraph (mail-link 'reply "Reply" id)
                 "     " (mail-link 'archive "Archive" id)
                 "     " (mail-link 'delete "Delete" id)
                 "     " (mail-link 'star (if (= (mail-ref m 'starred) 1) "Unstar" "Star") id)
                 "     " (mail-link 'source "Source" id)
                 "     " (mail-link 'back "← Back to messages")))
     (if html? (list (list 'paragraph (list 'emph "HTML view · remote images blocked · v shows plain text"))) '()))))

(define (empty-reader-blocks s)
  (let ((st (mail-state s)))
    (if (get st 'cached? #t)
        (list (list 'heading 1 (if (and (eq? (get st 'group) 'unread) (string=? (get st 'query "") ""))
                                  "You're caught up." "No messages in this view."))
              (list 'paragraph (mail-link 'all "Browse all mail →"))
              (list 'paragraph (mail-link 'dashboard "Views, accounts and folders →"))
              (list 'paragraph (mail-link 'sync-status "Sync status →")))
        (list '(heading 1 "No mail yet")
              '(paragraph "Set mail-accounts in your Scheme configuration, then g fetches mail. See doc/mail.org for examples. Nothing is fetched until you ask, and nothing is sent without confirmation.")))))

(define (reader! s)
  (let* ((st (mail-state s)) (b (reader-buffer! st))
         ;; The displayed reader has its own identity, even when opening from
         ;; Unread removes that message from the summary during this refresh.
         (m (let ((id (or (get st 'reader-id) (get st 'selected))))
              (and id (mail-store-message (database!) id))))
         (id (and m (mail-ref m 'id)))
         (same? (equal? id (get (buffer-state b) 'rendered-id)))
         (places (filter-map (lambda (v)
                               (and (document=? (view-document v) (buffer-document b))
                                    (list v (cadr (list-ref (view-ranges v) (view-primary v))) (view-scroll v))))
                             (let ((v (get st 'reader-view)))
                                (if (and v (not (memq v (session-panes s))))
                                    (cons v (session-panes s)) (session-panes s))))))
    (rich-present! b (if m (message-blocks s m) (empty-reader-blocks s)))
    ;; Generated rich rows have no source anchors. Rebuilding headers must not
    ;; drag a reader's caret/scroll to the final replacement row.
    (for-each (lambda (place)
                (let* ((v (car place)) (d (buffer-document b)) (len (document-length d))
                       (point (if same? (line-start d (min (cadr place) len)) 0))
                       (scroll (if same? (line-start d (min (caddr place) len)) 0)))
                  (view-set-ranges! v (list (list point point)) 0) (view-set-scroll! v scroll))) places)
    (put! (buffer-state b) 'rendered-id id)
    (put! st 'reader-id id)
    b))

(define (reader-buffer! st)
  ;; The reader's buffer, made empty the first time: a list opens alone.
  (or (get st 'reader)
      (let ((b (make-rich-buffer! "*mail reader*" #:mode 'mail-message-mode))) (put! st 'reader b) b)))
(define (refresh! s #:reader? [render-reader? #t])
  ;; The list is what its group and search found when it was last made
  ;; (`listed` unset), as a mu4e or Gnus summary is: a message read,
  ;; starred or marked since stays on its row, shown as it now is, until
  ;; the list is made again. What the query newly finds joins it.
  (let* ((st (mail-state s)) (view (view-of st)) (query (get st 'query ""))
         (found (view-query view query))
         (listed (get st 'listed))
         (rows (if (not listed) (begin (set! %kept (make-hash-table)) found)
                   (let ((fresh (make-hash-table)))
                     (for-each (lambda (m) (hash-table-set! fresh (mail-ref m 'id) #t)) found)
                     (let ((kept (filter-map (lambda (id)
                                               (and (not (hash-table-ref/default fresh id #f))
                                                    (or (hash-table-ref/default %kept id #f)
                                                        (let ((m (mail-store-message (database!) id #:raw? #f)))
                                                          (when m (hash-table-set! %kept id m))
                                                          m))))
                                             listed)))
                       (if (null? kept) found
                           (sort (append kept found)
                                 (lambda (a b)
                                   (let ((ta (mail-ref a 'sent-at 0)) (tb (mail-ref b 'sent-at 0)))
                                     (if (= ta tb) (> (mail-ref a 'id) (mail-ref b 'id)) (> ta tb))))))))))
         ;; Conversations by their newest message; inside one, a tree of replies.
         (groups (mail-thread-groups rows))
         (messages (mail-append-map mail-thread-tree groups))
         (id (get st 'selected)))
    (put! st 'listed (map (lambda (m) (mail-ref m 'id)) rows))
    (put! st 'messages messages)
    ;; A folder's list leaves out a second delivery of a message, so its
    ;; length does not say whether the limit was reached: count.
    (put! st 'view-total (max (length messages) (view-count view query)))
    (put! st 'threads (let ((threads (make-hash-table)))
                        (for-each (lambda (group)
                                    (let ((ids (map (lambda (m) (mail-ref m 'id)) group)))
                                      (for-each (lambda (id) (hash-table-set! threads id ids)) ids)))
                                  groups)
                        threads))
    (put! st 'view-unread (if (eq? (car view) 'unread) 0
                              (view-count view (string-append query " is:unread"))))
    (put! st 'cached? (or (pair? messages) (> (mail-ref (mail-store-counts (database!)) 'total) 0)))
    (unless (find (lambda (m) (equal? (mail-ref m 'id) id)) messages)
      (put! st 'selected (and (pair? messages) (mail-ref (car messages) 'id))))
    (let ((b (make-listing-buffer! "*mail summary*" 'mail-summary-mode summary-rows #:state (list (cons 'mail-state st)))))
      (put! st 'summary b))
    ;; The reader is made only while it is shown: a list opens alone,
    ;; and RET opens a message.
    (when (and render-reader? (get st 'reader-id) (not (get st 'reader-closed?))) (reader! s))))

(define (mail-single-pane? s)
  (if (eq? (option #f 'mail-reader-position) 'bottom)
      (< (or (sget s 'viewport-rows) 50) 36)
      (narrow? s)))
(define (mail-layout-width! s)
  (let ((st (mail-state s)))
    (put! st 'columns (or (sget s 'viewport-columns) 160))
    (put! st 'full-width? (or (eq? (option #f 'mail-reader-position) 'bottom)
                              (get st 'maximized?) (get st 'reader-closed?) (mail-single-pane? s)))))
(define (arrange-mail! s)
  ;; Reuse the views: orientation changes must not reset point/scroll or
  ;; confuse the selected row with the independently opened message.
  (let* ((st (mail-state s)) (position (option #f 'mail-reader-position))
         ;; A reader closed (q) leaves the list alone until RET opens one.
         (reader? (and (eq? (buffer-mode (current-buffer s)) 'mail-message-mode) (not (get st 'reader-closed?))))
         (single? (or (get st 'maximized?) (get st 'reader-closed?) (mail-single-pane? s))))
    (define (view-for b key)
      (or (find (lambda (v) (document=? (view-document v) (buffer-document b))) (session-panes s))
          (let ((v (get st key))) (and v (document=? (view-document v) (buffer-document b)) v))
          (make-view (buffer-document b) "user")))
    (let ((summary (view-for (get st 'summary) 'summary-view))
          (reader (view-for (reader-buffer! st) 'reader-view)))
      (put! st 'summary-view summary) (put! st 'reader-view reader)
      (put! st 'layout-position position)
      (mail-layout-width! s)
      (set-session-panes! s (if single? (list (if reader? reader summary)) (list summary reader))
                          (if (and reader? (not single?)) 1 0))
      (unless single?
        (set-session-tree! s (if (eq? position 'bottom)
                                '(split below ((0.4 . 0) (0.6 . 1)))
                                '(split right ((0.38 . 0) (0.62 . 1)))))))))
(define (workspace! s)
  (let ((st (mail-state s)))
    (unless (get st 'before)
      (put! st 'before (list (session-panes s) (session-tree s) (session-focus s))))
    ;; The list alone, as mu4e's headers and Gnus's summary open: the
    ;; reader opens with RET, beside or below it.
    (put! st 'maximized? #f) (put! st 'reader-closed? #t)
    (mail-layout-width! s)
    (put! st 'listed #f)
    (refresh! s) (arrange-mail! s) (sset! s 'focus 0)
    (let ((id (get st 'selected))) (when id (listing-goto! s (get st 'summary) (number->string id))))))

(add-hook! 'viewport-changed 'mail-layout
  (lambda (s columns rows)
    (let ((b (current-buffer s)) (st (sget s 'mail)))
      (when (and st b (memq (buffer-mode b) '(mail-summary-mode mail-message-mode)))
        (arrange-mail! s) (refresh! s))
      (when (and st b (eq? (buffer-mode b) 'mail-dashboard-mode))
        (put! st 'columns (or columns 100)) (listing-redisplay! b)))))
(add-hook! 'operation-ended 'mail-reader-position
  (lambda (s op)
    (let ((b (current-buffer s)) (st (sget s 'mail)))
      (when (and st b (memq (buffer-mode b) '(mail-summary-mode mail-message-mode))
                 (not (eq? (get st 'layout-position) (option #f 'mail-reader-position))))
        (arrange-mail! s) (listing-redisplay! (get st 'summary))))))
;; Inherited listing commands (mark/unmark, paging, buffer ends) move
;; point too. With no caret, the highlighted row must stay authoritative.
(add-hook! 'operation-ended 'mail-row-selection
  (lambda (s op)
    (let ((b (current-buffer s)) (st (sget s 'mail)))
      (when (and st b (eq? (buffer-mode b) 'mail-summary-mode) (not (sget s 'transient)))
        (let ((messages (get st 'messages '())) (at (row-id s)))
          (when (pair? messages)
            (let* ((first-id (mail-ref (car messages) 'id))
                   (first-span (presentation-row-span (doc s) (number->string first-id)))
                   (id (or at (if (and first-span (< (point s) (car first-span))) first-id
                                  (mail-ref (car (reverse messages)) 'id)))))
              ;; The highlight is drawn from `selected`: nothing to make again.
              (put! st 'selected id)
              (unless at (listing-goto! s b (number->string id))))))))))

(register-mode-line-segment! 'mail-context
  (lambda (b)
    (case (buffer-mode b)
      ((mail-dashboard-mode) "RET open · i u a views · / search · g fetch · C compose · q quit")
      ((mail-summary-mode) "RET read · r d ! ? + mark · % T several · x do · c all read · q dashboard")
      ((mail-drafts-mode) "RET open · d flag · x delete · C compose · q dashboard")
      ((mail-message-mode) "n/p next · R reply · r archive · x do · b back")
      ((mail-compose-mode)
       (let ((st (buffer-state b)))
         (cond ((get st 'uncertain?) "DELIVERY UNCERTAIN · C-c C-r resolve")
               ((get st 'sending?) "Sending…") ((get st 'sent?) "Accepted by SMTP")
               ((get st 'save-error?) "DRAFT SAVE FAILED · C-c C-d retry")
               ((not (equal? (get st 'saved-revision) (document-revision (buffer-document b)))) "Saving draft…")
               (else "Autosaved · C-c C-c send · C-c C-d later · C-c C-k discard"))))
      (else #f))))

(define-command (mail s n)
  "Open mail at its dashboard: views, then each account and its folders.
RET on one lists its messages; RET on a message opens it in a private
rich reader below or beside the list. Set `mail-accounts` in your Scheme
configuration; see doc/mail.org for examples."
  (database!) (show-dashboard! s))

(define (open-view! s view #:query [query ""])
  ;; Every way into a list comes here, from the dashboard, G, a search
  ;; or M-x from any buffer: the list above the reader, made anew.
  (let ((st (mail-state s)))
    (put! st 'group (car view)) (put! st 'account (cadr view)) (put! st 'mailbox (caddr view))
    (put! st 'query query) (put! st 'reader-id #f) (put! st 'selected #f)
    (workspace! s)))

;;; The dashboard, as mu4e's main view or Gnus's group buffer: where mail
;;; starts and q from a list returns to. The groups across accounts, then
;;; every account and its folders, with how much is unread in each, then
;;; what there is to do. RET opens a view as a list above the reader.
(define-mode mail-dashboard-mode
  "Mail's dashboard: groups, accounts and their folders, and commands.
n/p select a row and RET opens it: a group or folder as a list of its
messages above the reader. i, u, s and a open Inbox, Unread, Starred
and All mail. TAB folds an account's folders; c marks all of
the selected row's mail read. / searches all mail, g fetches mail, C
composes, D opens the drafts, and q leaves mail."
  #:parent 'listing-mode
  #:layer (lambda (d from to) (dashboard-layer d))
  #:options '((line-numbers . #f) (eob-marker . "") (cursor-visible . #f))
  #:keys '(("n" mail-dashboard-next) ("p" mail-dashboard-previous) ("j" mail-dashboard-next) ("k" mail-dashboard-previous)
           ("C-n" mail-dashboard-next) ("C-p" mail-dashboard-previous) ("<down>" mail-dashboard-next) ("<up>" mail-dashboard-previous)
           ("SPC" mail-dashboard-next) ("RET" mail-dashboard-open) ("TAB" mail-dashboard-fold)
           ("i" mail-inbox) ("u" mail-unread) ("s" mail-starred) ("a" mail-all)
           ("/" mail-search) ("G" mail-groups) ("g" mail-sync) ("e" mail-sync-status) ("c" mail-catch-up)
           ("C" mail-compose) ("D" mail-drafts) ("q" mail-quit) ("C-c C-k" mail-cancel-sync)
           ("C-/" mail-undo) ("C-_" mail-undo))
  #:normal '(("j" mail-dashboard-next) ("k" mail-dashboard-previous) ("n" mail-dashboard-next) ("p" mail-dashboard-previous)
             ("C-n" mail-dashboard-next) ("C-p" mail-dashboard-previous) ("<down>" mail-dashboard-next) ("<up>" mail-dashboard-previous)
             ("RET" mail-dashboard-open) ("TAB" mail-dashboard-fold)
             ("i" mail-inbox) ("u" mail-unread) ("s" mail-starred) ("a" mail-all)
             ("/" mail-search) ("G" mail-groups) ("g r" mail-sync) ("e" mail-sync-status) ("c" mail-catch-up)
             ("C" mail-compose) ("D" mail-drafts) ("q" mail-quit)))

(define (overview)
  ;; (ID FOLDER ...) for every account: configured ones in the order of
  ;; `mail-accounts`, then any the cache holds mail of that no longer is.
  (let* ((folders (mail-store-folders (database!)))
         (configured (filter-map (lambda (a) (let ((id (mail-ref a 'id))) (and (string? id) id))) (configured-accounts)))
         (cached (delete-duplicates (map (lambda (f) (mail-ref f 'account)) folders)))
         (ids (append configured (remove (lambda (id) (member id configured)) cached))))
    (map (lambda (id)
           (cons id (by-rank (filter (lambda (f) (equal? (mail-ref f 'account) id)) folders)
                             (lambda (f) (mail-ref f 'mailbox)))))
         ids)))
(define (sum-of folders key) (fold (lambda (f n) (+ n (or (mail-ref f key) 0))) 0 folders))
(define (inbox-folder? f) (string=? (string-downcase (mail-ref f 'mailbox)) "inbox"))
(define (account-view id cached)
  ;; An account's row opens its Inbox; one without, all of its mail.
  (if (any inbox-folder? cached) (list 'inbox id #f) (list 'all id #f)))
(define (account-folders-shown cached)
  ;; What an account's row counts: its Inbox, else all its mail, a
  ;; message once.
  (let ((inbox (filter inbox-folder? cached)))
    (if (pair? inbox) inbox (map (lambda (f) (list (cons 'messages (mail-ref f 'shown)) (cons 'unread (mail-ref f 'shown-unread)))) cached))))

(define (dashboard-rows b)
  (let* ((st (listing-get b 'mail-state)) (accounts (get st 'overview '()))
         (folders (append-map cdr accounts)) (folded (get st 'folded '()))
         (width (max 48 (- (get st 'columns 100) 2)))
         (name-width (max 18 (min 40 (- width 36))))
         (keys '()))
    (define (pad-left text n) (string-append (make-string (max 0 (- n (mail-columns text))) #\space) text))
    (define (line key target hint label unread total note #:indent [indent ""])
      ;; A row to select: the key that opens it, its name, how many of its
      ;; messages are unread and how many there are, and a note.
      (set! keys (cons key keys))
      (row #:key key #:target target
           (list (list (string-append "  " (mail-pad hint 3)) 'mail.status)
                 (list (mail-pad (string-append indent label) name-width) (if (> unread 0) 'mail.unread 'mail.subject))
                 (list (pad-left (if (> unread 0) (grouped unread) "") 8) 'mail.unread)
                 (list (pad-left (if total (grouped total) "") 10) 'mail.count)
                 (list (if note (string-append "   " (short note (max 10 (- width name-width 26)))) "") 'mail.preview))))
    (define (heading key label #:counts? [counts? #f])
      (row #:key key (list (list (string-append "  " (if counts? (mail-pad label (+ 3 name-width)) label)) 'mail.section)
                           (list (if counts? (string-append (pad-left "Unread" 8) (pad-left "Messages" 10)) "") 'mail.section))))
    (define (account-note id cached)
      (let ((status (hash-table-ref/default %account-status id #f)))
        (cond (status status)
              ((not (account-by-id id)) "not configured · g drops it")
              ((null? cached) "not fetched yet · g fetches it")
              (else #f))))
    (let ((rows
           (append
            (list (row #:key "title" (list (list "  Mail" 'mail.title)))
                  (row #:key "status" #:target (target 'mail-control '(sync-status))
                       (list (list (string-append "  " (short (if (sync-quiet?)
                                                                  (string-append (grouped (sum-of folders 'shown)) " messages cached"
                                                                                 (if (pair? (configured-accounts)) " · g fetches new mail" ""))
                                                                  %sync-status)
                                                              (- width 2)))
                                   'mail.preview)))
                  (row #:key "gap-1" "")
                  (heading "views" "Views" #:counts? #t)
                  (line "inbox" (target 'mail-view '(inbox #f #f)) "i" "Inbox"
                        (sum-of (filter inbox-folder? folders) 'unread) (sum-of (filter inbox-folder? folders) 'messages) #f)
                  (line "unread" (target 'mail-view '(unread #f #f)) "u" "Unread"
                        (sum-of folders 'shown-unread) #f #f)
                  (line "starred" (target 'mail-view '(starred #f #f)) "s" "Starred" 0 (sum-of folders 'starred) #f)
                  (line "all" (target 'mail-view '(all #f #f)) "a" "All mail" (sum-of folders 'shown-unread) (sum-of folders 'shown) #f)
                  (line "drafts" (target 'mail-control '(drafts)) "D" "Drafts" 0 (get st 'drafts 0) #f)
                  (row #:key "gap-2" "")
                  (heading "accounts" "Accounts" #:counts? #t))
            (if (null? accounts)
                (list (row #:key "no-accounts" (list (list "  No accounts yet. Set mail-accounts in your Scheme configuration." (quote mail.preview)))))
                (append-map
                 (lambda (account)
                   (let* ((id (car account)) (cached (cdr account)) (open? (not (member id folded))))
                     (cons (line (string-append "account:" id) (target 'mail-view (account-view id cached))
                                 (if (null? cached) "" (if open? "▾" "▸")) id
                                 (sum-of (account-folders-shown cached) 'unread) (and (pair? cached) (sum-of (account-folders-shown cached) 'messages))
                                 (account-note id cached))
                           (if open?
                               (map (lambda (f)
                                      (line (string-append "folder:" id "\x0;" (mail-ref f 'mailbox))
                                            (target 'mail-view (list 'all id (mail-ref f 'mailbox)))
                                            "" (mail-ref f 'mailbox) (mail-ref f 'unread) (mail-ref f 'messages) #f #:indent "  "))
                                    cached)
                               '()))))
                 accounts))
            (list (row #:key "gap-3" "")
                  (heading "do" "Do")
                  (line "search" (target 'mail-control '(search)) "/" "Search all mail" 0 #f #f)
                  (line "compose" (target 'mail-control '(compose)) "C" "Compose" 0 #f #f)

                  (line "sync" (target 'mail-control '(sync)) "g" "Fetch mail" 0 #f
                        (if %sync-task "running · C-c C-k cancels" #f))
                  (line "quit" (target 'mail-control '(quit)) "q" "Leave mail" 0 #f #f)))))
      (put! st 'dash-keys (reverse keys))
      rows)))

(define (dashboard-layer d)
  ;; The selected row's background, as in the list of messages.
  (let* ((b (document-buffer d)) (st (and b (listing-get b 'mail-state))) (key (and st (get st 'dash-key)))
         (span (and key (presentation-row-span d key))))
    (if span (list (list (car span) (min (document-length d) (+ 1 (cadr span))) 'mail.selected)) '())))

(define (dashboard! s)
  ;; The dashboard's rows made again, from what the cache holds now.
  (let ((st (mail-state s)))
    (put! st 'columns (or (sget s 'viewport-columns) 100))
    (put! st 'overview (overview))
    (put! st 'drafts (length (mail-store-drafts (database!))))
    (let ((b (make-listing-buffer! "*mail*" 'mail-dashboard-mode dashboard-rows #:state (list (cons 'mail-state st)))))
      (put! st 'dashboard b)
      (let ((keys (get st 'dash-keys '())))
        (unless (member (get st 'dash-key) keys)
          (put! st 'dash-key (and (pair? keys) (car keys)))
          (listing-redisplay! b)))
      b)))
(define (dashboard-shown? s)
  (let ((b (get (mail-state s) 'dashboard)))
    (and b (any (lambda (v) (document=? (view-document v) (buffer-document b))) (session-panes s)))))
(define (show-dashboard! s)
  ;; The dashboard alone in the frame, the panes before mail kept for q.
  (let ((st (mail-state s)))
    (unless (get st 'before)
      (put! st 'before (list (session-panes s) (session-tree s) (session-focus s))))
    (let* ((b (dashboard! s))
           (v (or (let ((v (get st 'dashboard-view))) (and v (document=? (view-document v) (buffer-document b)) v))
                  (make-view (buffer-document b) "user"))))
      (put! st 'dashboard-view v)
      (set-session-panes! s (list v) 0)
      (when (get st 'dash-key) (listing-goto! s b (get st 'dash-key))))))

(define (dashboard-step! s n)
  (let* ((st (mail-state s)) (b (get st 'dashboard)) (keys (get st 'dash-keys '()))
         (at (or (list-index (lambda (k) (equal? k (get st 'dash-key))) keys) 0))
         (next (max 0 (min (- (length keys) 1) (+ at n)))))
    (when (pair? keys)
      (put! st 'dash-key (list-ref keys next))
      (listing-redisplay! b)
      (listing-goto! s b (list-ref keys next)))))
(define (dashboard-target s)
  (let* ((st (mail-state s)) (b (get st 'dashboard)) (key (get st 'dash-key))
         (r (and b key (hash-table-ref/default (listing-get b 'rows) key #f))))
    (and r (row-target r))))
(define-command (mail-dashboard-next s n) "Select the next row of the mail dashboard." (dashboard-step! s n))
(define-command (mail-dashboard-previous s n) "Select the previous row of the mail dashboard." (dashboard-step! s (- n)))
(define-command (mail-dashboard-open s n)
  "Open the selected group, account or folder, or do the selected command."
  (let ((t (dashboard-target s)))
    (cond ((not t) (message! s "Nothing to open here"))
          ((eq? (target-kind t) 'mail-view) (open-view! s (target-value t)))
          (else (mail-use-control s (target-value t))))))
(define-command (mail-dashboard-fold s n)
  "Show or hide the folders of the selected account, as Gnus folds a topic."
  (let* ((st (mail-state s)) (t (dashboard-target s)) (view (and t (eq? (target-kind t) 'mail-view) (target-value t))))
    (if (not (and view (cadr view)))
        (message! s "Select an account to fold its folders")
        (let ((id (cadr view)) (folded (get st 'folded '())))
          (put! st 'folded (if (member id folded) (delete id folded) (cons id folded)))
          (put! st 'dash-key (string-append "account:" id))
          (listing-redisplay! (get st 'dashboard))
          (listing-goto! s (get st 'dashboard) (get st 'dash-key))))))
(define-command (mail-dashboard s n)
  "Show the mail dashboard: groups, accounts and folders, and commands."
  (database!) (show-dashboard! s))
(define-action mail-view (mail-open-view s view)
  "List the messages of this group, account or folder above the reader."
  (open-view! s view))
(define-command (mail-inbox s n) "List the Inbox of every account." (open-view! s '(inbox #f #f)))
(define-command (mail-unread s n) "List the unread mail of every account." (open-view! s '(unread #f #f)))
(define-command (mail-starred s n) "List the starred mail of every account." (open-view! s '(starred #f #f)))

(define-command (mail-all s n) "List all mail of every account." (open-view! s '(all #f #f)))
;; Selection is the row's background; whatever moved point (a click,
;; M-<) selects the row it is on, or goes back to the selected one.
(add-hook! 'operation-ended 'mail-dashboard-selection
  (lambda (s op)
    (let ((b (current-buffer s)) (st (sget s 'mail)))
      (when (and st b (eq? (buffer-mode b) 'mail-dashboard-mode) (not (sget s 'transient)))
        (let ((at (listing-key-at b (point s))))
          (cond ((and at (member at (get st 'dash-keys '())))
                 (unless (equal? at (get st 'dash-key)) (put! st 'dash-key at) (listing-redisplay! b)))
                ((get st 'dash-key) (listing-goto! s b (get st 'dash-key)))))))))

(define (reveal-summary! s)
  ;; Reader navigation updates an unfocused view. Frontends only auto-follow
  ;; the focused caret, so ask their existing visual recenter mechanism to
  ;; reveal this row without guessing pixels or changing the reader's focus.
  (let* ((st (mail-state s)) (b (get st 'summary)) (id (get st 'selected))
         (span (and b id (presentation-row-span (buffer-document b) (number->string id))))
         (shown (and b (find (lambda (v) (document=? (view-document v) (buffer-document b))) (session-panes s))))
         (v (or shown (get st 'summary-view))))
    (when (and span v (document=? (view-document v) (buffer-document b)))
      (view-set-ranges! v (list (list (car span) (car span))) 0)
      (when shown (request-view! s '(recenter middle) #:view v)))))

(define-command (mail-next s n)
  "Select the next summary without decoding its body.
In the reader, open it and mark it read."
  (step! s n))
(define-command (mail-previous s n)
  "Select the previous summary; in the reader, mark it read."
  (step! s (- n)))
(define (step! s n)
  (let* ((st (mail-state s)) (messages (get st 'messages '()))
         (reader? (eq? (buffer-mode (current-buffer s)) 'mail-message-mode))
         (ids (if reader? (get st 'navigation '()) (map (lambda (m) (mail-ref m 'id)) messages)))
         (index (or (list-index (lambda (id) (equal? id (if reader? (get st 'reader-id) (or (row-id s) (get st 'selected))))) ids) 0)))
    (if (null? ids) (message! s "No messages here")
        (let ((next (+ index n)))
          (if (or (< next 0) (>= next (length ids))) (message! s "No more messages in this group")
              ;; Selecting a row only moves the highlight, which is drawn
              ;; from `selected` each time: the list is not made again.
              (begin (put! st 'selected (list-ref ids next))
                     (if reader?
                         (begin
                           (put! st 'reader-id (list-ref ids next)) (put! st 'quotes? #f) (put! st 'body-view 'auto)
                           (remember-state! (list (get st 'reader-id)) 'unread #f)
                           (shown-read! s (get st 'reader-id))
                           (reader! s)
                           (move! s (lambda (p) 0)) (view-set-scroll! (session-view s) 0)
                           (reveal-summary! s))
                         (listing-goto! s (get st 'summary) (number->string (get st 'selected))))))))))
(define (shown-read! s id)
  ;; Message ID, just read, as its row and the header's count show it:
  ;; what the list holds is changed, not asked of the cache again.
  (let* ((st (mail-state s)) (was? #f)
         (messages (map (lambda (m)
                          (if (and (equal? (mail-ref m 'id) id) (= (mail-ref m 'unread) 1))
                              (begin (set! was? #t) (cons (cons 'unread 0) m))
                              m))
                        (get st 'messages '()))))
    (when was?
      (put! st 'messages messages)
      (put! st 'view-unread (max 0 (- (get st 'view-unread 0) 1)))
      (when (get st 'summary) (listing-redisplay! (get st 'summary))))))

(define-command (mail-read s n)
  "Read the message at point, marking it read (S, for a Maildir message).
In the reader, n and p then follow the list: through a conversation's
replies in order, then on to the next one."
  (let* ((st (mail-state s)) (id (current-id s)))
    (put! st 'selected id) (put! st 'reader-id id)
    (put! st 'navigation (map (lambda (m) (mail-ref m 'id)) (get st 'messages '())))
    (put! st 'quotes? #f) (put! st 'body-view 'auto)
    (remember-state! (list id) 'unread #f) (shown-read! s id) (reader! s)
    ;; The list stays when a message opens: a reader closed with q, a
    ;; list given the frame with f, or the list shown alone any other
    ;; way, splits again. Only a frame too small for two keeps one pane.
    (unless (or (mail-single-pane? s) (= (length (session-panes s)) 2))
      (put! st 'reader-closed? #f) (put! st 'maximized? #f)
      (focus-mail-buffer! s (get st 'summary) 0)
      (arrange-mail! s) (listing-redisplay! (get st 'summary)))
    (focus-mail-buffer! s (get st 'reader) 1)
    (move! s (lambda (p) 0)) (view-set-scroll! (session-view s) 0)
    (reveal-summary! s)))
(define-action mail-message (mail-open-message s id)
  "Read this cached message in the private reader."
  (put! (mail-state s) 'selected id) (put! (mail-state s) 'reader-id id) (mail-read s 1))

(define (focus-mail-buffer! s b preferred)
  (let* ((panes (session-panes s))
         (shown (list-index (lambda (v) (document=? (view-document v) (buffer-document b))) panes)))
    (sset! s 'focus (or shown (min preferred (- (length panes) 1))))
    (show-buffer! s b)))

(define-command (mail-toggle-layout s n)
  "Maximize this mail pane, or restore the list and reader split.
Short bottom layouts and narrow side layouts stay in one pane."
  (if (and (= (length (session-panes s)) 1) (mail-single-pane? s))
      (message! s "Full-width mail · b back to messages · RET opens a message")
      (let ((st (mail-state s)))
        (cond ((and (get st 'reader-closed?) (not (get st 'reader-id)))
               (message! s "RET opens the selected message beside the list"))
              ((get st 'reader-closed?)
               ;; The list alone after q: f splits again, as RET would.
               (put! st 'reader-closed? #f) (reader! s)
               (arrange-mail! s) (listing-redisplay! (get st 'summary)))
              (else
               (put! st 'maximized? (> (length (session-panes s)) 1))
               (arrange-mail! s) (listing-redisplay! (get st 'summary)))))))
(define-command (mail-close-reader s n)
  "Close the reader's pane, leaving the list the frame; RET opens it again.
In one pane, as a narrow frame has, go back to the list."
  (let ((st (mail-state s)))
    (if (or (= (length (session-panes s)) 1) (mail-single-pane? s))
        (mail-back s 1)
        (begin (put! st 'reader-closed? #t)
               (focus-mail-buffer! s (get st 'summary) 0)
               (arrange-mail! s) (listing-redisplay! (get st 'summary))
               (when (get st 'selected) (listing-goto! s (get st 'summary) (number->string (get st 'selected))))))))
(define-command (mail-toggle-reader-position s n)
  "Switch the mail reader between bottom and side, preserving views.
Set the global mail-reader-position preference for this runtime."
  (set-option! 'mail-reader-position (if (eq? (option #f 'mail-reader-position) 'bottom) 'side 'bottom))
  (let ((b (current-buffer s)))
    (when (and (sget s 'mail) b (memq (buffer-mode b) '(mail-summary-mode mail-message-mode)))
      (arrange-mail! s) (listing-redisplay! (get (mail-state s) 'summary))))
  (message! s (string-append "Mail reader: " (symbol->string (option #f 'mail-reader-position)))))

(define (mail-mode? b modes) (and b (memq (buffer-mode b) modes) #t))
(define (note-origin! s)
  ;; Where b and q go back to from help, setup, sync details, source or
  ;; a draft: the dashboard, or the list and reader.
  (let ((b (current-buffer s)))
    (cond ((mail-mode? b '(mail-dashboard-mode)) (put! (mail-state s) 'origin 'dashboard))
          ((mail-mode? b '(mail-drafts-mode)) (put! (mail-state s) 'origin 'drafts))
          ((mail-mode? b '(mail-summary-mode mail-message-mode)) (put! (mail-state s) 'origin 'messages)))))
(define (show-aside! s b)
  ;; Shown in the pane it was asked from.
  (note-origin! s)
  (show-buffer! s b))
(define-command (mail-back s n)
  "Return to the selected summary, saving an open draft before leaving it.
From what the dashboard showed (help, a draft), return to the dashboard."
  (when (eq? (buffer-mode (current-buffer s)) 'mail-compose-mode) (mail-save-draft s 1))
  (return-from-aside! s))
(define (return-from-aside! s)
  (let* ((st (mail-state s)) (b (get st 'summary))
         (aside? (not (mail-mode? (current-buffer s) '(mail-summary-mode mail-message-mode mail-dashboard-mode mail-drafts-mode)))))
    (cond ((and aside? (eq? (get st 'origin) 'dashboard)) (show-dashboard! s))
          ((and aside? (eq? (get st 'origin) 'drafts)) (show-drafts! s))
          (b (focus-mail-buffer! s b 0)
             (when (get st 'selected) (listing-goto! s b (number->string (get st 'selected)))))
          (else (workspace! s)))))
(define-command (mail-quit s n)
  "Leave mail and restore the panes that were shown before it."
  (let* ((st (mail-state s)) (before (get st 'before)))
    (when before (set-session-panes! s (car before) (caddr before)) (set-session-tree! s (cadr before)) (put! st 'before #f))))

(define-command (mail-search s n)
  "Search cached mail. Use from:, subject:, in:, account: and is:unread,
is:read or is:starred. Bare words search sender, subject and body; field
filters match literal substrings. All terms must match. For example, use
from:ada subject:release is:unread. From a mail list, search within that
list; from the dashboard or outside mail, search all cached mail."
  (let* ((st (mail-state s))
         (within? (mail-mode? (current-buffer s) '(mail-summary-mode mail-message-mode)))
         (view (if within? (view-of st) '(all #f #f))))
    (completing-read s (if within? (string-append "Search " (view-label view) ": ") "Search all mail: ") '()
                     #:require-match #f #:initial (if within? (get st 'query "") "") #:history 'mail-search
                     #:accept (lambda (s c) (open-view! s view #:query (candidate-text c))))))
(define-command (mail-clear-search s n)
  "Clear the current mail search, keeping the selected group."
  (open-view! s (view-of (mail-state s))))
(define-command (mail-groups s n)
  "Choose a group, an account or one of its folders, and list its mail.
Groups are across accounts: Inbox, Unread, Starred, All mail."
  (let* ((accounts (overview))
         (candidates
          (append
           (map (lambda (g) (candidate (group-label g) #:target (target 'mail-view (list g #f #f)))) '(inbox unread starred all))
           (append-map
            (lambda (account)
              (let ((id (car account)))
                (cons (candidate id #:annotation (string-append (grouped (sum-of (account-folders-shown (cdr account)) 'unread)) " unread")
                                 #:target (target 'mail-view (account-view id (cdr account))))
                      (map (lambda (f)
                             (candidate (view-label (list 'all id (mail-ref f 'mailbox)))
                                        #:annotation (string-append (grouped (mail-ref f 'unread)) " unread · "
                                                                    (grouped (mail-ref f 'messages)) " messages")
                                        #:target (target 'mail-view (list 'all id (mail-ref f 'mailbox)))))
                           (cdr account)))))
            accounts))))
    (completing-read s "Mail group or folder: " candidates
                     #:accept (lambda (s c) (open-view! s (target-value (candidate-target c)))))))

(define (affected-ids s)
  (let ((b (current-buffer s)))
    (if (and b (eq? (buffer-mode b) 'mail-summary-mode))
        (let ((marked (listing-marked b #\*)))
          (if (pair? marked) (map target-value marked) (list (current-id s))))
        (list (current-id s)))))
(define (remember-state! ids field value #:more? [more? #f] #:copies? [copies? #t])
  ;; What `mail-undo` puts back: each message FIELD changes for, with
  ;; what it was. MORE? adds to what is remembered (marks done together).
  ;; A Maildir message's state is its file's flags: it is set there.
  (let ((changed (filter-map (lambda (id)
                               (let ((before (= (mail-ref (mail-store-message (database!) id #:raw? #f) field) 1)))
                                 (and (not (eq? before value)) (list id field before)))) ids)))
    (remember! changed #:more? more?)
    (for-each (lambda (id) (hash-table-delete! %kept id)) ids)
    (mail-store-state! (database!) ids field value #:copies? copies?)))
(define (remember! changed #:more? [more? #f])
  (when (pair? changed)
    (set! %last-before (if more? (append changed %last-before) changed)) (set! %last-change #t)))
(define (change! s field)
  (let* ((ids (affected-ids s)) (messages (map (lambda (id) (mail-store-message (database!) id #:raw? #f)) ids))
         (value (not (= (mail-ref (car messages) field) 1))))
    (remember-state! ids field value)
    (refresh! s)
    (message! s (string-append (number->string (length ids)) (if (= (length ids) 1) " message" " messages")
                               " updated · a undoes"))))

;;; Archive and delete, at the source, as mu4e's refile and trash. A
;;; Maildir message is renamed only: given the T flag, which mbsync
;;; expunges, or moved to another folder of its account; mbsync carries
;;; it to the server. Undone (a) by renaming back, until mbsync runs.
(define (account-maildirs account)
  ;; Every folder of a Maildir account on disk, skipped ones too, as
  ;; (NAME . PATH): where archive and trash may go.
  (let ((root (mail-ref account 'path)))
    (if (not (string? root)) '()
        (map (lambda (f) (cons (if (string=? (car f) "") "INBOX" (car f)) (cdr f)))
             (maildir-folders root)))))
(define (gmail-maildir? folders)
  (any (lambda (f) (string-suffix? "/all mail" (string-downcase (car f)))) folders))
(define (folder-named folders name)
  (find (lambda (f) (string=? (car f) name)) folders))
(define (trash-folder account folders)
  ;; The folder delete moves to: the account's trash, else one named as
  ;; trash usually is.
  (let ((named (mail-ref account 'trash)))
    (if named (or (folder-named folders named) (error "The account's trash folder is not there" named))
        (find (lambda (f) (member (string-downcase (car f)) '("trash" "deleted items" "deleted messages" "[gmail]/trash" "[gmail]/bin")))
              folders))))
(define (dispose! ids how)
  ;; Archive (HOW archive) or delete (delete) the messages IDS; return
  ;; how many were, and what undoes them goes to `mail-undo'.
  (let ((db (database!)) (done 0) (undo '()))
    (for-each
     (lambda (id)
       (let* ((m (or (mail-store-message db id #:raw? #f) (error "No such message" id)))
              (account (account-by-id (mail-ref m 'account)))
              (file? (not (string=? (mail-ref m 'source-path "") "")))
              (copies (mail-store-copies db (list id)))
              (inbox (map car (filter (lambda (r) (string=? (string-downcase (caddr r)) "inbox")) copies))))
         (define (trash! ids)
           ;; T on these copies: mbsync expunges them.
           (let ((live (filter (lambda (i) (= 0 (mail-ref (mail-store-message db i #:raw? #f) 'trashed))) ids)))
             (when (pair? live)
               (set! undo (append (map (lambda (i) (list i 'trashed #f)) live) undo))
               (mail-store-state! db live 'trashed #t #:copies? #f)
               (set! done (+ done 1)))))
         (define (move! folder)
           (unless (string=? (mail-ref m 'mailbox) (car folder))
             (let ((back (mail-store-move! db id (car folder) (cdr folder))))
               (when back (set! undo (cons (list id 'move back) undo)) (set! done (+ done 1))))))
         (hash-table-delete! %kept id)
         (cond
          ;; Kept here only (an IMAP account, or not from a file): hidden here.
          ((not file?) (trash! (if (eq? how 'archive) (if (pair? inbox) inbox (list id)) (map car copies))))
          ((not account) (error "This message's account is no longer configured" (mail-ref m 'account)))
          (else
           (let ((folders (account-maildirs account)))
             (case how
               ((archive)
                (let ((named (mail-ref account 'archive)))
                  (cond (named (move! (or (folder-named folders named) (error "The account's archive folder is not there" named))))
                        ;; Gmail keeps everything in All Mail: archiving is
                        ;; leaving the Inbox, which drops its Inbox label.
                        ((gmail-maildir? folders) (trash! inbox))
                        (else (error "Set archive, the folder archiving moves mail to, for this account" (mail-ref account 'id))))))
               (else
                (let ((trash (and (not (and (gmail-maildir? folders) (not (mail-ref account 'trash))))
                                  (trash-folder account folders))))
                  (cond (trash (move! trash))
                        ;; Gmail: every copy gone, as deleting is there.
                        ((gmail-maildir? folders) (trash! (map car copies)))
                        (else (error "Set trash, the folder deleting moves mail to, for this account" (mail-ref account 'id))))))))))))
     ids)
    (values done undo)))
(define (dispose-now! s how)
  (let ((ids (affected-ids s)))
    (call-with-values (lambda () (dispose! ids how))
      (lambda (done undo)
        (remember! undo)
        (put! (mail-state s) 'listed #f)
        (refresh! s)
        (message! s (string-append (grouped done) (if (= done 1) " message " " messages ")
                                   (if (eq? how 'archive) "archived" "deleted")
                                   " · a undoes until mbsync runs"))))))
(define-command (mail-star s n) "Toggle the star of marked messages, or the current message.
A Maildir message is given or loses its F flag, as mu4e does." (change! s 'starred))
(define-command (mail-archive s n) "Archive marked messages, or the current message, now.
Gmail's: out of the Inbox, so its Inbox label goes; an account's archive
folder, if set, is where it is moved. Press a to undo, until mbsync runs."
  (dispose-now! s 'archive))
(define-command (mail-toggle-unread s n) "Toggle unread on marked messages, or the current message." (change! s 'unread))
(define-command (mail-undo s n)
  "Undo the last change: a message read, or the marks last done.
A Maildir message is renamed back: before mbsync runs, nothing of it
has reached the server."
  (if %last-change
      (let ((db (database!)))
        (set! %kept (make-hash-table))
        ;; By field and value, not by message: thousands at a time.
        (for-each (lambda (field)
                    (for-each (lambda (before)
                                (let ((ids (filter-map (lambda (entry)
                                                         (and (eq? (cadr entry) field) (eq? (caddr entry) before) (car entry)))
                                                       %last-before)))
                                  (when (pair? ids) (mail-store-state! db ids field before #:copies? (not (eq? field 'trashed))))))
                              '(#t #f)))
                  '(unread starred trashed))
        (for-each (lambda (entry)
                    (when (eq? (cadr entry) 'move)
                      (let ((back (caddr entry)))
                        (mail-store-move! db (car entry) (car back) (path-directory (path-directory (cadr back)))))))
                  %last-before)
        (set! %last-change #f) (set! %last-before '()) (put! (mail-state s) 'listed #f)
        (redisplay-mail! s) (message! s "Mail change undone"))
      (message! s "No mail change to undo")))

;;; Marks, as mu4e's: a row is marked for what is to be done with its
;;; message, and x does it for all of them. The first column shows the
;;; mark. m marks rows with * for one of these to be given to them all.
(define (mark! s c)
  (let* ((st (mail-state s)) (b (or (get st 'summary) (error "Open mail first"))) (marks (listing-get b 'marks))
         (generic (filter (lambda (key) (eqv? (hash-table-ref/default marks key #f) #\*)) (hash-table-keys marks)))
         (set (lambda (key) (if c (hash-table-set! marks key c) (hash-table-delete! marks key)))))
    (if (pair? generic)
        (begin (for-each set generic) (listing-redisplay! b))
        (begin (set (number->string (current-id s)))
               (listing-redisplay! b)
               ;; On to the next message, as mu4e goes: its row in the
               ;; list, the message itself in the reader.
               (step! s 1)))))
(define-command (mail-mark-archive s n) "Mark this message to be archived; x does it." (mark! s #\r))
(define-command (mail-mark-delete s n) "Mark this message to be deleted, to the trash; x does it." (mark! s #\D))
(define-command (mail-mark-read s n) "Mark this message to be made read; x does it." (mark! s #\!))
(define-command (mail-mark-unread s n) "Mark this message to be made unread; x does it." (mark! s #\?))
(define-command (mail-mark-star s n) "Mark this message to be starred; x does it." (mark! s #\+))
(define-command (mail-mark-unstar s n) "Mark this message to lose its star; x does it." (mark! s #\-))
(define-command (mail-unmark s n) "Take the mark off this message." (mark! s #f))
(define-command (mail-catch-up s n)
  "Mark every unread message of this group and search read, as Gnus's c.
That is all of them, not only the rows listed, and then, done with it,
back to the dashboard, as Gnus goes back to its groups. On the
dashboard, the selected row's. \\[mail-undo] undoes it all."
  (let* ((st (mail-state s)) (db (database!))
         (dashboard? (mail-mode? (current-buffer s) '(mail-dashboard-mode)))
         ;; On the dashboard, the selected group or folder's.
         (view (if dashboard?
                   (let ((t (dashboard-target s)))
                     (if (and t (eq? (target-kind t) 'mail-view)) (target-value t)
                         (error "Select a group, account or folder to catch up")))
                   (view-of st)))
         (query (string-append (if dashboard? "" (get st 'query "")) " is:unread"))
         (total
          ;; A page at a time: what is made read is not found again.
          (let loop ((total 0) (changed '()))
            (let ((ids (map (lambda (m) (mail-ref m 'id)) (view-query view query #:limit 4000))))
              (if (null? ids)
                  (begin (when (pair? changed) (set! %last-before changed) (set! %last-change #t)) total)
                  (begin (mail-store-state! db ids 'unread #f)
                         (loop (+ total (length ids))
                               (append (map (lambda (id) (list id 'unread #t)) ids) changed))))))))
    (set! %kept (make-hash-table))
    (put! st 'listed #f)
    (when (get st 'summary) (refresh! s))
    (show-dashboard! s)
    (message! s (if (= total 0) "Nothing unread here"
                    (string-append (grouped total) (if (= total 1) " message" " messages") " marked read in "
                                   (view-label view) " · C-/ undoes")))))

;;; Several rows marked at once, as mu4e's % and T: with *, which one of
;;; the mark keys then turns into its own (`mark!`), and x does.
(define (mark-rows! s ids what)
  (let* ((st (mail-state s)) (b (or (get st 'summary) (error "Open a list first")))
         (marks (listing-get b 'marks)) (wanted (make-hash-table)) (n 0))
    (for-each (lambda (id) (hash-table-set! wanted id #t)) ids)
    (for-each (lambda (m)
                (let ((id (mail-ref m 'id)))
                  (when (hash-table-ref/default wanted id #f)
                    (hash-table-set! marks (number->string id) #\*) (set! n (+ n 1)))))
              (get st 'messages '()))
    (listing-redisplay! b)
    (message! s (if (= n 0) (string-append "No row is " what)
                    (string-append (grouped n) (if (= n 1) " row " " rows ") what
                                   " marked * · then ! read, r archive, + star… and x does it")))))
(define-command (mail-mark-pattern s n)
  "Mark with * the listed rows a search finds, as mu4e's %.
The search is as \\[mail-search]'s, within the list's: from:github,
subject:release, words. A mark key then gives them all its mark: ! to
make them read, r to archive. \\[mail-execute] does it."
  (let ((st (mail-state s)))
    (unless (get st 'summary) (error "Open a list first"))
    (completing-read s "Mark rows matching: " '() #:require-match #f #:history 'mail-mark-pattern
                     #:accept (lambda (s c)
                                (mark-rows! s (map (lambda (m) (mail-ref m 'id))
                                                   (view-query (view-of st) (string-append (get st 'query "") " " (candidate-text c))
                                                               #:limit 5000))
                                            (string-append "matching " (candidate-text c)))))))
(define-command (mail-mark-thread s n)
  "Mark with * the listed messages of this conversation, as mu4e's T.
A mark key then gives them all its mark, and \\[mail-execute] does it."
  (let* ((st (mail-state s)) (id (current-id s)))
    (mark-rows! s (hash-table-ref/default (get st 'threads (make-hash-table)) id (list id)) "of this conversation")))
(define-command (mail-mark-keys s n)
  "Say which keys mark a message, where a listing's own would not apply."
  (message! s "r archive · d delete · ! read · ? unread · + star · - unstar · m several · u unmark · x do it · c all read"))
(define-command (mail-execute s n)
  "Do what messages are marked for: archive, delete, read, star...
A Maildir message is renamed, its flags or folder changed as mu4e changes
them, and mbsync carries that to the server; \\[mail-undo] undoes them
all until it runs. The list is then made again, so what left it goes."
  (let* ((st (mail-state s)) (b (get st 'summary)) (marks (and b (listing-get b 'marks)))
         (todo (if marks
                   (filter (lambda (entry) (memv (cdr entry) '(#\r #\D #\! #\? #\+ #\-)))
                           (map (lambda (key) (cons key (hash-table-ref/default marks key #f))) (hash-table-keys marks)))
                   '())))
    (if (null? todo)
        (message! s "Nothing is marked · r archive · d delete · ! read · ? unread · + star · - unstar")
        (begin
          (set! %last-change #f) (set! %last-before '())
          (for-each (lambda (entry)
                      (let ((ids (list (string->number (car entry)))))
                        (case (cdr entry)
                          ((#\r #\D) (call-with-values (lambda () (dispose! ids (if (eqv? (cdr entry) #\r) 'archive 'delete)))
                                       (lambda (done undo) (remember! undo #:more? #t))))
                          ((#\!) (remember-state! ids 'unread #f #:more? #t))
                          ((#\?) (remember-state! ids 'unread #t #:more? #t))
                          ((#\+) (remember-state! ids 'starred #t #:more? #t))
                          ((#\-) (remember-state! ids 'starred #f #:more? #t)))
                        (hash-table-delete! marks (car entry))))
                    todo)
          (put! st 'listed #f) (refresh! s)
          (message! s (string-append (number->string (length todo)) (if (= (length todo) 1) " mark" " marks")
                                     " done · a undoes until mbsync runs"))))))
(define-command (mail-toggle-body s n)
  "Toggle the private HTML and literal plain-text views."
  (let* ((st (mail-state s)) (m (selected-message s))
         (parsed (and m (parsed-message s m))))
    (if (and parsed (mail-html parsed) (mail-text parsed))
        (begin (put! st 'body-view
                     (case (get st 'body-view 'auto) ((html) 'plain) ((plain) 'html)
                           (else (if (option #f 'mail-prefer-html) 'plain 'html))))
               (reader! s))
        (message! s "This message has no alternative HTML/plain part"))))
(define-command (mail-toggle-details s n)
  "Show or hide message recipient, identity and routing details."
  (let ((st (mail-state s))) (put! st 'details? (not (get st 'details?))) (reader! s)))
(define-command (mail-toggle-quotes s n)
  "Expand or fold quoted lines in the plain-text message."
  (let ((st (mail-state s))) (put! st 'quotes? (not (get st 'quotes?))) (reader! s)))
(define-mode mail-info-mode
  "Mail help or setup. b or q returns to the mailbox."
  #:parent 'rich-mode #:options '((read-only . #t) (line-numbers . #f) (eob-marker . ""))
  #:keys '(("b" mail-back) ("q" mail-back) ("C-c C-k" mail-cancel-sync)) #:normal '(("b" mail-back) ("q" mail-back)))
(define-mode mail-source-mode
  "Cached MIME source as safe text. b or q returns to summaries."
  #:parent 'special-mode #:keys '(("b" mail-back) ("q" mail-back)) #:normal '(("b" mail-back) ("q" mail-back)))

(define-command (mail-source s n)
  "Show cached MIME source as read-only UTF-8 text.
Invalid UTF-8 bytes display as replacements; original octets remain in SQLite."
  (let* ((m (or (selected-message s) (error "Select a message first")))
         (raw (mail-ref m 'raw))
         (b (make-generated-buffer! "*mail source*" (make-document (safe-text (string-join (string-split (if (bytevector? raw) (mail-decode-utf8 raw) raw) "\r\n") "\n"))) 'mail-source-mode)))
    (show-aside! s b)))

;;; Sync is explicit, asynchronous and read-only. No expunge or remote flag writes.
(define (maildir-account-folders account)
  ;; The (MAILBOX . PATH) folders of a Maildir account. With a mailbox
  ;; named, its path is that one mailbox. Else every Maildir at and under
  ;; the path is one, named by its place there (Inbox, [Gmail]/All Mail):
  ;; those of `folders` if given, and none of `skip`.
  ;; A path that is missing, or holds no Maildir, is an error, not an
  ;; account with no folders: a sync removes from the cache the folders
  ;; an account no longer has, and a disk not mounted or a typo must not
  ;; empty it.
  (let ((root (mail-ref account 'path)) (box (mail-ref account 'mailbox))
        (only (mail-ref account 'folders)) (skip (mail-ref account 'skip '())))
    (unless (string? root) (error "A Maildir account needs a path"))
    (if box (list (cons box root))
        (let ((all (maildir-folders root)))
          (when (null? all) (error "No Maildir at or under the account's path" root))
          (map (lambda (folder) (cons (if (string=? (car folder) "") "INBOX" (car folder)) (cdr folder)))
               (filter (lambda (folder)
                         (and (not (member (car folder) skip))
                              (or (not (pair? only)) (member (car folder) only))))
                       all))))))
(define (issue! id box phase remedy)
  (set! %sync-issues (append %sync-issues (list (list (cons 'account id) (cons 'mailbox box)
                                                      (cons 'phase phase) (cons 'remedy remedy))))))
(define (sync-folder! id box path notify)
  ;; One folder of a Maildir account.
  (let ((skipped 0) (large 0))
    (notify 'maildir box 0)
    (mail-store-import-maildir! (database!) id box path
      #:notify (lambda (n) (notify 'cache box n))
      ;; A message over the size limit is left out by design, every sync
      ;; again: said so, but not a failure of the sync.
      #:on-error (lambda (key e)
                   (if (and (error-object? e) (string-contains (error-object-message e) "exceeds 16 MiB"))
                       (set! large (+ large 1))
                       (set! skipped (+ skipped 1))))
      ;; A message whose file is gone, moved or deleted by mbsync or by
      ;; hand, goes from the cache too, as mu's index forgets it.
      #:remove-missing (lambda (n) (set! %sync-removed (+ %sync-removed n))))
    (when (> large 0) (set! %sync-large (append %sync-large (list (list id box large)))))
    (when (> skipped 0)
      (issue! id box "Read Maildir"
              (string-append (number->string skipped)
                             " unreadable or malformed source(s) skipped. Other messages were indexed; source files are unchanged. Check file permissions, then retry explicitly.")))))
(define (sync-imap! account id notify)
  ;; Every mailbox of an IMAP account, over one connection.
  (notify 'credentials "" 0)
  (let ((password (secret account 'imap)))
    (notify 'connect "" 0)
    (let* ((security (mail-ref account 'imap-security 'implicit-tls))
           (port (mail-ref account 'port (if (eq? security 'starttls) 143 993)))
           (connection (imap-connect (mail-ref account 'host) port (mail-ref account 'user) password #:security security)))
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (for-each
           (lambda (box)
             (notify 'select box 0)
             (let* ((selected (imap-select! connection box)) (validity (mail-ref selected 'uidvalidity))
                    (since (mail-store-prepare-imap! (database!) id box validity)))
               (notify 'fetch box 0)
               (imap-for-each-message connection
                 (lambda (m)
                   (notify 'cache box 0)
                   (mail-store-import! (database!) id box
                                       (string-append (number->string validity) ":" (number->string (mail-ref m 'uid)))
                                       (mail-ref m 'raw) (mail-ref m 'flags '()))
                   (notify 'fetch box 1)) since)))
           (by-rank (mail-ref account 'mailboxes '("INBOX")) (lambda (box) box)))
          ;; A Gmail message fetched from INBOX and from All Mail is one.
          (mail-store-dedupe! (database!) id))
        (lambda () (imap-close connection))))))
(define (account-id account)
  (guard (e ((not (task-cancelled-condition? e)) "invalid account"))
    (let ((value (mail-ref account 'id #f))) (if (string? value) (one-line value) "invalid account"))))
(define (sync-jobs accounts listed)
  ;; What a sync does, in order, as (ACCOUNT ID BOX PATH): a Maildir
  ;; folder, or an IMAP account (BOX and PATH #f), all its mailboxes over
  ;; one connection. Every account's Inbox is read first and All Mail
  ;; last, so that each account's new mail comes in soon, not only after
  ;; the whole of the first. An account that cannot be read is reported.
  ;; LISTED gets the folders of each account whose folders are known.
  (by-rank
   (append-map
    (lambda (account)
      (let ((id (account-id account)))
        (guard (e ((not (task-cancelled-condition? e))
                   (issue! id "" (phase-label 'maildir) (phase-remedy 'maildir)) '()))
          (cond ((not (string? (mail-ref account 'id #f)))
                 (issue! id "" (phase-label 'configuration) "Give the account a string id.") '())
                ((eq? (mail-ref account 'kind) 'maildir)
                 (let ((folders (maildir-account-folders account)))
                   (hash-table-set! listed id (map car folders))
                   (map (lambda (folder) (list account id (car folder) (cdr folder))) folders)))
                ((eq? (mail-ref account 'kind) 'imap)
                 (hash-table-set! listed id (mail-ref account 'mailboxes '("INBOX")))
                 (list (list account id #f #f)))
                (else (issue! id "" (phase-label 'configuration) "Mail account kind must be imap or maildir.") '())))))
    accounts)
   (lambda (job) (or (caddr job) "inbox"))))
(define (reconcile! accounts listed)
  ;; The cache as the configuration says, as mu's index is made as its
  ;; Maildirs are: an account no longer configured goes, and a folder an
  ;; account no longer reads (skip, folders, mailboxes). Only at a sync,
  ;; which needs accounts configured: not before a configuration is
  ;; loaded. An account whose folders could not be listed keeps them, and
  ;; while an account is configured wrongly no other one goes.
  (let* ((db (database!)) (folders (mail-store-folders db))
         (ids (map account-id accounts))
         (whole? (every (lambda (a) (guard (e ((not (task-cancelled-condition? e)) #f)) (string? (mail-ref a 'id #f))))
                        accounts))
         (gone-accounts (if whole?
                            (delete-duplicates (filter-map (lambda (f) (let ((id (mail-ref f 'account))) (and (not (member id ids)) id)))
                                                           folders))
                            '()))
         (gone-folders (filter (lambda (f)
                                 (let ((names (hash-table-ref/default listed (mail-ref f 'account) #f)))
                                   (and names (not (member (mail-ref f 'mailbox) names)))))
                               folders)))
    (for-each (lambda (id)
                (set! %sync-removed (+ %sync-removed (mail-store-forget! db id)))
                (hash-table-delete! %account-status id))
              gone-accounts)
    (for-each (lambda (f)
                (set! %sync-removed (+ %sync-removed (mail-store-forget! db (mail-ref f 'account) #:mailbox (mail-ref f 'mailbox)))))
              gone-folders)))
(define (redisplay-mail! s #:reader? [reader? #t])
  ;; What mail shows made again: the dashboard and the list, if made.
  (let ((st (mail-state s)))
    (when (get st 'dashboard) (dashboard! s))
    (when (get st 'summary) (refresh! s #:reader? reader?))))
(define (phase-label phase)
  (case phase ((credentials) "Credentials") ((connect) "Connect / authenticate") ((select) "Select mailbox")
        ((fetch) "Fetch new messages") ((maildir) "Read Maildir") ((cache) "Cache message") (else "Configuration")))
(define (phase-remedy phase)
  (case phase ((credentials) "Check mail-credential-provider and your secret manager; retry explicitly.")
        ((connect) "Check host, TLS port/certificates and app-password authentication; no insecure fallback exists.")
        ((select) "Check the configured mailbox wire name and account permissions.")
        ((fetch) "Check server connectivity and message limits; g resumes after cached UIDs.")
        ((maildir) "Check that the Maildir path exists and cur/new files are readable; retry after concurrent moves.")
        ((cache) "Check cache permissions/free space and the 16 MiB MIME limit; already-cached messages are safe.")
        (else "Check the account alist in mail-accounts; see doc/mail.org.")))
(define-command (mail-sync s n)
  "Fetch configured IMAP or Maildir accounts into the local cache.
Runs in a task. IMAP uses BODY.PEEK; source flags and messages stay untouched.
The cache is then as `mail-accounts' says: an account no longer
configured, a folder no longer read and a Maildir message whose file is
gone are removed from it, with their local state; drafts stay."
  (cond (%sync-task (message! s "Mail sync is already running"))
        ((not (list? (option #f 'mail-accounts))) (error "mail-accounts must be a list of account alists"))
        ((null? (option #f (quote mail-accounts))) (message! s "No mail accounts. Set mail-accounts; see doc/mail.org."))
        (else
         (database!)
         (set! %sync-imported 0) (set! %sync-removed 0) (set! %sync-issues '()) (set! %sync-large '())
         (set! %sync-status "Syncing… · C-c C-k cancel · e details")
         (let ((accounts (configured-accounts)))
           (for-each (lambda (a) (hash-table-set! %account-status (account-id a) "waiting to fetch")) accounts)
           ;; Asking for mail makes the list again, as g does in a summary.
           (put! (mail-state s) 'listed #f)
           (redisplay-mail! s)
           (set! %sync-task
            (spawn
             (lambda ()
               (let* ((listed (make-hash-table)) (jobs (sync-jobs accounts listed)) (painted 0)
                      (new (make-hash-table)) (remaining (make-hash-table))
                      (issues (lambda (id) (length (filter (lambda (i) (equal? (mail-ref i 'account) id)) %sync-issues))))
                      (done (lambda (id)
                              (let ((n (hash-table-ref/default new id 0))
                                    (large (fold + 0 (map caddr (filter (lambda (l) (equal? (car l) id)) %sync-large)))))
                                (string-append (if (> (issues id) 0) "incomplete · e details · " "synced ")
                                               (clock-label (current-milliseconds))
                                               (if (> n 0) (string-append " · " (grouped n) " new") "")
                                               (if (> large 0) (string-append " · " (grouped large) " too large") ""))))))
                 (for-each (lambda (job) (hash-table-update!/default remaining (cadr job) (lambda (n) (+ n 1)) 0)) jobs)
                 ;; An account with nothing to do, as one that cannot be read, is done.
                 (for-each (lambda (a) (let ((id (account-id a)))
                                         (unless (hash-table-ref/default remaining id #f)
                                           (hash-table-set! %account-status id (done id)))))
                           accounts)
                 (for-each
                  (lambda (job)
                    (let ((account (car job)) (id (cadr job)) (phase 'configuration) (box (or (caddr job) "")))
                      (guard (e ((not (task-cancelled-condition? e)) (issue! id box (phase-label phase) (phase-remedy phase))))
                        (let ((notify
                               (lambda (next-phase mailbox count)
                                 (set! phase next-phase) (set! box (one-line mailbox))
                                 (set! %sync-imported (+ %sync-imported count))
                                 (hash-table-update!/default new id (lambda (n) (+ n count)) 0)
                                 (hash-table-set! %account-status id
                                                  (string-append "fetching " (if (string=? box "") "" (string-append box " "))
                                                                 "· " (grouped (hash-table-ref/default new id 0)) " new"))
                                 (set! %sync-status (string-append id " · " (phase-label phase) " · "
                                                                  (grouped %sync-imported) " cached · C-c C-k cancel"))
                                 (when (> (- (current-milliseconds) painted) 750)
                                   (set! painted (current-milliseconds)) (redisplay-mail! s #:reader? #f)))))
                          (if (cadddr job) (sync-folder! id box (cadddr job) notify) (sync-imap! account id notify))))
                      (hash-table-update!/default remaining id (lambda (n) (- n 1)) 1)
                      (hash-table-set! %account-status id
                                       (if (> (hash-table-ref/default remaining id 0) 0)
                                           (string-append "waiting · " (grouped (hash-table-ref/default new id 0)) " new so far")
                                           (done id)))))
                  jobs)
                 (guard (e ((not (task-cancelled-condition? e))
                            (issue! "cache" "" (phase-label 'cache) (phase-remedy 'cache))))
                   (reconcile! accounts listed)))
               (set! %sync-status (string-append (if (null? %sync-issues) "Up to date" "Sync incomplete")
                                                " · " (grouped %sync-imported) " processed"
                                                (if (> %sync-removed 0)
                                                    (string-append " · " (grouped %sync-removed) " gone from the cache")
                                                    "")
                                                (let ((large (fold + 0 (map caddr %sync-large))))
                                                  (if (> large 0) (string-append " · " (grouped large) " over 16 MiB not cached") ""))
                                                (if (null? %sync-issues) " · sources unchanged" " · e details / recovery")))
               (set! %sync-task #f) (forget-contacts!) (redisplay-mail! s) (message! s %sync-status))))))))

(define-command (mail-sync-status s n)
  "Inspect sanitized mail-sync progress and recovery.
Show account/phase progress and partial success.
Arbitrary server text, secret-provider errors and credentials are never logged."
  (let ((b (make-rich-buffer! "*mail sync*" #:mode 'mail-info-mode)))
    (rich-present! b
     (append (list '(heading 1 "Mail sync") (list 'paragraph %sync-status)
                   (list 'paragraph (number->string %sync-imported) " messages processed. Existing cached mail and drafts remain available."))
             (if %sync-task (list (list 'paragraph (mail-link 'cancel-sync "Cancel sync (C-c C-k)"))) '())
             (append-map (lambda (issue)
                           (list (list 'heading 2 (string-append (mail-ref issue 'account) " / " (mail-ref issue 'mailbox)))
                                 (list 'paragraph (list 'strong (mail-ref issue 'phase)) " failed. " (mail-ref issue 'remedy)))) %sync-issues)
             (if (null? %sync-large) '()
                 (list '(heading 2 "Not cached: larger than 16 MiB")
                       (cons 'paragraph
                             (mail-append-map (lambda (l i)
                                                (append (if (= i 0) '() (list '(hard-break)))
                                                        (list (string-append (car l) " / " (cadr l) ": " (grouped (caddr l))
                                                                             (if (= (caddr l) 1) " message" " messages")))))
                                              %sync-large (iota (length %sync-large))))
                       '(paragraph "A message over 16 MiB, mostly its attachments, is not read: it is not listed or searched here. Its file is unchanged; mu4e or another reader opens it.")))
             (list '(paragraph "No arbitrary server or credential-provider text is retained. Local state is separate from source flags; nothing is expunged, moved or deleted on the source.")
                   (list 'paragraph (mail-link 'back "← Back")))))
    (show-aside! s b)))

(define-command (mail-cancel-sync s n)
  "Cancel the current mail sync; already-cached messages stay usable."
  (if %sync-task
      (begin (task-cancel %sync-task) (set! %sync-task #f)
             (set! %sync-status "Sync cancelled · cached messages kept")
             (for-each (lambda (id) (let ((status (hash-table-ref/default %account-status id "")))
                                      (when (or (string-prefix? "fetching" status) (string-prefix? "waiting" status))
                                        (hash-table-set! %account-status id "cancelled · g fetches again"))))
                       (hash-table-keys %account-status))
             (redisplay-mail! s) (message! s "Mail sync cancelled"))
      (message! s "No mail sync is running")))

;;; Composition: ordinary editable buffers with durable drafts and explicit send.
;;; Who you are and whom you write to, learnt from the mail cached, as
;;; mu4e learns its contacts from mu's index.
(define %contacts #f)
(define (own-senders account)
  ;; (FROM COUNT LAST) of what ACCOUNT, or every account, sent: the From
  ;; headers of its sent folders, the most used first.
  (mail-store-senders (database!) #:account account #:sent? #t))
(define (mailboxes-of header)
  ;; HEADER's mailboxes, or none when it does not parse.
  (guard (e ((not (task-cancelled-condition? e)) '())) (mail-parse-addresses header)))
(define (mailbox-text name address)
  ;; Name <address>, the name quoted only when it must be.
  (cond ((string=? name "") address)
        ((any (lambda (c) (memv c '(#\, #\; #\: #\< #\> #\@ #\" #\( #\) #\[ #\] #\\))) (string->list name))
         (string-append "\"" (apply string-append (map (lambda (c) (if (memv c '(#\\ #\")) (string #\\ c) (string c))) (string->list name)))
                        "\" <" address ">"))
        (else (string-append name " <" address ">"))))
(define (own-addresses account)
  ;; The addresses ACCOUNT, or every account, sends from, lower case.
  (delete-duplicates
   (append (filter-map (lambda (a) (let ((from (mail-ref a 'from)))
                                     (and (string? from) (or (not account) (equal? (mail-ref a 'id) account))
                                          (let ((boxes (mailboxes-of from)))
                                            (and (pair? boxes) (string-downcase (mail-ref (car boxes) 'address)))))))
                       (configured-accounts))
           (append-map (lambda (r) (map (lambda (b) (string-downcase (mail-ref b 'address))) (mailboxes-of (car r))))
                       (own-senders account)))))
(define (default-from account #:to [to ""])
  ;; ACCOUNT's from; else the From it sends with that TO, the mail
  ;; answered, was addressed to; else the one it sends with most.
  (or (mail-ref account 'from)
      (let* ((sent (filter-map (lambda (r) (let ((boxes (mailboxes-of (car r))))
                                             (and (= (length boxes) 1) (car boxes))))
                               (own-senders (mail-ref account 'id))))
             (addressed (map (lambda (b) (string-downcase (mail-ref b 'address))) (mailboxes-of to)))
             (box (or (find (lambda (b) (member (string-downcase (mail-ref b 'address)) addressed)) sent)
                      (and (pair? sent) (car sent)))))
        (and box (mailbox-text (mail-ref box 'name "") (mail-ref box 'address))))
      (mail-ref account 'user "")))
(define (automated? address)
  (any (lambda (part) (string-contains address part))
       '("noreply" "no-reply" "donotreply" "do-not-reply" "notifications@" "mailer-daemon@" "bounce")))
(define (contacts-now)
  ;; As mu4e completes them: people of personal mail, not lists or
  ;; notifications. First those you wrote to, then those who wrote to
  ;; you; within each the most, then the newest first.
  (let* ((db (database!)) (sent (own-senders #f))
         (own (own-addresses #f)) (table (make-hash-table)))
    (define (add! header count last field)
      (for-each (lambda (box)
                  (let* ((address (string-downcase (mail-ref box 'address))) (name (mail-ref box 'name ""))
                         (old (or (hash-table-ref/default table address #f)
                                  (let ((new (vector name (mail-ref box 'address) 0 0 0))) (hash-table-set! table address new) new))))
                    (unless (or (member address own) (automated? address))
                      (when (string=? (vector-ref old 0) "") (vector-set! old 0 name))
                      (vector-set! old field (+ (vector-ref old field) count))
                      (vector-set! old 4 (max (vector-ref old 4) last)))))
                (mailboxes-of header)))
    ;; Fields: name, address, written to, written by, newest.
    (for-each (lambda (r) (add! (car r) (cadr r) (caddr r) 2)) (mail-store-recipients db (map car sent)))
    (for-each (lambda (r) (add! (car r) (cadr r) (caddr r) 3)) (mail-store-senders db #:to own))
    (map (lambda (v) (candidate (mailbox-text (vector-ref v 0) (vector-ref v 1))
                                #:annotation (if (> (vector-ref v 2) 0) "you wrote to" "wrote to you")))
         (sort (filter (lambda (v) (> (+ (vector-ref v 2) (vector-ref v 3)) 0)) (hash-table-values table))
               (lambda (a b)
                 (cond ((not (eq? (> (vector-ref a 2) 0) (> (vector-ref b 2) 0))) (> (vector-ref a 2) 0))
                       ((not (= (vector-ref a 2) (vector-ref b 2))) (> (vector-ref a 2) (vector-ref b 2)))
                       ((not (= (vector-ref a 3) (vector-ref b 3))) (> (vector-ref a 3) (vector-ref b 3)))
                       (else (> (vector-ref a 4) (vector-ref b 4)))))))))
;; Learnt in a task when a draft opens, so that they are there by the
;; time a name is typed; again after a sync or a message sent.
(define %contacts-task #f)
(define (learn-contacts!)
  (unless (or %contacts %contacts-task)
    (set! %contacts-task
          (spawn (lambda ()
                   (guard (e (#t (set! %contacts-task #f) (raise e)))
                     (set! %contacts (contacts-now))
                     (set! %contacts-task #f)))))))
(define (forget-contacts!) (set! %contacts #f) (set! %contacts-task #f))
(define (contacts)
  (or %contacts
      (begin (learn-contacts!)
             (let ((t %contacts-task)) (when t (task-join t)))
             (or %contacts '()))))
;; What completion takes as a word, as `completion-at-point' does: an
;; address or a part of a name, up to a space or a comma.
(define (word-char? c) (not (or (char-whitespace? c) (memv c '(#\( #\) #\[ #\] #\{ #\} #\' #\" #\` #\, #\;)))))
(define (word-start d pos)
  (let loop ((p pos))
    (if (and (> p 0) (word-char? (string-ref (document-substring d (- p 1) p) 0))) (loop (- p 1)) p)))
(define (address-field? d pos)
  ;; Whether POS is in the To, Cc or Bcc header of a draft, or on a line
  ;; continuing one: above the first empty line.
  (define (line-at p) (document-substring d (line-start d p) (line-end d p)))
  (define (in-headers? p)
    (or (= (line-start d p) 0)
        (let ((above (- (line-start d p) 1)))
          (and (not (string=? (line-at above) "")) (in-headers? above)))))
  (let loop ((p pos))
    (let ((text (line-at p)))
      (cond ((string=? (string-trim text) "") #f)
            ((memv (string-ref text 0) '(#\space #\tab))
             (and (> (line-start d p) 0) (loop (- (line-start d p) 1))))
            (else (and (any (lambda (h) (string-prefix? h (string-downcase text))) '("to:" "cc:" "bcc:"))
                       (in-headers? p)))))))

(define-mode mail-compose-mode
  "Compose a plain-text email, with automatic local draft saving.
\\[mail-save-draft] saves now; \\[mail-send] confirms and sends through native
SMTP over TLS. \\[mail-back] returns without discarding.
Headers end at the first blank line. Bcc is used only in the SMTP envelope.
In To, Cc and Bcc, the people you write to and who write to you
complete, after two characters or with \\[completion-at-point]."
  #:parent 'text-mode #:options '((line-numbers . #f) (eob-marker . ""))
  #:complete (lambda (b pos)
               (let ((d (buffer-document b)))
                 (and (address-field? d pos) (cons (word-start d pos) (contacts)))))
  #:keys '(("C-c C-d" mail-postpone) ("C-x C-s" mail-save-draft) ("C-c C-c" mail-send) ("C-c C-r" mail-resolve-delivery) ("C-c C-k" mail-discard-draft)
           ("C-M-i" completion-at-point) ("M-TAB" completion-at-point)))
(define (compose! s account text #:id [id (uuid-v4)] #:reply [reply ""] #:references [references ""] #:delivery-state [delivery-state "draft"])
  (let ((st (make-hash-table)))
    (put! st 'account account) (put! st 'draft-id id) (put! st 'reply reply) (put! st 'references references)
    (put! st 'uncertain? (and (member delivery-state '("sending" "uncertain")) #t))
    (let* ((b (make-generated-buffer! (string-append "*mail draft " (substring id 0 8) "*") (make-document text) 'mail-compose-mode #:state st))
           (db (database!)) (d (buffer-document b)))
      (put! st 'db db)
      (when (get st 'uncertain?) (set-option! 'read-only #t #:buffer b))
      (persist-draft! b)
      (put! st 'autosave
            (let ((initial-revision (get st 'saved-revision (document-revision d))))
              (spawn (lambda ()
                       (let loop ((revision initial-revision) (debounce? #t))
                         (when (and (memq b (buffer-list))
                                    (or (not (= revision (document-revision d)))
                                        (document-await-change d revision)))
                           (when debounce? (sleep 400))
                           (when (memq b (buffer-list))
                             (let ((saved (guard (e (#t (put! st 'save-error? #t) #f)) (persist-draft! b))))
                               ;; A write acknowledges its captured revision, not edits
                               ;; arriving during SQLite I/O. Drain newer edits now.
                               (if saved (loop saved (= saved (document-revision d)))
                                   (loop (document-revision d) #t))))))))))
      (note-origin! s)
      (learn-contacts!)
      (focus-mail-buffer! s b 1)
      (let* ((crlf (string-contains text "\r\n\r\n")) (lf (string-contains text "\n\n"))
             (body (cond (crlf (+ crlf 4)) (lf (+ lf 2)) (else (string-length text)))))
        (move! s (lambda (p) (bytevector-length (string->utf8 (substring text 0 body))))))
      (message! s (if (get st 'uncertain?) "Delivery uncertain · read-only inspection · check server delivery, then C-c C-r to resolve"
                      "Draft autosaves locally · C-c C-c send · C-c C-d later · C-c C-k discard")))))
(define (context-account s)
  ;; The account of what mail shows, as mu4e's context follows the
  ;; message: the dashboard row's, the list's, else the message read or
  ;; selected; #f across accounts with nothing selected.
  (let* ((st (mail-state s)) (b (current-buffer s)) (mode (and b (buffer-mode b)))
         (id (cond ((eq? mode 'mail-dashboard-mode)
                    (let ((t (dashboard-target s))) (and t (eq? (target-kind t) 'mail-view) (cadr (target-value t)))))
                   ((memq mode '(mail-summary-mode mail-message-mode))
                    (or (get st 'account)
                        (let ((m (let ((id (if (eq? mode 'mail-message-mode) (get st 'reader-id) (or (row-id s) (get st 'selected)))))
                                   (and id (mail-store-message (database!) id #:raw? #f)))))
                          (and m (mail-ref m 'account)))))
                   (else #f))))
    (and id (account-by-id id))))
(define (choose-account s accept)
  ;; Asked whenever there is a choice; the account of what mail shows is
  ;; first, so RET takes it.
  (let* ((accounts (configured-accounts)) (here (context-account s))
         (ordered (if here (cons here (remove (lambda (a) (eq? a here)) accounts)) accounts)))
    (cond ((null? accounts) (error "Set mail-accounts in your Scheme configuration first"))
          ((null? (cdr accounts)) (accept (car accounts)))
          (else (completing-read s "From account: "
                                 (map (lambda (a) (candidate (mail-ref a 'id) #:annotation (default-from a)
                                                             #:target (target 'mail-account a)))
                                      ordered)
                                 #:accept (lambda (s c) (accept (target-value (candidate-target c)))))))))
(define-command (mail-compose s n)
  "Compose a new message, starting in the recipient field.
The account is that of the group, folder or message mail shows, else
chosen; From is its from, else the address it sends from most."
  (choose-account s (lambda (a)
                      (compose! s a (string-append "From: " (default-from a) "\nTo: \nCc: \nBcc: \nSubject: \n\n"))
                      (move! s (lambda (p) (line-end (doc s) (line-down (doc s) 0 1 0)))))))
(define-command (mail-reply s n)
  "Compose a reply to the current message.
Preserve its Message-ID threading headers."
  (let* ((m (or (mail-store-message (database!) (current-id s)) (error "Select a message first"))) (parsed (mail-parse (mail-ref m 'raw)))
         (a (or (account-by-id (mail-ref m 'account)) (error "This message's account is no longer configured")))
         (subject (mail-ref m 'subject)) (parent (mail-header parsed "message-id"))
         (text (or (mail-text parsed) "[HTML message; open the original to read it.]")))
    (compose! s a
              (string-append "From: " (default-from a #:to (string-append (mail-header-raw parsed "to") ", " (mail-header-raw parsed "cc")))
                             "\nTo: "
                             (one-line (mail-display-addresses (mail-header-raw parsed "reply-to" (mail-header-raw parsed "from"))))
                             "\nCc: \nBcc: \nSubject: " (if (string-prefix? "re:" (string-downcase subject)) "" "Re: ") (one-line subject)
                             "\n\n\n\n" (one-line (mail-ref m 'sender)) " wrote:\n"
                             (string-join (map (lambda (l) (string-append "> " l)) (string-split text "\n")) "\n"))
              #:reply parent #:references (string-trim (string-append (mail-header parsed "references") " " parent)))))
(define (draft-buffer s)
  (let ((b (current-buffer s))) (unless (and b (eq? (buffer-mode b) 'mail-compose-mode)) (error "Not a mail draft")) b))
(define (draft-text b)
  (let* ((st (buffer-state b)) (text (document-string (buffer-document b))) (parsed (mail-parse text)))
    (string-append
     (if (or (string=? (get st 'reply "") "") (not (string=? (mail-header parsed "in-reply-to") ""))) ""
         (string-append "In-Reply-To: " (get st 'reply) "\n"))
     (if (or (string=? (get st 'references "") "") (not (string=? (mail-header parsed "references") ""))) ""
         (string-append "References: " (get st 'references) "\n")) text)))
(define (persist-draft! b)
  (let ((st (buffer-state b)))
    (if (or (get st 'sent?) (get st 'sending?) (get st 'uncertain?)) #f
        (let* ((revision (document-revision (buffer-document b))) (text (draft-text b)))
          (put! st 'saving-revision revision)
          (guard (e (#t (put! st 'saving-revision #f) (put! st 'save-error? #t) (raise e)))
            (mail-store-draft! (get st 'db) (get st 'draft-id) (mail-ref (get st 'account) 'id) text))
          (put! st 'saving-revision #f) (put! st 'save-error? #f)
          (put! st 'saved-revision revision)
          ;; Manual saves can overlap the follower too. Every writer drains a
          ;; newer revision before returning, so an older queued write cannot
          ;; remain the final durable text after a newer one.
          (if (= revision (document-revision (buffer-document b))) revision
              (persist-draft! b))))))
(add-hook! 'buffer-killed 'mail-drafts
           (lambda (s b)
             (when (eq? (buffer-mode b) 'mail-compose-mode)
               (let ((st (buffer-state b)))
                 (when (get st 'autosave) (task-cancel (get st 'autosave)))
                 ;; A draft discarded is not saved again on its way out.
                 (unless (get st 'discarded?)
                   (guard (e (#t (message! s "Draft save failed; check your local cache"))) (persist-draft! b)))))))
(define-command (mail-postpone s n)
  "Save this draft and leave it for later, as message mode's postponing.
\\[mail-drafts] resumes it."
  (mail-back s 1))
(define (draft-empty? b)
  ;; Nothing written yet: no recipient, subject or text.
  (let ((parsed (mail-parse (document-string (buffer-document b)))))
    (and (every (lambda (h) (string=? (string-trim (mail-header parsed h)) "")) '("to" "cc" "bcc" "subject"))
         (string=? (string-trim (or (mail-text parsed) "")) ""))))
(define (forget-draft! b)
  ;; The draft gone from the cache, its buffer no longer saving it.
  (let ((st (buffer-state b)))
    (when (or (get st 'sending?) (get st 'uncertain?))
      (error "Its delivery is not settled; resolve it with C-c C-r first"))
    (put! st 'discarded? #t)
    (when (get st 'autosave) (task-cancel (get st 'autosave)))
    (mail-store-delete-draft! (get st 'db) (get st 'draft-id))))
(define (discard-draft! s b)
  ;; The draft gone from the cache and its buffer killed, unsaved.
  (forget-draft! b)
  (drop-buffer! s b))
(define-command (mail-discard-draft s n)
  "Discard this draft, unsaved, and go back, as message mode's kill.
A draft with something in it asks first; one being sent cannot go."
  (let ((b (draft-buffer s)))
    (define (discard! s)
      (forget-draft! b) (return-from-aside! s) (drop-buffer! s b) (message! s "Draft discarded"))
    (if (draft-empty? b) (discard! s)
        (completing-read s "Discard this draft? " '("no" "yes")
                         #:accept (lambda (s c)
                                    (if (string=? (candidate-text c) "yes") (discard! s) (message! s "Draft kept")))))))
(define-command (mail-save-draft s n)
  "Save this draft in the local SQLite cache, including threading headers."
  (let* ((b (draft-buffer s)) (st (buffer-state b))) (persist-draft! b)
    (message! s (if (or (get st 'sent?) (get st 'sending?) (get st 'uncertain?))
                    "Delivery state protected · no draft state was reset · C-c C-r resolves uncertainty"
                    "Draft saved locally · M-x mail-drafts to resume"))))
;;; Drafts are a mailbox as the others are: a list of them, RET opening
;;; one below it, as a message opens below its list; d flags one to be
;;; deleted and x deletes those flagged, as dired and mu4e do.
(define-mode mail-drafts-mode
  "The saved drafts, listed as a mailbox's messages are.
RET opens the selected one below the list, to go on writing it. d flags
a draft to be deleted, u unflags it, x deletes those flagged. C composes
another; q goes back to the dashboard. Nothing is sent from here."
  #:parent 'listing-mode
  #:layer (lambda (d from to) (drafts-layer d))
  #:options '((line-numbers . #f) (eob-marker . "") (cursor-visible . #f))
  #:keys '(("n" mail-drafts-next) ("p" mail-drafts-previous) ("j" mail-drafts-next) ("k" mail-drafts-previous)
           ("C-n" mail-drafts-next) ("C-p" mail-drafts-previous) ("<down>" mail-drafts-next) ("<up>" mail-drafts-previous)
           ("RET" mail-drafts-open) ("x" mail-drafts-execute) ("C" mail-compose) ("g" mail-drafts) ("q" mail-dashboard))
  #:normal '(("j" mail-drafts-next) ("k" mail-drafts-previous) ("n" mail-drafts-next) ("p" mail-drafts-previous)
             ("C-n" mail-drafts-next) ("C-p" mail-drafts-previous) ("<down>" mail-drafts-next) ("<up>" mail-drafts-previous)
             ("RET" mail-drafts-open) ("x" mail-drafts-execute) ("C" mail-compose) ("g r" mail-drafts) ("q" mail-dashboard)))

(define (draft-label ms)
  ;; When a draft was last written: a clock today, else a date; blank
  ;; for one saved before drafts said when.
  (if (= ms 0) ""
      (let* ((zone (guard (e ((not (task-cancelled-condition? e)) "UTC")) (local-zone)))
             (c (instant->civil ms zone)) (today (instant->civil (current-milliseconds) zone)))
        (if (and (= (car c) (car today)) (= (cadr c) (cadr today)) (= (caddr c) (caddr today)))
            (clock-label ms)
            (string-append (list-ref '("Jan" "Feb" "Mar" "Apr" "May" "Jun" "Jul" "Aug" "Sep" "Oct" "Nov" "Dec") (- (cadr c) 1))
                           " " (number->string (caddr c)))))))
(define (drafts-rows b)
  (let* ((st (listing-get b 'mail-state)) (drafts (get st 'draft-list '()))
         (columns (get st 'columns 160)) (width (max 40 (- columns 6))))
    (put! st 'draft-keys (map car drafts))
    (append
     (list (row #:key "header" (list (list (string-append "Drafts · " (grouped (length drafts)) (if (= (length drafts) 1) " draft" " drafts"))
                                           'mail.section)))
           (row #:key "header-gap" ""))
     (if (null? drafts)
         (list (row #:key "empty" (list (list "No drafts." 'mail.unread)))
               (row #:key "empty-2" "C composes a message; q goes back to the dashboard."))
         (map (lambda (d)
                (let* ((parsed (mail-parse (caddr d)))
                       (to (one-line (mail-header parsed "to"))) (subject (one-line (mail-header parsed "subject")))
                       (state (list-ref d 3))
                       (gone? (not (account-by-id (cadr d)))))
                  (listing-row b (car d) (target 'mail-draft d)
                               (list "  " (list (mail-pad (draft-label (list-ref d 4)) 10) 'mail.preview) " "
                                     (list (mail-pad (if (string=? to "") "(no recipient)" to) 24) 'mail.preview) " "
                                     (list (mail-column (if (string=? subject "") "(no subject)" subject) (max 10 (- width 60))) 'mail.unread)
                                     (list (string-append "   " (cadr d) (if gone? " (not configured)" "")
                                                          (if (string=? state "draft") "" (string-append " · " state)))
                                           (if (string=? state "draft") 'mail.preview 'warning))))))
              drafts)))))
(define (drafts-layer d)
  (let* ((b (document-buffer d)) (st (and b (listing-get b 'mail-state))) (key (and st (get st 'draft-key)))
         (span (and key (presentation-row-span d key))))
    (if span (list (list (car span) (min (document-length d) (+ 1 (cadr span))) 'mail.selected)) '())))
(define (drafts! s)
  ;; The drafts' list made again, from what the cache holds now.
  (let ((st (mail-state s)))
    (put! st 'columns (or (sget s 'viewport-columns) 160))
    (put! st 'draft-list (mail-store-drafts (database!)))
    (let ((b (make-listing-buffer! "*mail drafts*" 'mail-drafts-mode drafts-rows #:state (list (cons 'mail-state st)))))
      (put! st 'drafts-buffer b)
      (put! st 'drafts (length (get st 'draft-list)))
      (unless (member (get st 'draft-key) (get st 'draft-keys '()))
        (put! st 'draft-key (let ((keys (get st 'draft-keys '()))) (and (pair? keys) (car keys)))))
      b)))
(define (show-drafts! s)
  ;; The drafts' list alone in the frame, as a mailbox's list opens.
  (let ((st (mail-state s)))
    (unless (get st 'before)
      (put! st 'before (list (session-panes s) (session-tree s) (session-focus s))))
    (let* ((b (drafts! s))
           (v (or (let ((v (get st 'drafts-view))) (and v (document=? (view-document v) (buffer-document b)) v))
                  (make-view (buffer-document b) "user"))))
      (put! st 'drafts-view v)
      (set-session-panes! s (list v) 0)
      (when (get st 'draft-key) (listing-goto! s b (get st 'draft-key))))))
(define (drafts-step! s n)
  (let* ((st (mail-state s)) (b (get st 'drafts-buffer)) (keys (get st 'draft-keys '()))
         (at (or (list-index (lambda (k) (equal? k (get st 'draft-key))) keys) 0))
         (next (+ at n)))
    (cond ((null? keys) (message! s "No drafts"))
          ((or (< next 0) (>= next (length keys))) (message! s "No more drafts"))
          (else (put! st 'draft-key (list-ref keys next))
                (listing-goto! s b (list-ref keys next))))))
(define-command (mail-drafts s n)
  "List the saved drafts, as a mailbox: RET opens one, d and x delete.
An uncertain delivery is never retried automatically."
  (database!) (show-drafts! s))
(define-command (mail-drafts-next s n) "Select the next draft." (drafts-step! s n))
(define-command (mail-drafts-previous s n) "Select the previous draft." (drafts-step! s (- n)))
(define (selected-draft s)
  (let* ((st (mail-state s)) (b (get st 'drafts-buffer)) (key (get st 'draft-key))
         (r (and b key (hash-table-ref/default (listing-get b 'rows) key #f))))
    (and r (row-target r) (target-value (row-target r)))))
(define-command (mail-drafts-open s n)
  "Open the selected draft below the list, to go on writing it."
  (let ((d (or (selected-draft s) (error "No draft here"))))
    (mail-resume-draft s d)))
(define-command (mail-drafts-execute s n)
  "Delete the drafts flagged with d, after asking."
  (let* ((st (mail-state s)) (b (get st 'drafts-buffer))
         (flagged (map target-value (listing-marked b #\D))))
    (if (null? flagged) (message! s "Nothing is flagged · d flags a draft to delete")
        (completing-read s (string-append "Delete " (number->string (length flagged)) (if (= (length flagged) 1) " draft? " " drafts? "))
                         '("no" "yes")
                         #:accept (lambda (s c)
                                    (if (not (string=? (candidate-text c) "yes")) (message! s "Nothing deleted")
                                        (let ((done (filter (lambda (d) (guard (e ((not (task-cancelled-condition? e)) #f))
                                                                          (delete-draft! s d) #t))
                                                            flagged)))
                                          (show-drafts! s)
                                          (message! s (string-append (number->string (length done)) " deleted"
                                                                     (if (< (length done) (length flagged))
                                                                         " · one whose delivery is not settled stays" ""))))))))))
(add-hook! 'operation-ended 'mail-drafts-selection
  (lambda (s op)
    (let ((b (current-buffer s)) (st (sget s 'mail)))
      (when (and st b (eq? (buffer-mode b) 'mail-drafts-mode) (not (sget s 'transient)))
        (let ((at (listing-key-at b (point s))))
          (cond ((and at (member at (get st 'draft-keys '()))) (put! st 'draft-key at))
                ((get st 'draft-key) (listing-goto! s b (get st 'draft-key)))))))))
(define (beside-list! s list-view b)
  ;; B opened below or beside LIST-VIEW, as a message below its list.
  (let ((v (or (find (lambda (v) (document=? (view-document v) (buffer-document b))) (session-panes s))
               (make-view (buffer-document b) "user"))))
    (unless (mail-single-pane? s)
      (set-session-panes! s (list list-view v) 1)
      (set-session-tree! s (if (eq? (option #f 'mail-reader-position) 'bottom)
                              '(split below ((0.4 . 0) (0.6 . 1)))
                              '(split right ((0.38 . 0) (0.62 . 1))))))))
(define-action mail-draft (mail-resume-draft s d)
  "Open this saved draft to go on writing it.
A draft of an account no longer configured is sent from one that is."
  (let* ((parsed (mail-parse (caddr d)))
         (old (buffer-named (string-append "*mail draft " (substring (car d) 0 8) "*")))
         (from-drafts? (mail-mode? (current-buffer s) '(mail-drafts-mode)))
         (list-view (and from-drafts? (get (mail-state s) 'drafts-view))))
    (define (open! a)
      (if old (begin (note-origin! s) (show-buffer! s old))
          (compose! s a (with-from (caddr d) parsed a) #:id (car d) #:reply (mail-header parsed "in-reply-to")
                    #:references (mail-header parsed "references") #:delivery-state (list-ref d 3)))
      (when list-view (beside-list! s list-view (or old (current-buffer s)))))
    (let ((a (account-by-id (cadr d))))
      (if a (open! a)
          (choose-account s (lambda (a)
                              (open! a)
                              (message! s (string-append "Its account " (cadr d) " is not configured · sent from " (mail-ref a 'id)))))))))
(define (with-from text parsed account)
  ;; TEXT with ACCOUNT's From when its own is empty, as a draft of an
  ;; account no longer configured may have it.
  (let ((nl (string-index text #\newline)))
    (if (and (string=? (string-trim (mail-header parsed "from")) "") nl
             (string-prefix? "from:" (string-downcase (substring text 0 nl))))
        (string-append "From: " (default-from account) (substring text nl (string-length text)))
        text)))
(define (delete-draft! s d)
  (when (member (list-ref d 3) '("sending" "uncertain"))
    (error "Its delivery is not settled; open it and resolve it with C-c C-r first"))
  (let ((old (buffer-named (string-append "*mail draft " (substring (car d) 0 8) "*"))))
    (if old (discard-draft! s old) (mail-store-delete-draft! (database!) (car d)))))
(define-action mail-draft (mail-delete-draft s d)
  "Delete this saved draft, and close it if it is open."
  (delete-draft! s d)
  (when (mail-mode? (current-buffer s) '(mail-drafts-mode)) (show-drafts! s))
  (message! s "Draft deleted"))
(define-command (mail-resolve-delivery s n)
  "Resolve an uncertain delivery after checking the server.
Inspect the saved message read-only first. Explicitly confirm delivered or not
delivered; returning to draft never sends anything automatically."
  (let* ((b (draft-buffer s)) (st (buffer-state b)) (db (get st 'db)) (id (get st 'draft-id))
         (state (mail-store-draft-state db id)))
    (unless (member state '("sending" "uncertain")) (error "This draft has no ambiguous delivery to resolve"))
    (when (get st 'sending?) (error "The live submission is still running"))
    (completing-read s "After checking server delivery: "
                     (list (candidate "Keep uncertain" #:annotation "Safest if outcome is unknown; no retry")
                           (candidate "Confirmed delivered — mark sent" #:annotation "Positive delivery evidence; keep it sent")
                           (candidate "Confirmed NOT delivered — edit draft" #:annotation "Missing Sent copy alone is NOT proof of non-delivery"))
                     #:accept (lambda (s c)
                                (let ((choice (candidate-text c)))
                                  (cond ((string-prefix? "Confirmed delivered" choice)
                                         (mail-store-resolve-draft! db id "sent") (put! st 'uncertain? #f) (put! st 'sent? #t)
                                         (set-option! 'read-only #t #:buffer b) (message! s "Delivery recorded as sent; it will not be retried"))
                                        ((string-prefix? "Confirmed NOT delivered" choice)
                                         (mail-store-resolve-draft! db id "draft") (put! st 'uncertain? #f) (put! st 'sent? #f)
                                         (put! st 'saved-revision (document-revision (buffer-document b)))
                                         (set-option! 'read-only #f #:buffer b) (message! s "Draft editable again · review and confirm sending separately"))
                                        (else (message! s "Uncertainty retained · no mail sent"))))))))

(define-command (mail-send s n)
  "Confirm and submit this draft with native SMTP over verified TLS.
The draft is saved first. An ambiguous failure is marked uncertain and never
automatically retried; check the server's Sent mail before sending again."
  (let* ((b (draft-buffer s)) (st (buffer-state b)) (a (get st 'account))
         (db (database!)) (text (document-string (buffer-document b))) (saved-text (draft-text b))
         (parsed (mail-parse text))
         ;; Keep mailbox syntax intact until the shared address tokenizer runs.
         (from (mail-header-raw parsed "from")) (to (mail-header-raw parsed "to")) (cc (mail-header-raw parsed "cc"))
         (recipients (delete-duplicates (append (mail-addresses to) (mail-addresses cc) (mail-addresses (mail-header-raw parsed "bcc")))))
         (subject (mail-header parsed "subject")) (id (get st 'draft-id)))
    (when (get st 'sending?) (error "This draft is already being sent"))
    (when (get st 'sent?) (error "This message was already sent"))
    (when (get st 'uncertain?) (error "Delivery is uncertain; inspect it and use C-c C-r after checking server delivery"))
    (unless (equal? (mail-store-draft-state db id) "draft") (error "Durable delivery state blocks another send; inspect saved drafts"))
    (unless (and (pair? recipients) (not (string=? subject ""))) (error "Provide recipients and a subject first"))
    (unless (mail-ref a 'smtp-host) (error "Set smtp-host for this account in mail-accounts"))
    (let ((raw (mail-compose-message from to subject (or (mail-text parsed) "") #:cc cc
                                     #:reply-to (get st 'reply (mail-header parsed "in-reply-to"))
                                     #:references (get st 'references (mail-header parsed "references")))))
      (mail-save-draft s 1)
      (completing-read s (string-append "Send to " (string-join recipients ", ") "? ") '("no" "yes")
                       #:accept
                       (lambda (s choice)
                         (if (not (string=? (candidate-text choice) "yes")) (message! s "Not sent · draft saved")
                             (begin
                               (unless (string=? text (document-string (buffer-document b))) (error "Draft changed; review and confirm it again"))
                               (when (or (get st 'sending?) (get st 'sent?) (get st 'uncertain?)) (error "This draft has already been submitted"))
                               (put! st 'sending? #t) (set! %deliveries (+ %deliveries 1))
                               (set-option! 'read-only #t #:buffer b)
                               (spawn
                                (lambda ()
                                  (let ((claimed? #f) (data-started? #f))
                                    (guard (e (#t (put! st 'sending? #f) (put! st 'uncertain? data-started?)
                                                   (set! %deliveries (- %deliveries 1))
                                                   (set-option! 'read-only data-started? #:buffer b)
                                                   (when claimed?
                                                     (if data-started?
                                                         (mail-store-draft! db id (mail-ref a 'id) saved-text #:state "uncertain")
                                                         (mail-store-resolve-draft! db id "draft")))
                                                   (message! s (if data-started?
                                                                   "Delivery uncertain · inspect draft · check server delivery, then C-c C-r"
                                                                   "Not submitted · draft kept · check SMTP host/credentials, then retry explicitly"))))
                                      (let* ((password (secret a 'smtp))
                                             (security (mail-ref a 'smtp-security 'implicit-tls))
                                             (port (mail-ref a 'smtp-port (if (eq? security 'starttls) 587 465))))
                                        (unless (mail-store-claim-draft! db id saved-text) (error "Draft was already claimed or changed"))
                                        (set! claimed? #t)
                                        (mail-submit! (mail-ref a 'smtp-host) port (mail-ref a 'smtp-user (mail-ref a 'user ""))
                                                      password from recipients raw #:security security #:on-data (lambda () (set! data-started? #t))))
                                      (mail-store-draft! db id (mail-ref a 'id) saved-text #:state "sent")
                                      (forget-contacts!)
                                      (put! st 'sending? #f) (put! st 'sent? #t) (set! %deliveries (- %deliveries 1))
                                      (message! s "Message accepted by your SMTP server · no automatic retry"))))))))))))

;;; Clickable reader controls share exactly the keyboard commands.
(define-action mail-control (mail-use-control s control)
  "Use this mail reader or workspace control."
  (let ((action (car control)) (args (cdr control)))
    (when (and (pair? args) (number? (car args)))
      (put! (mail-state s) 'selected (car args)) (put! (mail-state s) 'reader-id (car args)))
    (case action
      ((reply) (mail-reply s 1)) ((star) (mail-star s 1)) ((archive) (mail-archive s 1))
      ((delete) (dispose-now! s 'delete))
      ((thread) (remember-state! (list (car args)) 'unread #f) (refresh! s)
                (move! s (lambda (p) 0)) (view-set-scroll! (session-view s) 0))
      ((source) (mail-source s 1)) ((back) (mail-back s 1)) ((groups) (mail-groups s 1))
      ((all) (open-view! s '(all #f #f))) ((dashboard) (mail-dashboard s 1))
      ((sync-status) (mail-sync-status s 1)) ((cancel-sync) (mail-cancel-sync s 1))
      ((details) (mail-toggle-details s 1)) ((quotes) (mail-toggle-quotes s 1))
      ((search) (mail-search s 1)) ((compose) (mail-compose s 1)) ((drafts) (mail-drafts s 1))
      ((sync) (mail-sync s 1)) ((quit) (mail-quit s 1))
      ((attachment)
       (let* ((m (selected-message s)) (attachments (mail-attachments (mail-parse (mail-ref m 'raw))))
              (a (list-ref attachments (cadr args)))
              (name (file-name (mail-ref a 'name "attachment"))))
         (read-file-name s "Save attachment (never executed): " (string-append (default-directory s) name)
                         (lambda (s path)
                           (when (file-exists? path) (error "Refusing to overwrite an existing file"))
                           (call-with-port (open-binary-output-file path) (lambda (p) (write-bytevector (mail-ref a 'bytes) p)))
                           (message! s (string-append "Saved attachment: " path)))))))))
