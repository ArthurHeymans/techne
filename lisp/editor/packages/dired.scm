;;; Directories, as Emacs's dired lists them and dirvish shows them.
;;;
;;; A directory's listing is a buffer of rows (listings.scm), one an entry,
;;; keyed by its name, standing for a file or directory target. RET goes
;;; into a directory or opens a file; ^ goes up. Entries are marked (m),
;;; flagged for deletion (d) and acted on together: deleted (x, D),
;;; renamed or moved (R), copied (C). The listing is made again after
;;; each, so the caret stays on its entry.
;;;
;;; M-x dirvish shows the directory as dirvish does, in three panes: the
;;; parent directory, the directory, and a preview of the entry at point,
;;; which follows the caret. Opening a file or q puts the panes back as
;;; they were.
;;;
;;; C-x C-q makes the names editable, as wdired does: each name is an
;;; excerpt of a document holding them, edited in place; C-c C-c renames
;;; the entries whose names changed, C-c C-k leaves them.

(import (techne editor))
(import (techne time))

(define-option dired-details #t
  "Whether a directory's listing shows each entry's mode, size and date.
\\[dired-toggle-details] turns them on and off in one listing."
  #:type 'boolean)

(define-option dired-hide-dotfiles #f
  "Whether a directory's listing leaves out the dotfiles, named .something.
\\[dired-toggle-dotfiles] turns them on and off in one listing."
  #:type 'boolean)

(define-option dired-sort 'name
  "How a directory's listing is sorted, directories first.
By `name`, by `time` (the newest first) or by `size` (the largest
first); \\[dired-cycle-sort] goes to the next in one listing."
  #:type '(one-of name time size))

(define-option dired-preview-bytes 65536
  "The most bytes of a file the preview of dirvish shows."
  #:type 'natural)

;;; Entries: (name kind size modified mode target), NAME as
;;; `directory-list` gives it (a directory's with a slash), the rest as
;;; `file-attributes` does.

(define (without-slash path)
  (if (and (string-suffix? "/" path) (> (string-length path) 1))
      (substring path 0 (- (string-length path) 1))
      path))

(define (entries dir)
  (filter-map (lambda (name)
                (let ((a (guard (e (#t #f)) (file-attributes (string-append dir (without-slash name))))))
                  (and a (cons name a))))
              (directory-list dir)))

(define (entry-name e) (car e))
(define (entry-kind e) (cadr e))
(define (entry-size e) (caddr e))
(define (entry-time e) (cadddr e))
(define (entry-mode e) (list-ref e 4))
(define (entry-link e) (list-ref e 5))
(define (entry-directory? e) (string-suffix? "/" (entry-name e)))
(define (dotfile? e) (string-prefix? "." (entry-name e)))

(define (sorted es by)
  (let ((before? (case by
                   ((time) (lambda (a b) (> (entry-time a) (entry-time b))))
                   ((size) (lambda (a b) (> (entry-size a) (entry-size b))))
                   (else (lambda (a b) (string-ci<? (entry-name a) (entry-name b)))))))
    (sort es (lambda (a b)
               (if (eq? (entry-directory? a) (entry-directory? b)) (before? a b) (entry-directory? a))))))

;;; Columns

(define (digit mode k) (remainder (quotient mode (expt 8 k)) 8))

(define (permissions e)
  (apply string-append
         (case (entry-kind e) ((directory) "d") ((symlink) "l") ((file) "-") (else "?"))
         (map (lambda (k)
                (let ((d (digit (entry-mode e) k)))
                  (string (if (>= d 4) #\r #\-) (if (odd? (quotient d 2)) #\w #\-) (if (odd? d) #\x #\-))))
              '(2 1 0))))

(define (executable? e)
  (and (eq? (entry-kind e) 'file) (any (lambda (k) (odd? (digit (entry-mode e) k))) '(0 1 2))))

(define (human-size n)
  "Return the size N, in bytes, as ls -h writes it."
  (let loop ((n (exact->inexact n)) (units '("" "k" "M" "G" "T" "P")))
    (cond ((or (< n 1024) (null? (cdr units)))
           (string-append (if (or (string=? (car units) "") (>= n 10))
                              (number->string (exact (round n)))
                              (number->string (/ (round (* n 10)) 10)))
                          (car units)))
          (else (loop (/ n 1024) (cdr units))))))

(define (pad-left text width)
  (if (>= (string-length text) width) text (string-append (make-string (- width (string-length text)) #\space) text)))

(define (two n) (if (< n 10) (string-append "0" (number->string n)) (number->string n)))

(define (date-text seconds zone)
  (let ((c (instant->civil (* seconds 1000) zone)))
    (string-append (number->string (car c)) "-" (two (cadr c)) "-" (two (caddr c)) " " (two (list-ref c 3)) ":" (two (list-ref c 4)))))

(define (name-face e mark)
  (cond ((eqv? mark #\D) 'error)
        (mark 'warning)
        ((entry-directory? e) 'keyword)
        ((eq? (entry-kind e) 'symlink) 'key)
        ((executable? e) 'string)
        (else #f)))

;;; Listings: their state holds the directory (`dir`, absolute, with a
;;; slash), and what was toggled in them (`details`, `dotfiles`, `sort`),
;;; else the options say.

(define (dired? b) (and b (derived-mode? (buffer-mode b) 'dired-mode) (listing? b)))
(define (dired-buffers) (filter dired? (buffer-list)))

(define (setting b key option-name)
  (let ((v (hash-table-ref/default (buffer-state b) key 'unset)))
    (if (eq? v 'unset) (option b option-name) v)))

(define (listing-dir b) (listing-get b 'dir))

;; The target of entry NAME of DIR: a directory's or a file's.
(define (entry-target dir name)
  (if (string-suffix? "/" name)
      (target 'directory (string-append dir name))
      (target 'file (string-append dir name))))

(define (dired-rows b)
  (let* ((dir (listing-dir b))
         (details (setting b 'details 'dired-details))
         (by (setting b 'sort 'dired-sort))
         (zone (guard (e (#t "UTC")) (local-zone)))
         (found (guard (e (#t (error-text e))) (entries dir)))
         (head (row #:key "" (list (list (abbreviate-path dir) 'keyword)
                                   (list (string-append "  by " (symbol->string by)) 'comment)))))
    (if (string? found)
        (list head (row #:key "error" (list (list found 'error))))
        (let* ((shown (sorted (if (setting b 'dotfiles 'dired-hide-dotfiles) (remove dotfile? found) found) by))
               (parent (and (not (string=? dir "/"))
                            (guard (e (#t #f)) (cons "../" (file-attributes (string-append dir ".."))))))
               (all (if parent (cons parent shown) shown))
               (width (fold (lambda (e w) (max w (string-length (size-text e)))) 0 all))
               (names (listing-get b 'names))
               (spans (and names (name-spans b))))
          (cons head
                (map (lambda (e)
                       (let* ((name (entry-name e))
                              (mark (listing-mark b name))
                              (shown-name (name-column e mark names spans))
                              (target (if (string=? name "../")
                                          (target 'directory (normal-directory (string-append dir "..")))
                                          (entry-target dir name))))
                         (if details
                             ((if (string=? name "../") parent-row listing-row) b name target
                                          (list (list (permissions e) 'comment))
                                          (pad-left (size-text e) width)
                                          (list (list (date-text (entry-time e) zone) 'comment))
                                          shown-name)
                             ((if (string=? name "../") parent-row listing-row) b name target shown-name))))
                     all))))))

;; The row of ../: it goes up, and is never marked or acted on.
(define (parent-row b key target . columns)
  (apply row #:key key #:target target " " columns))

(define (size-text e) (if (entry-directory? e) "-" (human-size (entry-size e))))

;; An entry's name: an excerpt of NAMES, the document of its editable
;; names, while they are edited; then a symbolic link's target.
(define (name-column e mark names spans)
  (let* ((name (entry-name e))
         (span (and spans (not (string=? name "../")) (hash-table-ref/default spans name #f))))
    (append (if span
                (list (excerpt names (car span) (cadr span) #:face (name-face e mark)))
                (list (list name (name-face e mark))))
            (if (entry-link e) (list (list (string-append " -> " (entry-link e)) 'comment)) '()))))

(define-mode dired-mode
  "A directory's listing.
\\[dired-find-file] goes into the directory or opens the file at point,
\\[dired-up-directory] goes up. Entries marked with
\\[listing-mark-row] or the one at point are deleted with
\\[dired-do-delete], renamed with \\[dired-do-rename] and copied with
\\[dired-do-copy]; \\[dired-do-flagged-delete] deletes those flagged
with \\[listing-flag-row]. \\[dired-create-directory] makes a
directory. \\[dired-toggle-details] shows and hides details,
\\[dired-toggle-dotfiles] dotfiles, \\[dired-cycle-sort] sorts by name,
time or size; \\[dired-toggle-read-only] edits the names."
  #:parent 'listing-mode
  #:keys '(("RET" dired-find-file) ("o" dired-find-file-other-pane)
           ("^" dired-up-directory) ("n" dired-next-line) ("p" dired-previous-line) ("SPC" dired-next-line)
           ("x" dired-do-flagged-delete) ("D" dired-do-delete) ("R" dired-do-rename) ("C" dired-do-copy)
           ("+" dired-create-directory) ("w" dired-copy-filename) ("(" dired-toggle-details) ("." dired-toggle-dotfiles)
           ("s" dired-cycle-sort) ("% m" dired-mark-matching) ("* /" dired-mark-directories)
           ("C-x C-q" dired-toggle-read-only) ("q" dired-quit) ("%" "regexp") ("*" "mark"))
  #:normal '(("RET" dired-find-file) ("l" dired-find-file) ("h" dired-up-directory) ("-" dired-up-directory)
             ("j" dired-next-line) ("k" dired-previous-line) ("o" dired-find-file-other-pane)
             ("x" dired-do-flagged-delete) ("D" dired-do-delete) ("R" dired-do-rename) ("C" dired-do-copy)
             ("+" dired-create-directory) ("y" dired-copy-filename) ("(" dired-toggle-details) ("." dired-toggle-dotfiles)
             ("s" dired-cycle-sort) ("% m" dired-mark-matching) ("* /" dired-mark-directories)
             ("i" dired-toggle-read-only) ("q" dired-quit) ("%" "regexp") ("*" "mark"))
  #:directory (lambda (b) (and (listing? b) (listing-dir b))))


;; The listing of DIR in buffer B, or the one named by it: the one there
;; is shown again, B is moved there when no other is.
(define (listing-for dir reuse)
  (let* ((name (abbreviate-path dir)) (old (buffer-named name)))
    (cond ((dired? old) old)
          ((and reuse (not old))
           (set-buffer-name! reuse name)
           (let ((marks (listing-get reuse 'marks)))
             (for-each (lambda (k) (hash-table-delete! marks k)) (hash-table-keys marks)))
           reuse)
          (else name))))

(define (dired! s dir #:select [select #f] #:reuse [reuse #f])
  "Show the listing of the directory DIR in the focused pane of S.
The caret goes to the entry named SELECT, else the first. REUSE, a
listing, is moved to DIR rather than another made."
  (let* ((dir (normal-directory dir))
         (found (listing-for dir reuse))
         (b (make-listing-buffer! (if (string? found) found (buffer-name found)) 'dired-mode dired-rows
                                  #:state (list (cons 'dir dir)))))
    (show-buffer! s b)
    (goto-name! s b (or (and select (presentation-row-span (buffer-document b) select) select)
                        (first-entry b)))
    b))

;; The first entry's key, past ../.
(define (first-entry b)
  (let* ((p (buffer-document b))
         (keys (filter-map (lambda (l) (presentation-key-at p (car l))) (document-lines p))))
    (find (lambda (k) (not (member k '("" "../" "error")))) keys)))

;; Put the caret on the name of entry KEY.
(define (goto-name! s b key)
  (let ((at (and key (name-position b key))))
    (when at (view-set-ranges! (session-view s) (list (list at at)) 0))))

;; Where the name of entry KEY is in listing B, or #f.
(define (name-position b key)
  (let* ((p (buffer-document b)) (span (presentation-row-span p key)))
    (and span
         (let* ((text (document-substring p (car span) (cadr span)))
                (at (or (name-start key text) (name-start (without-slash key) text))))
           (+ (car span) (if at (bytes (substring text 0 at)) 0))))))

(define (bytes text) (bytevector-length (string->utf8 text)))

;; Where the name KEY starts in a row's TEXT: the last place it is
;; followed by the end or by a link's arrow.
(define (name-start key text)
  (let ((n (string-length key)) (len (string-length text)))
    (let loop ((i (- len n)))
      (cond ((< i 0) #f)
            ((and (string=? (substring text i (+ i n)) key)
                  (or (= (+ i n) len) (string-prefix? " -> " (substring text (+ i n) len))))
             i)
            (else (loop (- i 1)))))))

(define (current s)
  (let ((b (session-buffer s)))
    (if (dired? b) b (error "Not a directory's listing"))))

(define (refresh-all!)
  (for-each listing-redisplay! (dired-buffers)))

;;; Moving and visiting

(define (move-line! s n)
  (let ((b (current s)))
    (run-command s (if (< n 0) 'previous-line 'next-line) (abs n))
    (goto-name! s b (listing-key-at b (point s)))))

(define-command (dired-next-line s n)
  "Move to the next entry's name."
  (move-line! s n))

(define-command (dired-previous-line s n)
  "Move to the previous entry's name."
  (move-line! s (- n)))

(define (target-here s)
  (or (target-at (doc s) (point s)) (error "No entry here")))

(define-command (dired-find-file s n)
  "Go into the directory at point, or open the file at point.
In dirvish, a file is opened where the panes were before."
  (let ((t (target-here s)))
    (if (eq? (target-kind t) 'directory)
        (go-to-directory! s (target-value t) #f)
        (begin (dirvish-quit! s) (show-document! s (file-document (target-value t)))))))

(define-command (dired-find-file-other-pane s n)
  "Open the entry at point in a new pane below."
  (let ((t (target-here s)))
    (dirvish-quit! s)
    (if (eq? (target-kind t) 'directory)
        (begin (show-in-other-pane! s (doc s)) (dired! s (target-value t)))
        (show-in-other-pane! s (file-document (target-value t))))))

;; Show DIR in the listing of the focused pane, moved there; in dirvish,
;; in the middle pane.
(define (go-to-directory! s dir select)
  (when (dirvish? s) (sset! s 'focus 1))
  (dired! s dir #:select select #:reuse (current s)))

(define-command (dired-up-directory s n)
  "Go up to the parent directory, the caret on this one's entry."
  (let ((dir (listing-dir (current s))))
    (unless (string=? dir "/")
      (go-to-directory! s (normal-directory (string-append dir "..")) (base-name (without-slash dir) #:slash #t)))))

(define (base-name path #:slash [slash #f])
  (let ((i (let loop ((i (- (string-length path) 1)))
             (cond ((< i 0) 0) ((char=? (string-ref path i) #\/) (+ i 1)) (else (loop (- i 1)))))))
    (string-append (substring path i (string-length path)) (if slash "/" ""))))

(define-command (dired s n)
  "Show a directory's listing, reading its name.
The directory of this buffer is offered first."
  (read-directory-name s "Dired: " (abbreviate-path (default-directory s))
                       (lambda (s dir) (dired! s dir))))

(define-command (dired-jump s n)
  "Show the listing of this buffer's directory, at its file's entry."
  (let ((path (document-path (doc s))))
    (dired! s (default-directory s) #:select (and path (base-name path)))))

;;; Acting on entries

;; The paths of the entries marked with MARK, else the one at point.
(define (paths s mark)
  (entries-only (current s) (map target-value (listing-targets s #:mark mark))))

;; PATHS, each the path of an entry of listing B: never its directory, the
;; one above, or anything elsewhere, which no command acts on.
(define (entries-only b paths)
  (let* ((dir (listing-dir b))
         (ps (filter (lambda (p)
                       (and (string-prefix? dir p)
                            (let ((name (without-slash (substring p (string-length dir) (string-length p)))))
                              (and (not (member name '("" "." "..")))
                                   (not (string-index name #\/))))))
                     paths)))
    (if (null? ps) (error "Not on an entry") ps)))

(define (describe paths)
  (if (null? (cdr paths))
      (base-name (without-slash (car paths)) #:slash (string-suffix? "/" (car paths)))
      (string-append (number->string (length paths)) " entries")))

(define (confirm s prompt yes)
  (completing-read s prompt '("no" "yes")
                   #:accept (lambda (s c) (if (string=? (candidate-text c) "yes") (yes s) (message! s "Nothing done")))))

;; Do (F path) for each of PATHS, then make the listings again; say how
;; many were done, and the first error. F may return a list of what went
;; wrong without stopping it, said too.
(define (each! s paths verb f)
  (let* ((said (map (lambda (p)
                      (guard (e (#t (cons #f (error-text e))))
                        (let ((r (f p))) (cons #t (if (and (pair? r) (string? (car r))) (car r) #f)))))
                    paths))
         (done (length (filter car said)))
         (problems (filter-map cdr said)))
    (refresh-all!)
    (message! s (string-append verb " " (number->string done) " of " (number->string (length paths))
                               (if (null? problems) "" (string-append "; " (car problems)))))))

(define (delete! s paths)
  (confirm s (string-append "Delete " (describe paths)
                            (if (any (lambda (p) (string-suffix? "/" p)) paths) " (directories with all they hold)" "")
                            "? ")
           (lambda (s) (each! s paths "Deleted" (lambda (p) (delete-path-and-buffers! s (without-slash p)))))))

(define-command (dired-do-delete s n)
  "Delete the marked entries, or the one at point, after asking.
Directories are deleted with all they hold, and the buffers of the
files deleted killed. An entry with a file of unsaved edits open is
left."
  (delete! s (paths s #\*)))

(define-command (dired-do-flagged-delete s n)
  "Delete the entries flagged for deletion, after asking."
  (let ((flagged (listing-marked (current s) #\D)))
    (if (null? flagged) (message! s "No entries flagged") (delete! s (entries-only (current s) (map target-value flagged))))))

;; Where PATH goes for the destination TO: into it, keeping its name,
;; when TO is a directory.
(define (destination path to)
  (let ((into (and (or (string-suffix? "/" to) (eq? (car (or (file-attributes to) '(#f))) 'directory))
                   (string-append (without-slash to) "/"))))
    (if into (string-append into (base-name (without-slash path))) to)))

;; Read where PATHS go and do (F from to) with each.
(define (transfer! s paths verb done f)
  (let* ((dir (listing-dir (current s)))
         (one (null? (cdr paths))))
    (completing-read s (string-append verb " " (describe paths) " to: ") '()
                     #:initial (abbreviate-path (if one (without-slash (car paths)) dir))
                     #:keymap file-name-map
                     #:require-match #f
                     #:accept (lambda (s c)
                                (let ((to (absolute-path (candidate-text c))))
                                  (each! s paths done
                                         (lambda (p) (f (without-slash p) (destination p to)))))))))

(define-command (dired-do-rename s n)
  "Rename or move the marked entries, or the one at point.
Into a directory they keep their names. The buffers of the files moved
follow them, unsaved edits and all."
  (transfer! s (paths s #\*) "Rename" "Renamed" move-path!))

(define-command (dired-do-copy s n)
  "Copy the marked entries, or the one at point.
Directories are copied with all they hold."
  (transfer! s (paths s #\*) "Copy" "Copied" copy-file))

(define-command (dired-create-directory s n)
  "Make a directory, reading its name."
  (let ((b (current s)))
    (completing-read s "Create directory: " '()
                     #:initial (abbreviate-path (listing-dir b))
                     #:keymap file-name-map
                     #:require-match #f
                     #:accept (lambda (s c)
                                (let ((dir (absolute-path (candidate-text c))))
                                  (make-directory dir)
                                  (refresh-all!)
                                  (goto-name! s b (string-append (base-name (without-slash dir)) "/")))))))

(define-command (dired-copy-filename s n)
  "Save the names of the marked entries, or the one at point, as a kill."
  (let ((names (string-join (map (lambda (p) (base-name (without-slash p))) (paths s #\*)) " ")))
    (kill-save! s names #f #f)
    (message! s names)))

;;; Marking

(define (mark-where! s keep?)
  (let* ((b (current s)) (marks (listing-get b 'marks)) (rows (listing-get b 'rows)))
    (for-each (lambda (k)
                (let ((r (hash-table-ref rows k)))
                  (when (and (row-target r) (not (string=? k "../")) (keep? k))
                    (hash-table-set! marks k #\*))))
              (hash-table-keys rows))
    (listing-redisplay! b)))

(define-command (dired-mark-matching s n)
  "Mark the entries whose names match a regular expression read.
It is written as `string->regexp` takes it, \\.rs$ say, and found
anywhere in the name; a directory's has no slash."
  (completing-read s "Mark matching regexp: " '()
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let* ((re (string->regexp (candidate-text c)))
                                     (before (count-marked (current s))))
                                (mark-where! s (lambda (k) (string-search-regexp (without-slash k) re)))
                                (message! s (string-append "Marked " (number->string (- (count-marked (current s)) before))))))))

(define (count-marked b)
  (length (filter (lambda (k) (eqv? (listing-mark b k) #\*)) (hash-table-keys (listing-get b 'rows)))))

(define-command (dired-mark-directories s n)
  "Mark the directories."
  (mark-where! s (lambda (k) (string-suffix? "/" k))))

;;; Toggles

(define (toggle! s key option-name)
  (let ((b (current s)))
    (listing-set! b key (not (setting b key option-name)))
    (listing-redisplay! b)
    (goto-name! s b (listing-key-at b (point s)))))

(define-command (dired-toggle-details s n)
  "Show or hide each entry's mode, size and date in this listing."
  (toggle! s 'details 'dired-details))

(define-command (dired-toggle-dotfiles s n)
  "Hide or show the entries starting with a dot in this listing."
  (toggle! s 'dotfiles 'dired-hide-dotfiles))

(define-command (dired-cycle-sort s n)
  "Sort this listing by the next of name, time and size."
  (let* ((b (current s)) (by (setting b 'sort 'dired-sort)))
    (listing-set! b 'sort (case by ((name) 'time) ((time) 'size) (else 'name)))
    (listing-redisplay! b)
    (goto-name! s b (listing-key-at b (point s)))))

;;; Editing names, as wdired: while `names` is in the state, each name is
;;; an excerpt of that document, one name a line, in the order of the
;;; entries in `original`, a list of (key . name). The rest of the listing
;;; stays read-only.

;; The span (start end) of each entry's name in `names`, by key, as it is
;; now; an error once a line was added or taken away.
(define (name-spans b)
  (let* ((original (listing-get b 'original))
         (lines (document-lines (listing-get b 'names)))
         (lines (if (and (pair? lines) (string=? (cadr (last lines)) "")) (drop-right lines 1) lines))
         (spans (make-hash-table)))
    (unless (= (length lines) (length original)) (error "A name a line: lines were added or taken away"))
    (for-each (lambda (e l) (hash-table-set! spans (car e) (list (car l) (+ (car l) (bytes (cadr l))))))
              original lines)
    spans))

(define-command (dired-toggle-read-only s n)
  "Edit the names of this listing's entries in place.
\\[dired-finish-edit] renames the entries whose names changed,
\\[dired-abort-edit] leaves them as they were."
  (let* ((b (current s))
         (keys (filter (lambda (k) (not (member k '("" "../" "error")))) (map car (sorted-rows b))))
         (names (map without-slash keys))
         (d (make-document (string-join names "\n"))))
    (listing-set! b 'names d)
    (listing-set! b 'original (map cons keys names))
    (set-buffer-mode! b 'wdired-mode)
    (redisplay-here! s b)
    (set-view-read-only! (session-view s) #f)
    (message! s "Edit the names; C-c C-c renames, C-c C-k leaves them")))

;; The rows by key, in order.
(define (sorted-rows b)
  (let ((p (buffer-document b)))
    (filter-map (lambda (l) (let ((k (presentation-key-at p (car l)))) (and k (cons k l)))) (document-lines p))))

(define-mode wdired-mode
  "A directory's listing whose names are edited.
\\[dired-finish-edit] renames the entries whose names changed,
\\[dired-abort-edit] leaves them as they were."
  #:keys '(("C-c C-c" dired-finish-edit) ("C-x C-s" dired-finish-edit) ("C-c C-k" dired-abort-edit) ("C-c ESC" dired-abort-edit))
  #:normal '(("Z Z" dired-finish-edit) ("Z Q" dired-abort-edit))
  #:directory (lambda (b) (and (listing? b) (listing-dir b))))

(define (end-edit! s b)
  (for-each (lambda (k) (hash-table-delete! (buffer-state b) k)) '(names original))
  (set-buffer-mode! b 'dired-mode)
  (set-view-read-only! (session-view s) #t)
  (redisplay-here! s b))

;; Make B's rows again, the caret going back to the name of its entry:
;; a row whose name becomes an excerpt, or text again, is shown anew.
(define (redisplay-here! s b)
  (let ((k (listing-key-at b (point s))))
    (listing-redisplay! b)
    (goto-name! s b k)))

(define (editing s)
  (let ((b (session-buffer s)))
    (if (and b (eq? (buffer-mode b) 'wdired-mode)) b (error "Not editing names"))))

(define-command (dired-finish-edit s n)
  "Rename the entries whose names were changed, and stop editing names."
  (let* ((b (editing s))
         (d (listing-get b 'names))
         (dir (listing-dir b))
         (spans (name-spans b))
         (renames (filter-map (lambda (e)
                                (let* ((span (hash-table-ref spans (car e)))
                                       (now (document-substring d (car span) (cadr span))))
                                  (and (not (string=? now (cdr e))) (list (cdr e) now))))
                              (listing-get b 'original)))
         (bad (find (lambda (r) (or (string=? (cadr r) "") (string-index (cadr r) #\/) (string-index (cadr r) #\newline))) renames)))
    (when bad (error "Not a name" (cadr bad)))
    (let ((twice (find (lambda (r) (< 1 (count (lambda (o) (string=? (cadr o) (cadr r))) renames))) renames)))
      (when twice (error "Two entries would be named" (cadr twice))))
    (let ((taken (find (lambda (r) (and (not (assoc (cadr r) renames)) (file-attributes (string-append dir (cadr r)))))
                       renames)))
      (when taken (error "Already there" (cadr taken))))
    (end-edit! s b)
    (let* ((reserved (append (map car renames) (map cadr renames)))
           (steps (rename-steps renames (lambda (name)
                                          (let ((t (free-name dir name reserved)))
                                            (set! reserved (cons t reserved))
                                            t))))
           (warnings '())
           ;; Steps depend on those before: the first that fails stops.
           (failed (let loop ((steps steps) (done 0))
                     (and (pair? steps)
                          (let ((problem (guard (e (#t (error-text e)))
                                           (set! warnings (append warnings (move-path! (string-append dir (car (car steps)))
                                                                                       (string-append dir (cadr (car steps))))))
                                           #f)))
                            (if problem
                                (string-append "Stopped after " (number->string done) " of " (number->string (length steps))
                                               " moves: " problem)
                                (loop (cdr steps) (+ done 1))))))))
      (refresh-all!)
      (message! s (or failed
                      (string-append "Renamed " (number->string (length renames))
                                     (if (null? warnings) "" (string-append "; " (car warnings)))))))))

;; The renames (from to) of names in one directory, as steps that never
;; move onto a name still to be moved away: a cycle (a to b and b to a)
;; goes through a name (TEMP name) that is free.
(define (rename-steps renames temp)
  (let loop ((pending renames) (acc '()))
    (cond ((null? pending) (reverse acc))
          ((find (lambda (r) (not (member (cadr r) (map car pending)))) pending)
           => (lambda (r) (loop (remove (lambda (x) (eq? x r)) pending) (cons r acc))))
          (else (let* ((r (car pending)) (t (temp (car r))))
                  (loop (cons (list t (cadr r)) (cdr pending)) (cons (list (car r) t) acc)))))))

;; A name for NAME in DIR that no entry has, nor is one of RESERVED.
(define (free-name dir name reserved)
  (let loop ((i 0))
    (let ((t (string-append ".wdired-" (number->string i) "-" name)))
      (if (or (member t reserved) (file-attributes (string-append dir t))) (loop (+ i 1)) t))))

(define-command (dired-abort-edit s n)
  "Stop editing names, leaving the entries as they were."
  (end-edit! s (editing s))
  (message! s "Names left as they were"))

;;; Targets: a directory is listed; files have their actions already.

(define-action directory (visit-directory s path) "List the directory." (dired! s path))
(define-action directory (visit-directory-other-pane s path)
  "List the directory in a new pane below."
  (show-in-other-pane! s (doc s))
  (dired! s path))
(define-action directory (copy-directory-name s path) "Save the directory's name as a kill." (kill-save! s path #f #f))
(define-action file (list-file-directory s path)
  "List the file's directory, at the file."
  (dired! s (directory-part path) #:select (base-name path)))

(define (directory-part path) (substring path 0 (- (string-length path) (string-length (base-name path)))))

;;; Dirvish: the parent directory, the directory and a preview of the
;;; entry at point, side by side. The session's `dirvish` is the panes,
;;; focus and tiling to put back, while it is shown.

(define (dirvish? s) (and (sget s 'dirvish) #t))

(define-command (dirvish s n)
  "Show this buffer's directory as dirvish does, in three panes.
The parent directory is on the left, a preview of the entry at point on
the right, following the caret. Opening a file or
\\[dired-quit] puts the panes back as they were."
  (let* ((path (document-path (doc s)))
         (b (session-buffer s))
         (select (if (dired? b) (listing-key-at b (point s)) (and path (base-name path))))
         (dir (default-directory s)))
    (unless (dirvish? s) (dirvish-on! s))
    (dired! s dir #:select select)
    (let ((v (session-view s)))
      (set-session-panes! s (list (view-split v) v (view-split v)) 0)
      (show-buffer! s (parent-buffer s) #:remember #f)
      (sset! s 'focus 2)
      (show-buffer! s (preview-buffer s) #:remember #f)
      (sset! s 'focus 1)
      (set-session-tree! s '(split right ((0.2 . 0) (0.35 . 1) (0.45 . 2))))
      (sset! s 'dirvish-shown #f)
      (dirvish-update! s))))

(define (dirvish-quit! s)
  (let ((saved (sget s 'dirvish)))
    (when saved
      (dirvish-off! s)
      (set-session-panes! s (car saved) (cadr saved))
      (set-session-tree! s (caddr saved))
      (for-each (lambda (b) (when (memq b (buffer-list)) (drop-buffer! s b)))
                (or (sget s 'dirvish-buffers) '()))
      (sset! s 'dirvish-buffers #f))))

(define-command (dired-quit s n)
  "Leave this listing; in dirvish, put the panes back as they were."
  (if (dirvish? s) (dirvish-quit! s) (run-command s 'quit-window 1)))

;; The parent and preview buffers of session S, its own: each attached
;; session shows a directory in dirvish its own way.
(define (dirvish-buffers s)
  (let ((own (sget s 'dirvish-buffers)))
    (if (and own (every (lambda (b) (memq b (buffer-list))) own))
        own
        (let ((own (list (make-listing-buffer! (unused-name "*dirvish parent*") 'dired-mode dired-rows
                                               #:state (list (cons 'dir "/") (cons 'details #f)))
                         (let ((d (make-document "")))
                           (document-limit-history! d 2)
                           (let ((b (make-generated-buffer! (unused-name "*dirvish preview*") d 'fundamental-mode)))
                             (set-option! 'read-only #t #:buffer b)
                             (set-option! 'line-numbers #f #:buffer b)
                             b)))))
          (sset! s 'dirvish-buffers own)
          own))))

(define (parent-buffer s) (car (dirvish-buffers s)))
(define (preview-buffer s) (cadr (dirvish-buffers s)))

;; NAME, or NAME<2>, <3>... when a buffer has it.
(define (unused-name name)
  (let loop ((i 1))
    (let ((n (if (= i 1) name (string-append name "<" (number->string i) ">"))))
      (if (buffer-named n) (loop (+ i 1)) n))))

;; What the preview of PATH shows, and the mode it is shown in.
(define (preview path)
  (cond ((string-suffix? "/" path)
         (cons (guard (e (#t (error-text e))) (string-join (directory-list path) "\n")) 'fundamental-mode))
        ((eq? 'other (car (or (guard (e (#t #f)) (file-attributes path)) '(file))))
         (cons "Not a regular file" 'fundamental-mode))
        (else
         (let ((text (guard (e (#t (error-text e))) (file-head path (option #f 'dired-preview-bytes)))))
           (if (string-index text #\null)
               (cons (string-append "Binary, " (human-size (entry-size (cons path (file-attributes path))))) 'fundamental-mode)
               (cons text (mode-for-file path)))))))

(define (show-preview! s view path)
  (let* ((b (preview-buffer s)) (d (buffer-document b)) (p (preview path)))
    (view-edit! (make-view d "dirvish") (list (list 0 (document-length d) (car p))) "new")
    (set-buffer-mode! b (cdr p))
    (view-set-ranges! view (list (list 0 0)) 0)
    (view-set-scroll! view 0)))

;; Make the parent pane and the preview follow the middle pane.
(define (dirvish-update! s)
  (let ((panes (session-panes s)))
    (if (not (= (length panes) 3))
        (dirvish-off! s)
        (let* ((mid (list-ref panes 1)) (b (document-buffer (view-document mid))))
          (when (dired? b)
            (let* ((dir (listing-dir b))
                   (pb (parent-buffer s))
                   (pv (list-ref panes 0))
                   (t (target-at (view-document mid) (cadr (list-ref (view-ranges mid) (view-primary mid)))))
                   (path (and t (target-value t))))
              (unless (equal? (listing-get pb 'child) dir)
                (listing-set! pb 'child dir)
                (listing-set! pb 'dir (normal-directory (string-append dir "..")))
                (listing-redisplay! pb)
                (let ((at (or (name-position pb (base-name (without-slash dir) #:slash #t)) 0)))
                  (view-set-ranges! pv (list (list at at)) 0)))
              (unless (equal? path (sget s 'dirvish-shown))
                (sset! s 'dirvish-shown path)
                (when path (show-preview! s (list-ref panes 2) path)))))))))

;; The sessions showing dirvish. The hook following their carets is
;; added only while one does: it would run on every key.
(define %dirvish-sessions '())

(define (dirvish-on! s)
  (sset! s 'dirvish (list (session-panes s) (session-focus s) (session-tree s)))
  (set! %dirvish-sessions (cons s (remove (lambda (x) (eq? x s)) %dirvish-sessions)))
  (add-hook! 'after-key 'dirvish
             (lambda (s key)
               (when (and (dirvish? s) (not (sget s 'minibuffer)))
                 (dirvish-update! s)))))

(define (dirvish-off! s)
  (sset! s 'dirvish #f)
  (set! %dirvish-sessions (filter dirvish? %dirvish-sessions))
  (when (null? %dirvish-sessions) (remove-hook! 'after-key 'dirvish)))

;; A new generation takes over the listings shown.
(for-each (lambda (b) (make-listing-buffer! (buffer-name b) 'dired-mode dired-rows)) (dired-buffers))

(define-key! emacs-map "C-x d" 'dired)
(define-key! emacs-map "C-x C-j" 'dired-jump)
(define-key! emacs-map "C-c o -" 'dired-jump)
(define-key! emacs-map "C-c o v" 'dirvish)
(name-prefix! emacs-map "C-c o" "open")
(define-key! modal-map "SPC o -" 'dired-jump)
(define-key! modal-map "SPC o v" 'dirvish)
(define-key! modal-map "SPC f d" 'dired)
(name-prefix! modal-map "SPC o" "open")
