;;; Mail policy and persistence are Scheme; SQLite is only the storage mechanism.
(define-library (techne mail store)
  (export mail-store-open mail-store-close mail-store-import! mail-store-import-batch! mail-store-import-maildir! mail-store-source-keys mail-store-query
          mail-store-message mail-store-source-cached? mail-store-state! mail-store-counts mail-store-count mail-store-folders
          mail-store-dedupe! mail-store-copy-mailboxes mail-store-forget! mail-store-senders mail-store-recipients
          mail-store-copies mail-store-move!
          mail-store-draft! mail-store-drafts mail-store-delete-draft! mail-store-prepare-imap!
          mail-store-draft-state mail-store-claim-draft! mail-store-finish-draft!
          mail-store-ambiguous-attempt mail-store-resolve-draft! call-with-mail-store-lease
          mail-store-skip-imap! mail-ref)
  (import (techne) (techne task) (techne sqlite) (techne mail mime) (techne mail date) (techne mail maildir))
  (begin
    (define (mail-ref object key . default)
      "Return KEY in mail alist OBJECT, or DEFAULT (otherwise #f)."
      (let ((entry (assq key object)))
        (if entry (cdr entry) (if (pair? default) (car default) #f))))

    (define (mail-store-open path)
      "Open the local mail cache at PATH; :memory: makes an isolated store.
Sources are authoritative: a Maildir message's read, star and trashed
state are its flags, read at each sync and written when they change."
      (let ((db (sqlite-open path #:private? #t)))
        (for-each
         (lambda (sql) (sqlite-execute db sql '()))
         '("CREATE TABLE IF NOT EXISTS mail_messages (id INTEGER PRIMARY KEY, account TEXT NOT NULL, mailbox TEXT NOT NULL, source_key TEXT NOT NULL, raw TEXT NOT NULL, subject TEXT NOT NULL, sender TEXT NOT NULL, recipients TEXT NOT NULL, date TEXT NOT NULL, sent_at INTEGER NOT NULL DEFAULT 0, message_id TEXT NOT NULL, reply_to TEXT NOT NULL, refs TEXT NOT NULL, body TEXT NOT NULL, unread INTEGER NOT NULL DEFAULT 1, starred INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0, UNIQUE(account,mailbox,source_key))"
           "CREATE INDEX IF NOT EXISTS mail_source ON mail_messages(account,mailbox)"
           "CREATE TABLE IF NOT EXISTS mail_drafts (id TEXT PRIMARY KEY, account TEXT NOT NULL, text TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'draft')"
           "CREATE VIRTUAL TABLE IF NOT EXISTS mail_search USING fts5(subject,sender,body,content='mail_messages',content_rowid='id')"
           "CREATE TRIGGER IF NOT EXISTS mail_search_insert AFTER INSERT ON mail_messages BEGIN INSERT INTO mail_search(rowid,subject,sender,body) VALUES(new.id,new.subject,new.sender,new.body); END"
           "CREATE TRIGGER IF NOT EXISTS mail_search_delete AFTER DELETE ON mail_messages BEGIN INSERT INTO mail_search(mail_search,rowid,subject,sender,body) VALUES('delete',old.id,old.subject,old.sender,old.body); END"
           "CREATE TRIGGER IF NOT EXISTS mail_search_update AFTER UPDATE OF subject,sender,body ON mail_messages BEGIN INSERT INTO mail_search(mail_search,rowid,subject,sender,body) VALUES('delete',old.id,old.subject,old.sender,old.body); INSERT INTO mail_search(rowid,subject,sender,body) VALUES(new.id,new.subject,new.sender,new.body); END"))
        ;; A cache created by an earlier development version lacked chronology.
        (guard (e ((not (task-cancelled-condition? e)) (sqlite-execute db "ALTER TABLE mail_messages ADD COLUMN sent_at INTEGER NOT NULL DEFAULT 0" '())))
          (sqlite-query db "SELECT sent_at FROM mail_messages LIMIT 0" '()))
        (guard (e ((not (task-cancelled-condition? e)) (sqlite-execute db "ALTER TABLE mail_messages ADD COLUMN source_path TEXT NOT NULL DEFAULT ''" (quote ()))))
          (sqlite-query db "SELECT source_path FROM mail_messages LIMIT 0" (quote ())))
        (sqlite-execute db "CREATE INDEX IF NOT EXISTS mail_chronology ON mail_messages(sent_at DESC,id DESC)" (quote ()))
        (sqlite-execute db "CREATE INDEX IF NOT EXISTS mail_copies ON mail_messages(account,message_id)" '())
        ;; A Maildir message's T flag, as mbsync expunges it next: gone from
        ;; every list, until it is gone from its folder too.
        (guard (e ((not (task-cancelled-condition? e))
                   (sqlite-execute db "ALTER TABLE mail_messages ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0" '())))
          (sqlite-query db "SELECT trashed FROM mail_messages LIMIT 0" '()))
        ;; A cache made before copies were recognised: find them once.
        (guard (e ((not (task-cancelled-condition? e))
                   (sqlite-execute db "ALTER TABLE mail_messages ADD COLUMN duplicate INTEGER NOT NULL DEFAULT 0" '())
                   (mail-store-dedupe! db)))
          (sqlite-query db "SELECT duplicate FROM mail_messages LIMIT 0" '()))
        ;; A draft saved before drafts said when.
        (guard (e ((not (task-cancelled-condition? e))
                   (sqlite-execute db "ALTER TABLE mail_drafts ADD COLUMN updated INTEGER NOT NULL DEFAULT 0" '())))
          (sqlite-query db "SELECT updated FROM mail_drafts LIMIT 0" '()))
        (guard (e ((not (task-cancelled-condition? e))
                   (guard (e ((not (task-cancelled-condition? e))
                              (sqlite-query db "SELECT attempt FROM mail_drafts LIMIT 0" '())))
                     (sqlite-execute db "ALTER TABLE mail_drafts ADD COLUMN attempt TEXT NOT NULL DEFAULT ''" '()))))
          (sqlite-query db "SELECT attempt FROM mail_drafts LIMIT 0" '()))
        (sqlite-execute db "CREATE TABLE IF NOT EXISTS mail_imap_skipped (account TEXT NOT NULL, mailbox TEXT NOT NULL, validity INTEGER NOT NULL, uid INTEGER NOT NULL, PRIMARY KEY(account,mailbox))" '())
        ;; Counting a folder's mail, or a group's, reads this and not the
        ;; rows: a cold cache of 500 MB took seconds without it.
        (sqlite-execute db "DROP INDEX IF EXISTS mail_folder_state" '())
        (sqlite-execute db "CREATE INDEX IF NOT EXISTS mail_folder_flags ON mail_messages(account,mailbox,duplicate,trashed,unread,starred)" '())
        db))

    ;; Which copy of a message is the one shown: one not trashed; the
    ;; Inbox's, else an ordinary folder's, else a Gmail label's, All
    ;; Mail's last; among equals the first cached.
    (define copy-rank
      "CASE WHEN lower(b.mailbox)='inbox' THEN 0 WHEN b.mailbox LIKE '%/All Mail' THEN 3 WHEN b.mailbox LIKE '[%]/%' THEN 2 ELSE 1 END")
    (define (mail-store-dedupe! db [account #f])
      "Choose the copy shown of each message of ACCOUNT in DB, or of all.
Rows of one account with one Message-ID are copies of a message, as Gmail
keeps a message in Inbox and again in All Mail. One is listed and counted;
the others stay cached, found by a search of their own mailbox, and take
every local state change with it. A row without a Message-ID has no copies.
Run after rows are added; only rows whose standing changes are written."
      (let ((shown (string-append "(id<>(SELECT b.id FROM mail_messages b WHERE b.account=mail_messages.account"
                                  " AND b.message_id=mail_messages.message_id ORDER BY b.trashed," copy-rank ",b.id LIMIT 1))")))
        (sqlite-execute db (string-append "UPDATE mail_messages SET duplicate=" shown
                                          " WHERE message_id<>'' AND duplicate<>" shown
                                          (if account " AND account=?" ""))
                        (if account (list account) '()))))

    (define (mail-store-close db)
      "Close local mail store DB."
      (sqlite-close db))

    (define (mail-store-source-cached? db account mailbox key)
      "Return whether DB caches source KEY of ACCOUNT and MAILBOX.
A Maildir key denotes immutable content. Check before opening its body;
explicit mail-store-import! still supports replacing cached content."
      (pair? (sqlite-query db "SELECT 1 FROM mail_messages WHERE account=? AND mailbox=? AND source_key=? LIMIT 1"
                          (list account mailbox key))))

    (define (mail-store-source-keys db account mailbox)
      "Return the cached source keys in DB for ACCOUNT and MAILBOX.
A hash table of key to (id unread starred trashed path), true as a set.
Page by cache ID, not OFFSET, keeping each SQLite reply below its row limit."
      (let ((keys (make-hash-table)))
        (let loop ((after 0))
          (let ((rows (sqlite-query db "SELECT id,source_key,unread,starred,trashed,source_path FROM mail_messages WHERE account=? AND mailbox=? AND id>? ORDER BY id LIMIT 1024"
                                    (list account mailbox after))))
            (for-each (lambda (r) (hash-table-set! keys (cadr r) (cons (car r) (cddr r)))) rows)
            (when (= (length rows) 1024) (loop (car (car (reverse rows)))))))
        keys))
    (define (prepare-entry account mailbox entry)
      (let* ((raw (mail-ref entry 'raw)) (flags (mail-ref entry 'flags '()))
             (path (mail-ref entry 'path "")) (msg (mail-parse raw #:index? #t)))
        (list account mailbox (mail-ref entry 'key) (if (string=? path "") raw #u8())
              (mail-header msg "subject" "(no subject)") (mail-header msg "from") (mail-header msg "to") (mail-header msg "date")
              (or (mail-date->instant (mail-header msg "date")) 0) (mail-header msg "message-id") (mail-header msg "in-reply-to")
              (mail-header msg "references") (or (mail-text msg) (mail-html msg) "")
              (if (member "\\Seen" flags) 0 1) (if (member "\\Flagged" flags) 1 0) path
              (if (member "\\Deleted" flags) 1 0))))
    (define insert-prefix "INSERT INTO mail_messages(account,mailbox,source_key,raw,subject,sender,recipients,date,sent_at,message_id,reply_to,refs,body,unread,starred,source_path,trashed) ")
    ;; Both paths keep IDs and all local flags on conflict.
    (define upsert-suffix " ON CONFLICT(account,mailbox,source_key) DO UPDATE SET raw=excluded.raw,subject=excluded.subject,sender=excluded.sender,recipients=excluded.recipients,date=excluded.date,sent_at=excluded.sent_at,message_id=excluded.message_id,reply_to=excluded.reply_to,refs=excluded.refs,body=excluded.body,source_path=excluded.source_path")
    (define (commit-entries! db rows)
      (if (null? rows) 0
          (sqlite-execute db
            (string-append insert-prefix "VALUES" (string-join (make-list (length rows) "(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)") ",") upsert-suffix)
            (append-map (lambda (r) r) rows))))
    ;; An existing generic codec/table mechanism, not interpolated user SQL.
    ;; Raw-retaining IMAP entries always use the binary-safe parameter path.
    (define maildir-insert
      (string-append insert-prefix "SELECT "
        (string-join (map (lambda (i) (if (= i 3) "X''" (string-append "json_extract(value,'$[" (number->string i) "]')"))) (iota 17)) ",")
        " FROM json_each(?) WHERE 1" upsert-suffix))
    (define (index-row-bound row)
      ;; UTF-8 JSON keeps non-ASCII; control characters cost at most six
      ;; bytes. Include punctuation/numeric slack and the ignored raw slot.
      (+ 2 (apply + (map (lambda (v) (if (string? v) (+ 4 (* 6 (string-length v))) 64)) row))))
    (define (commit-maildir-entries! db rows)
      (if (<= (length rows) 15) (commit-entries! db rows)
          (begin
            (unless (and (<= (length rows) 256)
                         (every (lambda (r) (and (bytevector? (list-ref r 3)) (= 0 (bytevector-length (list-ref r 3)))
                                                  (string? (list-ref r 15)) (not (string=? (list-ref r 15) "")))) rows)
                         (<= (+ 2 (apply + (map index-row-bound rows))) 16777216))
              (error "Maildir batch exceeds bounded source-backed index format"))
            (let ((json (json->string (list->vector
                          (map (lambda (r) (list->vector (append (take r 3) (list #f) (drop r 4)))) rows)))))
              (when (> (bytevector-length (string->utf8 json)) 16777216) (error "Maildir JSON batch exceeds 16 MiB"))
              (sqlite-execute db maildir-insert (list json))))))
    (define (mail-store-import-batch! db account mailbox entries)
      "Cache ENTRIES atomically in DB for ACCOUNT and MAILBOX.
Each entry has key/raw/flags and optionally an absolute Maildir path. With a
path, retain searchable text and a locator, not another raw-message copy.
At most fifteen raw-retaining entries (SQLite's 256 parameters, 17 a row),
or 256 entirely source-backed entries,
are accepted. Large source-backed batches must fit the 16 MiB JSON bound.
Only text MIME parts are decoded. Local read/star/archive state is preserved."
      (let ((n (length entries)))
        (when (or (> n 256) (and (> n 15) (not (every (lambda (e) (let ((p (mail-ref e 'path ""))) (and (string? p) (not (string=? p ""))))) entries))))
          (error "Mail cache batch exceeds its source-backed or raw-retaining limit"))
        (let ((rows (map (lambda (e) (prepare-entry account mailbox e)) entries)))
          (if (> n 15) (commit-maildir-entries! db rows) (commit-entries! db rows)))))
    (define (mail-store-import! db account mailbox key raw flags)
      "Cache RAW under ACCOUNT, MAILBOX and source KEY in DB.
FLAGS seed read/star state only on first import; resync preserves local choices.
An IMAP KEY must contain UIDVALIDITY and UID, not a sequence number."
      (mail-store-import-batch! db account mailbox (list (list (cons 'key key) (cons 'raw raw) (cons 'flags flags)))))
    (define (mail-store-forget! db account #:mailbox [mailbox #f] #:keys [keys #f])
      "Remove what DB caches of ACCOUNT, or of its MAILBOX, or of KEYS in it.
KEYS, a list of source keys, are messages of MAILBOX. Return how many
messages went, with their local read, star and archive state; drafts
stay. Which copy of a message is shown is chosen again. Nothing at the
source changes: fetching what is still there brings it back."
      (let ((removed
             (cond (keys
                    (let loop ((keys keys) (n 0))
                      (if (null? keys) n
                          (let ((chunk (take keys (min 2000 (length keys)))))
                            (loop (drop keys (length chunk))
                                  (+ n (sqlite-execute db "DELETE FROM mail_messages WHERE account=? AND mailbox=? AND source_key IN (SELECT value FROM json_each(?))"
                                                       (list account mailbox (json->string (list->vector chunk))))))))))
                   (mailbox (sqlite-execute db "DELETE FROM mail_messages WHERE account=? AND mailbox=?" (list account mailbox)))
                   (else (sqlite-execute db "DELETE FROM mail_messages WHERE account=?" (list account))))))
        (unless keys
          (sqlite-execute db (if mailbox "DELETE FROM mail_imap_skipped WHERE account=? AND mailbox=?"
                                          "DELETE FROM mail_imap_skipped WHERE account=?")
                          (if mailbox (list account mailbox) (list account))))
        (when (and (> removed 0) (or mailbox keys)) (mail-store-dedupe! db account))
        removed))
    (define (follow-flags! db account present keys)
      ;; Cached rows whose file's flags or place changed, as mbsync or a
      ;; phone left them: the source's flags are the state. Return how many.
      (let ((changed
             (filter-map (lambda (key)
                           (let ((cached (hash-table-ref/default keys key #f)) (path (hash-table-ref present key)))
                             (and (pair? cached)
                                  (let* ((flags (maildir-path-flags path))
                                         (now (list (if (member "\\Seen" flags) 0 1) (if (member "\\Flagged" flags) 1 0)
                                                    (if (member "\\Deleted" flags) 1 0) path)))
                                    (and (not (equal? now (cdr cached))) (list->vector (cons (car cached) now)))))))
                         (hash-table-keys present))))
        (let loop ((rows changed))
          (when (pair? rows)
            (let ((chunk (take rows (min 2000 (length rows)))))
              (sqlite-execute db (string-append "UPDATE mail_messages SET unread=json_extract(j.value,'$[1]'),starred=json_extract(j.value,'$[2]'),"
                                                "trashed=json_extract(j.value,'$[3]'),source_path=json_extract(j.value,'$[4]')"
                                                " FROM json_each(?) AS j WHERE mail_messages.id=json_extract(j.value,'$[0]')")
                              (list (json->string (list->vector chunk))))
              (loop (drop rows (length chunk))))))
        (when (pair? changed) (mail-store-dedupe! db account))
        (length changed)))
    (define (mail-store-import-maildir! db account mailbox root #:notify [notify (lambda (n) #f)] #:on-error [on-error #f]
                                        #:remove-missing [remove-missing #f])
      "Index Maildir ROOT into DB as ACCOUNT's MAILBOX, writing no source.
Bulk-load cached keys once. Buffer at most 256 messages, 4 MiB of source or a
conservative 16 MiB JSON bound per atomic commit. One larger bounded message
is committed alone using binary-safe parameters, not JSON.
NOTIFY receives newly committed counts, never pending ones. ON-ERROR receives
(key condition) for source/parse failures; without it those failures propagate.
Cache commit failures always propagate. Raw bodies remain in Maildir; decoded
attachments are loaded only by the reader. REMOVE-MISSING, a procedure,
receives the count of cached messages whose file is gone from ROOT, once
they are removed (`mail-store-forget!`); without it they stay. With it,
cached messages also take the flags their files have now."
      (let* ((keys (mail-store-source-keys db account mailbox)) (pending '()) (bytes 0) (encoded 2) (total 0)
             ;; Listed before anything is read: a message arriving meanwhile
             ;; is not mistaken for one gone.
             (present (and remove-missing (maildir-keys root)))
             (gone (if present
                       (filter (lambda (key) (not (hash-table-ref/default present key #f))) (hash-table-keys keys))
                       '())))
        (when present (follow-flags! db account present keys))
        (define (flush!)
          (when (pair? pending)
            (commit-maildir-entries! db (map cdr (reverse pending)))
            (let ((n (length pending)))
              (set! pending '()) (set! bytes 0) (set! encoded 2) (set! total (+ total n)) (notify n))))
        (maildir-for-each root
          (lambda (m)
            (let ((size (bytevector-length (mail-ref m 'raw))))
              (when (and (pair? pending) (> (+ bytes size) 4194304)) (flush!))
              (let ((prepared (guard (e ((not (task-cancelled-condition? e)) (if on-error (begin (on-error (mail-ref m 'key) e) #f) (raise e))))
                                (prepare-entry account mailbox m))))
                (when prepared
                  (let ((bound (index-row-bound prepared)))
                    (when (and (pair? pending) (> (+ encoded bound) 16777216)) (flush!))
                    (set! pending (cons (cons (mail-ref m 'key) prepared) pending))
                    (hash-table-set! keys (mail-ref m 'key) #t)
                    (set! bytes (+ bytes size)) (set! encoded (+ encoded bound))
                    (when (or (= (length pending) 256) (>= bytes 4194304) (>= encoded 16777216)) (flush!)))))))
          #:skip? (lambda (key) (hash-table-ref/default keys key #f)) #:cached-keys keys
          #:order 'unread-newest #:on-error on-error)
        (flush!)
        ;; New rows may be copies of ones cached from another folder.
        (when (> total 0) (mail-store-dedupe! db account))
        (when (pair? gone) (remove-missing (mail-store-forget! db account #:mailbox mailbox #:keys gone)))
        total))

    (define columns '(id account mailbox source-key subject sender recipients date message-id reply-to refs body unread starred trashed sent-at))
    (define select-columns "id,account,mailbox,source_key,subject,sender,recipients,date,message_id,reply_to,refs,substr(body,1,180),unread,starred,trashed,sent_at")
    (define (as-message row) (map cons columns row))

    (define (like-escape text)
      (apply string-append
             (map (lambda (c) (if (memv c '(#\% #\_ #\\)) (string #\\ c) (string c)))
                  (string->list text))))

    (define (query-parts query)
      ;; Deliberately a small, predictable query language, never SQL or FTS syntax.
      (let loop ((words (remove (lambda (x) (string=? x "")) (string-split query " ")))
                 (where '()) (params '()))
        (if (null? words)
            (cons (reverse where) (reverse params))
            (let* ((word (car words))
                   (prefix (find (lambda (p) (string-prefix? p word)) '("from:" "subject:" "in:" "account:"))))
              (cond
               ((string=? word "is:unread") (loop (cdr words) (cons "unread=1" where) params))
               ((string=? word "is:read") (loop (cdr words) (cons "unread=0" where) params))
               ((string=? word "is:starred") (loop (cdr words) (cons "starred=1" where) params))
               (prefix
                (let ((column (cond ((string=? prefix "from:") "sender") ((string=? prefix "subject:") "subject")
                                    ((string=? prefix "in:") "mailbox") (else "account"))))
                  (loop (cdr words) (cons (string-append column " LIKE ? ESCAPE '\\'") where)
                        (cons (string-append "%" (like-escape (substring word (string-length prefix) (string-length word))) "%") params))))
               (else
                (loop (cdr words) (cons "id IN (SELECT rowid FROM mail_search WHERE mail_search MATCH ?)" where)
                      (cons (string-append "\"" (string-join (string-split word "\"") "\"\"") "\"") params))))))))

    ;; The WHERE clauses and parameters of a view: a group, an account and
    ;; a folder of it, then a search. A folder is asked for by name (in:,
    ;; or MAILBOX exactly), so its copies of messages are what is found;
    ;; the Inbox is the Inbox folders, as mu4e and Gnus have it. A message
    ;; trashed is in no view: it goes at the next mbsync.
    (define (view-where query group account mailbox)
      (let* ((parts (query-parts query))
             (inbox? (and (not mailbox) (not (memq group '(all unread starred)))))
             (base (case group ((all) "trashed=0") ((unread) "trashed=0 AND unread=1")
                         ((starred) "trashed=0 AND starred=1") (else "trashed=0 AND lower(mailbox)='inbox'")))
             (by-mailbox? (or mailbox inbox? (any (lambda (clause) (string-prefix? "mailbox " clause)) (car parts))))
             (exact (append (if account '("account=?") '()) (if mailbox '("mailbox=?") '())))
             (exact-params (append (if account (list account) '()) (if mailbox (list mailbox) '()))))
        (list (append (list base) exact (if by-mailbox? '() '("duplicate=0")) (car parts))
              (append exact-params (cdr parts))
              by-mailbox?)))

    (define (mail-store-count db query #:group [group 'inbox] #:account [account #f] #:mailbox [mailbox #f])
      "Return how many messages of DB `mail-store-query` finds, unlimited.
QUERY, GROUP, ACCOUNT and MAILBOX are as there. A message with copies
counts once, unless MAILBOX asks for a folder."
      (let ((where (view-where query group account mailbox)))
        ;; A folder can hold a message twice; it is listed once.
        (caar (sqlite-query db (string-append "SELECT "
                                              (if (caddr where)
                                                  "count(DISTINCT account||char(0)||CASE WHEN message_id='' THEN '#'||id ELSE message_id END)"
                                                  "count(*)")
                                              " FROM mail_messages WHERE " (string-join (car where) " AND "))
                            (cadr where)))))

    (define (mail-store-query db query #:group [group 'inbox] #:account [account #f] #:mailbox [mailbox #f] #:limit [limit 500])
      "Return at most LIMIT message summaries matching QUERY in DB.
GROUP is inbox (the Inbox folders), all, unread or starred. ACCOUNT, if
given, is the only account searched, and MAILBOX the only folder of it.
Search supports from:, subject:, in:, account:, is:unread, is:read and
is:starred. Other words search subject, sender and body using SQLite FTS5.
Field filters match literal substrings. Values are bound SQL parameters;
FTS operators in input are quoted as data. Sort by parsed Date, then cache
ID as a fallback. A message with copies (`mail-store-dedupe!`) is returned
once: its shown copy, or with in: or MAILBOX, the copy found there."
      (let* ((where (view-where query group account mailbox))
             (by-mailbox? (caddr where))
             (rows (map as-message
                        (sqlite-query db (string-append "SELECT " select-columns " FROM mail_messages WHERE "
                                                        (string-join (car where) " AND ") " ORDER BY sent_at DESC,id DESC LIMIT ?")
                                      (append (cadr where) (list (max 1 (min 5000 limit))))))))
        (if (not by-mailbox?) rows
            ;; Two mailboxes can match (in:Mail finds All Mail and Sent Mail).
            (let ((seen (make-hash-table)))
              (filter (lambda (m)
                        (let ((id (mail-ref m 'message-id "")))
                          (or (string=? id "")
                              (let ((key (string-append (mail-ref m 'account "") "\x0;" id)))
                                (and (not (hash-table-ref/default seen key #f))
                                     (begin (hash-table-set! seen key #t) #t))))))
                      rows)))))

    (define (mail-store-message db id #:raw? [include-raw? #t])
      "Return message ID in DB, or #f. INCLUDE-RAW? loads MIME on demand.
Maildir MIME is read from its immutable source key, following flag renames;
unavailable sources raise an error. With INCLUDE-RAW? false, read cached state
only, without touching Maildir or transferring raw blobs."
      (let ((rows (sqlite-query db (string-append "SELECT " select-columns
                                    (if include-raw? ",raw,source_path" ",X'',source_path")
                                    " FROM mail_messages WHERE id=?") (list id))))
        (and (pair? rows)
             (let* ((m (map cons (append columns '(raw source-path)) (car rows)))
                    (path (mail-ref m 'source-path)))
               (if (and include-raw? (not (string=? path "")))
                   (cons (cons 'raw (maildir-source-bytes path (mail-ref m 'source-key)))
                         (remove (lambda (e) (eq? (car e) 'raw)) m))
                   m)))))

    (define (copy-rows db ids copies?)
      (let loop ((ids ids) (out '()))
        (if (null? ids) (delete-duplicates out)
            (let* ((n (min 1000 (length ids))) (json (json->string (list->vector (take ids n))))
                   (sql (string-append "SELECT id,account,mailbox,source_key,source_path,unread,starred,trashed FROM mail_messages"
                                       " WHERE id>?2 AND (id IN (SELECT value FROM json_each(?1))"
                                       (if copies?
                                           " OR (message_id<>'' AND (account,message_id) IN (SELECT account,message_id FROM mail_messages WHERE id IN (SELECT value FROM json_each(?1))))"
                                           "") ") ORDER BY id LIMIT 512")))
              (loop (drop ids n)
                    (let page ((after 0) (out out))
                      (let ((rows (sqlite-query db sql (list json after))))
                        (if (< (length rows) 512) (append rows out)
                            (page (caar (reverse rows)) (append rows out))))))))))
    (define (mail-store-copies db ids #:copies? [copies? #t])
      "Return (ID ACCOUNT MAILBOX KEY PATH) of IDS in DB, with their copies.
COPIES? #f leaves them out. PATH is a Maildir message's, else empty."
      (map (lambda (row) (take row 5)) (copy-rows db ids copies?)))

    (define (mail-store-state! db ids field value #:copies? [copies? #t]
                              #:expected [expected #f] #:on-change [on-change (lambda (rows) #f)])
      "Set FIELD of IDS in DB to VALUE; COPIES? includes their other copies.
FIELD is unread, starred, or trashed.
With EXPECTED, change only exact matching (ID ACCOUNT MAILBOX KEY PATH) rows.
ON-CHANGE receives undo records after each bounded batch, so prior batches
remain undoable if a later batch fails. Maildir flags are renamed at source."
      (let* ((index (case field ((unread) 5) ((starred) 6) ((trashed) 7) (else (error "Unknown mail state" field))))
             (column (symbol->string field))
             (letter (case field ((unread) #\S) ((starred) #\F) (else #\T)))
             (on? (if (eq? field 'unread) (not value) value))
             (wanted (if value 1 0))
             (identities (and expected (make-hash-table))))
        (when expected (for-each (lambda (r) (hash-table-set! identities (car r) r)) expected))
        (let loop ((rows (copy-rows db ids (and copies? (not expected)))))
          (when (pair? rows)
            (let* ((n (min 512 (length rows)))
                   (changed
                    (filter-map
                     (lambda (r)
                       (let* ((identity (take r 5)) (path (list-ref r 4)))
                         (and (not (= wanted (list-ref r index)))
                              (or (not expected) (and (equal? identity (hash-table-ref/default identities (car r) #f))
                                                      (or (string=? path "") (file-exists? path))))
                              (let ((now (if (string=? path "") path (maildir-set-flag! path (list-ref r 3) letter on?))))
                                (and now (list identity now))))))
                     (take rows n))))
              (when (pair? changed)
                ;; The row must still be the same cached source before writing.
                (sqlite-execute db
                  (string-append "UPDATE mail_messages SET " column "=?1,source_path=json_extract(j.value,'$[5]')"
                                 " FROM json_each(?2) AS j WHERE (mail_messages.id,account,mailbox,source_key,source_path)="
                                 "(json_extract(j.value,'$[0]'),json_extract(j.value,'$[1]'),json_extract(j.value,'$[2]'),json_extract(j.value,'$[3]'),json_extract(j.value,'$[4]'))")
                  (list wanted (json->string (list->vector (map (lambda (c) (list->vector (append (car c) (cdr c)))) changed)))))
                (on-change (map (lambda (c) (list (caar c) field (not value) (append (take (car c) 4) (cdr c)))) changed))
                (when (eq? field 'trashed)
                  (for-each (lambda (account) (mail-store-dedupe! db account)) (delete-duplicates (map cadar changed)))))
              (loop (drop rows n)))))))
    (define (mail-store-move! db id mailbox root #:expected [expected #f] #:undo? [undo? #f])
      "Move Maildir message ID of DB to MAILBOX at ROOT, keeping its flags.
EXPECTED, if set, requires the same cached identity and location before moving.
Return (MAILBOX PATH) it was in, with its new identity appended if UNDO?."
      (let* ((rows (copy-rows db (list id) #f)) (row (and (pair? rows) (take (car rows) 5))))
        (and row (or (not expected) (equal? expected row))
             (let ((path (list-ref row 4)) (key (list-ref row 3)))
               (when (string=? path "") (error "Only a Maildir message can be moved"))
               (let ((now (and (or (not expected) (file-exists? path)) (maildir-move! path key root))))
                 (and now
                      (begin
                        (sqlite-execute db "UPDATE mail_messages SET mailbox=?,source_path=? WHERE id=? AND account=? AND mailbox=? AND source_key=? AND source_path=?"
                                        (append (list mailbox now) row))
                        (mail-store-dedupe! db (cadr row))
                        (append (list (caddr row) path) (if undo? (list (list id (cadr row) mailbox key now)) '())))))))))

    (define (mail-store-copy-mailboxes db id)
      "Return the mailboxes holding message ID of DB or a copy of it.
The shown copy's comes first; a message without copies has the one."
      (map car (sqlite-query db (string-append "SELECT b.mailbox FROM mail_messages a JOIN mail_messages b ON b.account=a.account"
                                               " AND (b.id=a.id OR (a.message_id<>'' AND b.message_id=a.message_id))"
                                               " WHERE a.id=? ORDER BY b.duplicate,b.id LIMIT 50")
                                (list id))))

    (define (mail-store-counts db)
      "Return total, unread and starred counts in DB as an alist.
A message is counted once, its copies not; trashed ones not at all."
      (let ((r (car (sqlite-query db "SELECT count(*),coalesce(sum(unread),0),coalesce(sum(starred),0) FROM mail_messages WHERE duplicate=0 AND trashed=0" '()))))
        (map cons '(total unread starred) r)))

    (define (mail-store-senders db #:account [account #f] #:sent? [sent? #f] #:to [to '()])
      "Return (FROM COUNT LAST) for each From header of DB's messages.
COUNT is how many messages have it, LAST when the newest was sent; the
most frequent come first. Only ACCOUNT's if given; only those of its
sent folders (a mailbox named with Sent) if SENT?; only those addressed
to one of the addresses TO, if any (in To, without case)."
      (let ((clauses (append '("duplicate=0" "sender<>''")
                             (if account '("account=?") '())
                             (if sent? '("mailbox LIKE '%sent%'") '())
                             (if (pair? to)
                                 (list (string-append "(" (string-join (map (lambda (a) "instr(lower(recipients),?)>0") to) " OR ") ")"))
                                 '()))))
        (sqlite-query db (string-append "SELECT sender,count(*),max(sent_at) FROM mail_messages WHERE "
                                        (string-join clauses " AND ") " GROUP BY sender ORDER BY 2 DESC LIMIT 5000")
                      (append (if account (list account) '()) (map string-downcase to)))))
    (define (mail-store-recipients db senders)
      "Return (TO COUNT LAST) for each To header of messages from SENDERS.
SENDERS are From headers as `mail-store-senders' gives them; the
messages are DB's, and the most frequent To headers come first."
      (if (null? senders) '()
          (sqlite-query db (string-append "SELECT recipients,count(*),max(sent_at) FROM mail_messages WHERE duplicate=0 AND recipients<>''"
                                          " AND sender IN (SELECT value FROM json_each(?)) GROUP BY recipients ORDER BY 2 DESC LIMIT 5000")
                        (list (json->string (list->vector senders))))))
    (define (mail-store-folders db)
      "Return what each cached folder of DB holds, one alist a folder.
Account and mailbox name it. Messages and unread count what is in it,
copies too, as its folder has them. Shown, shown-unread and starred count
the copy shown of a message (`mail-store-dedupe!`) alone, as the groups
do: so summed over an account's folders, or all, they are its groups'
counts. A trashed message is in none. One pass over an index, not the rows."
      (map (lambda (r) (map cons '(account mailbox messages unread shown shown-unread starred) r))
           (sqlite-query db (string-append
                             "SELECT account,mailbox,"
                             "sum(CASE WHEN trashed=0 THEN 1 ELSE 0 END),"
                             "sum(CASE WHEN trashed=0 AND unread=1 THEN 1 ELSE 0 END),"
                             "sum(CASE WHEN duplicate=0 AND trashed=0 THEN 1 ELSE 0 END),"
                             "sum(CASE WHEN duplicate=0 AND trashed=0 AND unread=1 THEN 1 ELSE 0 END),"
                             "sum(CASE WHEN duplicate=0 AND trashed=0 AND starred=1 THEN 1 ELSE 0 END)"
                             " FROM mail_messages GROUP BY account,mailbox ORDER BY account,mailbox")
                         '())))

    (define (mail-store-prepare-imap! db account mailbox validity)
      "Invalidate stale UID locators in DB and return the current highest UID.
ACCOUNT and MAILBOX identify the source; VALIDITY is its current UIDVALIDITY.
A changed validity rebuilds the local cache; no server message is touched.
Local reading state on invalidated entries is discarded."
      (unless (and (exact-integer? validity) (<= 1 validity 4294967295)) (error "Invalid UIDVALIDITY"))
      (let ((prefix (string-append (number->string validity) ":")))
        (sqlite-execute db "DELETE FROM mail_messages WHERE account=? AND mailbox=? AND source_key NOT LIKE ?"
                        (list account mailbox (string-append prefix "%")))
        (caar (sqlite-query db "SELECT max(coalesce((SELECT max(CAST(substr(source_key,?1) AS INTEGER)) FROM mail_messages WHERE account=?2 AND mailbox=?3),0),coalesce((SELECT uid FROM mail_imap_skipped WHERE account=?2 AND mailbox=?3 AND validity=?4),0))"
                            (list (+ 1 (string-length prefix)) account mailbox validity)))))
    (define (mail-store-skip-imap! db account mailbox validity uid)
      "Checkpoint an oversized UID of ACCOUNT and MAILBOX in DB.
VALIDITY is the mailbox's UIDVALIDITY. Never checkpoint a failed import."
      (sqlite-execute db "INSERT INTO mail_imap_skipped VALUES(?,?,?,?) ON CONFLICT(account,mailbox) DO UPDATE SET validity=excluded.validity,uid=CASE WHEN validity=excluded.validity THEN max(uid,excluded.uid) ELSE excluded.uid END"
                      (list account mailbox validity uid)))

    (define (mail-store-draft! db id account text #:state [state "draft"])
      "Save draft ID in DB with ACCOUNT, TEXT and STATE.
A live, uncertain or sent snapshot is never replaced."
      (unless (member state '("draft" "uncertain")) (error "Invalid draft save state" state))
      (sqlite-execute db (string-append "INSERT INTO mail_drafts(id,account,text,state,updated) VALUES(?,?,?,?,?)"
                                        " ON CONFLICT(id) DO UPDATE SET account=excluded.account,text=excluded.text,state=excluded.state,"
                                        "updated=CASE WHEN mail_drafts.text=excluded.text AND mail_drafts.account=excluded.account"
                                        " THEN mail_drafts.updated ELSE excluded.updated END WHERE mail_drafts.state='draft'")
                      (list id account text state (current-milliseconds))))

    (define (mail-store-draft-state db id)
      "Return durable delivery state of draft ID in DB, or #f."
      (let ((rows (sqlite-query db "SELECT state FROM mail_drafts WHERE id=?" (list id))))
        (and (pair? rows) (caar rows))))
    (define (call-with-mail-store-lease db proc)
      "Call PROC with exclusive delivery access to DB, across runtimes."
      (call-with-sqlite-lease db proc))
    (define (mail-store-claim-draft! db id text token)
      "Claim draft ID in DB for TOKEN if its saved TEXT still matches.
Hold the delivery lease while claiming and submitting."
      (unless (and (string? token) (not (string=? token ""))) (error "Missing delivery token"))
      (= 1 (sqlite-execute db "UPDATE mail_drafts SET state='sending',attempt=? WHERE id=? AND state='draft' AND text=?"
                           (list token id text))))
    (define (mail-store-finish-draft! db id token state)
      "Set the live attempt with ID and TOKEN in DB to STATE.
Hold the delivery lease while completing the attempt."
      (unless (member state '("sent" "uncertain" "draft")) (error "Invalid delivery outcome"))
      (= 1 (sqlite-execute db "UPDATE mail_drafts SET state=?,attempt=CASE WHEN ?='draft' THEN '' ELSE attempt END WHERE id=? AND state='sending' AND attempt=?"
                           (list state state id token))))
    (define (mail-store-ambiguous-attempt db id)
      "Read the ambiguous (STATE TOKEN) snapshot of ID in DB, or #f."
      (let ((rows (sqlite-query db "SELECT state,attempt FROM mail_drafts WHERE id=? AND state IN ('sending','uncertain')" (list id))))
        (and (pair? rows) (car rows))))
    (define (mail-store-resolve-draft! db id expected state)
      "Set draft ID in DB to STATE if its attempt matches EXPECTED.
The operator must check delivery before asking to resolve it.
A live runtime's lease prevents recovery, even in another process."
      (unless (member state '("draft" "sent")) (error "Invalid delivery resolution"))
      (call-with-mail-store-lease db
        (lambda ()
          (= 1 (sqlite-execute db "UPDATE mail_drafts SET state=?,attempt='' WHERE id=? AND state=? AND attempt=?"
                               (append (list state id) expected))))))

    (define (mail-store-drafts db)
      "Return saved (id account text state updated) drafts in DB, newest first.
UPDATED is when the text was last changed, in milliseconds, 0 if unknown."
      (sqlite-query db "SELECT id,account,text,state,updated FROM mail_drafts WHERE state<>'sent' ORDER BY updated DESC,rowid DESC" '()))

    (define (mail-store-delete-draft! db id)
      "Remove local draft ID from DB unless its delivery is unsettled."
      (let ((n (sqlite-execute db "DELETE FROM mail_drafts WHERE id=? AND state NOT IN ('sending','uncertain')" (list id))))
        (when (and (= n 0) (mail-store-ambiguous-attempt db id))
          (error "Cannot discard a draft with unsettled delivery"))
        n))))
