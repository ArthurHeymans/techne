;;; Compact repository questions for trusted agents. Files, snapshots and
;;; processes stay in the world; only bounded answers cross the connection.
(define-library (techne tools)
  (export tool-project tool-session-project tool-root tool-files tool-read tool-expand
          tool-search tool-symbols tool-read-symbol tool-edit! tool-refresh!
          tool-discover tool-describe tool-ref tool-batch tool-help
          tool-run tool-process-result tool-process-done? tool-process-cancel!
          tool-find tool-lines tool-apply! tool-show tool-stamp tool-check)
  (import (techne) (techne languages))
  (begin
    (define (tool-ref reply key)
      "Return KEY's value in the structured REPLY, or #f if absent."
      (let ((entry (assq key reply))) (and entry (cadr entry))))

    (define (tool-project root)
      "Open ROOT as a repository workspace for bounded queries and edits.
File lists respect ignore rules. Snapshots live in a 16 MiB, 128-entry
cache per project; an evicted handle expires. This is not a filesystem sandbox."
      (%tool-open root))

    (define %projects (quote ()))

    (define (tool-session-project root)
      "Return the project for ROOT retained in this world's session.
Repeated calls share its cache; the eight most recent roots are retained.
Those caches hold at most 128 MiB of source and 1024 snapshots in total.
Use `tool-project` for an independently owned project, or `tool-refresh!`
to expire the shared snapshots."
      (let* ((key (%repository-canonical-root root))
             (existing (assoc key %projects))
             (chosen (if existing (cdr existing) (tool-project key)))
             (rest (remove (lambda (entry) (string=? key (car entry))) %projects)))
        (set! %projects (cons (cons key chosen) (take rest (min 7 (length rest)))))
        chosen))

    (define (tool-root project)
      "Return the canonical root directory of PROJECT."
      (%tool-root project))

    (define (tool-files project #:prefix [prefix ""] #:limit [limit 20] #:offset [offset 0])
      "List PROJECT's file names starting with PREFIX, with bounded output.
Hidden files are listed (`.github/`), version control metadata (`.git`,
`.jj`, `.hg`, `.svn`) never; ignore rules apply, so put secrets and
generated directories not otherwise ignored (`.env`, `.direnv/`) in
`.ignore`. At most 10000 files are walked, before PREFIX selects.
Return LIMIT names at most, from OFFSET, and total, complete and next.
LIMIT is 1 to 50. The revision hashes the cached file-name list, not
file contents. Skipped and problems describe incomplete traversal. Call
`tool-refresh!` to relist after files change."
      (%response (%tool-files project prefix limit offset)))

    (define (tool-read project path #:from [from 1] #:lines [lines 40] #:chars [chars 8000])
      "Read a bounded region of PATH in PROJECT, starting at line FROM.
Read LINES lines at most, 1 to 200; return CHARS characters at most,
1 to 16000. Lines are 1-based: a trailing newline ends the final line,
not an extra empty line. An empty file has line 1. The reply names its
revision and expandable snapshot handle.
Use `tool-expand` with the returned next offset for clipped text."
      (%response (%tool-read project path from lines chars)))

    (define (tool-expand project handle #:offset [offset 0] #:chars [chars 8000])
      "Read CHARS characters of PROJECT's snapshot HANDLE from OFFSET.
CHARS is 1 to 16000. OFFSET and next count Unicode characters, not bytes.
A handle is immutable: it still reads the old text after a file changes.
Characters gives the full length of the selected handle, even when this
reply is clipped. An evicted, refreshed or foreign handle is an error;
query again."
      (%response (%tool-expand project handle offset chars)))

    (define (tool-search project query #:prefix [prefix ""] #:limit [limit 8] #:offset [offset 0])
      "Find lines containing literal QUERY in PROJECT, bounded by LIMIT.
Search files whose names start with PREFIX. Return at most LIMIT items,
1 to 50, from OFFSET, with paths, revisions, lines and expandable handles.
Previews center on the match and contain at most 240 characters; inspect
preview-truncated and preview-offset for clipping, and column for the
1-based Unicode match column. Total
counts matching lines, not occurrences. Complete is false if files were
skipped or a scan limit was reached; inspect skipped and problems.
Binary files are excluded from text search and counted separately."
      (%response (%tool-search project query prefix limit offset)))

    (define (tool-symbols project path #:name [name ""] #:limit [limit 10] #:offset [offset 0])
      "List definitions of PATH in PROJECT with source spans and handles.
NAME selects an exact name, or all names when empty. Return LIMIT items
at most, 1 to 50, from OFFSET. Scope distinguishes module, member, local
and library definitions. Line and end-line are 1-based and inclusive.
Rust spans include attached outer attributes and documentation comments.
Complete covers syntactic enumeration, not generated runtime bindings or
conditional-compilation semantics. Scheme uses the exact reader.
Other source languages reuse the editor language declarations and
definition queries. Unsupported languages are an error, not guessed symbols."
      (%response (%tool-symbols project path name limit offset)))

    (define (tool-read-symbol project path name #:chars [chars 8000])
      "Read the single definition NAME in PATH of PROJECT, within CHARS.
CHARS is 1 to 16000. An absent or ambiguous name is an error: use
`tool-symbols` to choose a handle when names are repeated."
      (let* ((reply (tool-symbols project path #:name name #:limit 2))
             (items (tool-ref reply 'items)))
        (unless (= 1 (tool-ref reply 'total))
          (error "tool-read-symbol: expected one definition; use tool-symbols" name (tool-ref reply 'total)))
        (cons (list (quote complete) (tool-ref reply (quote complete)))
              (tool-expand project (tool-ref (car items) (quote handle)) #:chars chars))))

    (define (tool-edit! project path revision edits)
      "Apply checked replacements EDITS to PATH in PROJECT at REVISION.
EDITS is a list of (old new) strings. Every old text must occur exactly
once in the original file, and replacements must not overlap. Validate
all edits before writing; stale or invalid requests change nothing.
Return the new revision and bounded change summaries. Writes use a
conditional atomic rename, not a lock against other programs."
      (%response (%tool-edit! project path revision edits)))

    (define (tool-refresh! project)
      "Forget PROJECT's file list and cached snapshots, expiring handles."
      (%tool-refresh! project)
      #t)

    (define (%count n max who)
      (unless (and (integer? n) (exact? n) (<= 1 n max))
        (error who "count out of range" n)))
    (define (%offset n)
      (unless (and (integer? n) (exact? n) (>= n 0))
        (error "tool: offset must be a non-negative integer")))
    (define (%clip s n)
      (substring s 0 (min n (string-length s))))
    (define (%binding description key)
      (let ((entry (assq key description))) (and entry (cdr entry))))
    (define (%first-line s)
      (car (string-split s "\n")))
    (define (%page all limit offset #:complete [complete #t])
      (let* ((total (length all)) (rest (list-tail all (min total offset)))
             (items (take rest (min limit (length rest)))) (next (+ offset (length items))))
        `((items ,items) (total ,total) (complete ,complete)
          (truncated ,(or (< next total) (not complete))) (next ,(and (< next total) next)))))

    (define (tool-discover query #:module [module "user"] #:limit [limit 6] #:offset [offset 0])
      "Find live bindings matching every word of QUERY, within LIMIT.
Inspect MODULE's visible bindings and loaded definitions, not a separate
registry. Prefer name matches; return signatures, short docs and source
locations. LIMIT is 1 to 50; OFFSET pages the results. Discovery is a live
view, not a stable snapshot. Lexical variables inside a CLI form are not
module bindings. Use `tool-describe` for full docs or signatures; a
clipped signature has an explicit signature-truncated flag."
      (%count limit 50 "tool-discover") (%offset offset)
      (let* ((words (remove (lambda (s) (string=? s "")) (string-split (string-downcase query) " ")))
             (matches
              (filter-map
               (lambda (binding)
                 (let* ((name (car binding)) (home (cadr binding))
                        (description (binding-description name home))
                        (signature (or (%binding description 'signature) ""))
                        (doc (or (%binding description 'doc) ""))
                        (label (symbol->string name))
                        (hay (string-downcase (string-append label " " signature " " doc))))
                   (and (every (lambda (w) (string-contains hay w)) words)
                        (list (+ (if (string=? (string-downcase label) (string-downcase query)) 100 0)
                                 (* 4 (count (lambda (w) (string-contains (string-downcase label) w)) words)))
                              `((name ,name) (module ,home) (kind ,(caddr binding))
                                (signature ,(%clip signature 240))
                                (signature-truncated ,(> (string-length signature) 240))
                                (doc ,(%clip (%first-line doc) 160))
                                (location ,(%binding description 'location)))))))
               (defined-names module)))
             (sorted (sort matches (lambda (a b)
                                    (if (= (car a) (car b))
                                        (string<? (symbol->string (tool-ref (cadr a) 'name))
                                                  (symbol->string (tool-ref (cadr b) 'name)))
                                        (> (car a) (car b)))))))
        (%page (map cadr sorted) limit offset)))

    (define (tool-describe name #:module [module "user"] #:chars [chars 4000] #:offset [offset 0] #:part [part (quote doc)])
      "Describe NAME in MODULE, with a bounded docstring or signature.
Return CHARS characters at most, 1 to 16000, from OFFSET of PART, `doc`
or `signature`. Text is the selected part; doc remains the docstring
window by default. Signature metadata is a preview with its own explicit
signature-truncated flag. Next and truncated describe the selected part."
      (%count chars 16000 "tool-describe") (%offset offset)
      (unless (memq part (quote (doc signature))) (error "tool-describe: expected doc or signature part" part))
      (let ((description (binding-description name module)))
        (unless description (error "tool-describe: unknown binding" name))
        (let* ((doc (or (%binding description (quote doc)) ""))
               (signature (or (%binding description (quote signature)) ""))
               (selected (if (eq? part (quote signature)) signature doc))
               (n (string-length selected)) (start (min offset n)) (end (min (+ start chars) n))
               (chunk (substring selected start end)))
          `((name ,name) (module ,module) (part ,part)
            (signature ,(%clip signature 240)) (signature-truncated ,(> (string-length signature) 240))
            (location ,(%binding description (quote location)))
            (doc ,(if (eq? part (quote doc)) chunk (%clip (%first-line doc) 160)))
            (text ,chunk) (truncated ,(< end n)) (next ,(and (< end n) end))))))

    (define (tool-batch thunks)
      "Call each of THUNKS, returning separate success or error replies.
Batch up to 20 bounded reads, regions, expansions or other data queries
in one request. A failure does not discard other answers. Each thunk
must take no arguments; successful values should themselves be bounded."
      (unless (and (list? thunks) (<= (length thunks) 20))
        (error "tool-batch: expected at most 20 thunks"))
      (map (lambda (thunk)
             (guard (e (#t (let ((message (condition/report-string e)))
                            `((ok #f) (error ,(%clip message 1600))
                              (error-truncated ,(> (string-length message) 1600))))))
               `((ok #t) (value ,(thunk)))))
           thunks))

    ;; Each stream keeps only its newest bounded text, while always draining
    ;; the process. A full stderr pipe must never deadlock a stdout reader.
    ;; With KEEP, a stream also keeps its first KEEP characters whole, as
    ;; chunks, made one snapshot when the job is first reported. With a
    ;; timeout, a watchdog kills it at its deadline, and says so.
    (define-record-type tool-job
      (%job program process out err stdout stderr wait project keep kept timed-out)
      tool-job?
      (program %job-program) (process %job-process)
      (out %job-out) (err %job-err)
      (stdout %job-stdout) (stderr %job-stderr) (wait %job-wait)
      (project %job-project) (keep %job-keep) (kept %job-kept %job-kept-set!)
      (timed-out %job-timed-out? %job-timed-out-set!))

    ;; Wait until (DONE?) or the DEADLINE in milliseconds, #f for none;
    ;; return whether it is done.
    (define (%wait-until done? deadline)
      (let loop ()
        (cond ((done?) #t)
              ((and deadline (>= (current-milliseconds) deadline)) #f)
              (else (sleep (if deadline (max 1 (min 50 (- deadline (current-milliseconds)))) 50)) (loop)))))

    (define (%seconds n who)
      (unless (and (real? n) (> n 0) (<= n 86400)) (error who "seconds must be more than 0, at most 86400" n))
      (exact (ceiling (* 1000 n))))

    ;; Both streams at their limit, four bytes a character, take half the
    ;; project's 16 MiB snapshot cache: storing one never evicts the other.
    (define %keep-limit (* 1024 1024))

    ;; STATE is #(tail seen chunks kept): the newest RETAIN characters, all
    ;; characters seen, and, with KEEP, the first KEEP in reverse chunks.
    (define (%drain p stream state retain keep)
      (let loop ()
        (let ((chunk (process-read p stream)))
          (unless (eof-object? chunk)
            (let* ((joined (string-append (vector-ref state 0) chunk)) (n (string-length joined)))
              (vector-set! state 0 (substring joined (max 0 (- n retain)) n))
              (vector-set! state 1 (+ (vector-ref state 1) (string-length chunk))))
            (when keep
              (let* ((kept (vector-ref state 3)) (room (- keep kept)))
                (when (> room 0)
                  (let ((part (%clip chunk room)))
                    (vector-set! state 2 (cons part (vector-ref state 2)))
                    (vector-set! state 3 (+ kept (string-length part)))))))
            (loop)))))

    (define (tool-run project program args #:retain [retain 4000] #:environment [environment '()] #:keep [keep #f]
                      #:timeout [timeout #f])
      "Start PROGRAM with ARGS in PROJECT and keep bounded output tails.
RETAIN is 1 to 16000 characters per stream. Both streams drain
concurrently, even after their retained text is full. The current scope
owns the process. Wait with `tool-process-result`, or cancel explicitly;
this does not run a shell unless PROGRAM is a shell. ENVIRONMENT is a
list of (name . value) strings added to the inherited environment.
KEEP, 1 to 1048576 characters, also keeps each stream's first KEEP
characters whole: the result then gives each stream a snapshot handle,
for `tool-find`, `tool-expand` and `tool-show`, in PROJECT's snapshot
cache (16 MiB, which both fit in; older handles are evicted first).
TIMEOUT, seconds up to a day, kills its process group if it has not
exited by then; its result says timed-out."
      (%count retain 16000 "tool-run")
      (when keep (%count keep %keep-limit "tool-run"))
      (when timeout (%seconds timeout "tool-run"))
      (unless (and (list? environment)
                   (every (lambda (entry)
                            (and (pair? entry) (string? (car entry)) (string? (cdr entry))
                                 (not (string=? (car entry) ""))
                                 (not (string-contains (car entry) "="))
                                 (not (string-contains (car entry) (string (integer->char 0))))
                                 (not (string-contains (cdr entry) (string (integer->char 0))))))
                          environment))
        (error "tool-run: environment must be a list of (name . value) strings; names must be nonempty without =, and strings without NUL"))
      (let* ((p (process-spawn program args #:directory (tool-root project) #:environment environment))
             (out (vector "" 0 '() 0)) (err (vector "" 0 '() 0))
             (stdout (spawn (lambda () (%drain p 'stdout out retain keep))))
             (stderr (spawn (lambda () (%drain p 'stderr err retain keep))))
             (wait (spawn (lambda () (process-wait p))))
             (job (%job program p out err stdout stderr wait project keep #f #f)))
        (when timeout
          (let ((deadline (+ (current-milliseconds) (%seconds timeout "tool-run"))))
            (spawn (lambda ()
                     ;; The job, not its leader: what it started holds its streams.
                     (unless (%wait-until (lambda () (tool-process-done? job)) deadline)
                       (%job-timed-out-set! job #t)
                       (process-kill p))))))
        job))

    (define (tool-process-done? job)
      "Return whether JOB has exited and both output streams have drained."
      (and (task-done? (%job-wait job)) (task-done? (%job-stdout job)) (task-done? (%job-stderr job))))

    (define (tool-process-result job #:chars [chars 2000] #:wait [wait #f])
      "Wait for JOB and return its status and bounded stdout and stderr.
CHARS is 1 to 8000 characters per stream. Characters counts all text
seen; retained counts the saved tail. Dropped reports retention loss,
clipped a smaller report window, and truncated either kind of omission.
This is a tail, not parsed diagnostics. Repeated reads are safe.
WAIT, seconds, waits at most that long: if JOB still runs, the result
has running #t, status #f and the output so far, and JOB goes on; ask
again, or cancel it. Timed-out says its own timeout killed it."
      (define (snapshot name state)
        (let* ((text (apply string-append (reverse (vector-ref state 2))))
               (s (%snapshot (%repository-nonce) (string-append "<" name " of " (%job-program job) ">")
                             (%repository-nonce) text (%utf8-size text))))
          (%keep! (%job-project job) s)
          (list (%handle (%job-project job) s 0 (string-length text)) (string-length text))))
      (define (stream state kept)
        (let* ((s (vector-ref state 0)) (n (string-length s)) (seen (vector-ref state 1)))
          `((text ,(substring s (max 0 (- n chars)) n))
            (characters ,seen) (retained ,n) (dropped ,(> seen n))
            (clipped ,(> n chars)) (truncated ,(> seen (min n chars)))
            ,@(if kept `((handle ,(car kept)) (kept ,(cadr kept))) '()))))
      (%count chars 8000 "tool-process-result")
      (if (and wait (not (%wait-until (lambda () (tool-process-done? job))
                                      (+ (current-milliseconds) (%seconds wait "tool-process-result")))))
          `((program ,(%job-program job)) (status #f) (running #t) (timed-out #f)
            (stdout ,(stream (%job-out job) #f)) (stderr ,(stream (%job-err job) #f)))
      (let ((status (task-join (%job-wait job))))
        (task-join (%job-stdout job)) (task-join (%job-stderr job))
        (when (and (%job-keep job) (not (%job-kept job)))
          (%job-kept-set! job (list (snapshot "stdout" (%job-out job)) (snapshot "stderr" (%job-err job)))))
        `((program ,(%job-program job)) (status ,status) (running #f) (timed-out ,(%job-timed-out? job))
          (stdout ,(stream (%job-out job) (and (%job-kept job) (car (%job-kept job)))))
          (stderr ,(stream (%job-err job) (and (%job-kept job) (cadr (%job-kept job)))))))))

    (define (tool-process-cancel! job)
      "Kill JOB's process group and drain the remaining bounded output."
      (process-kill (%job-process job))
      (tool-process-result job))

    (define (tool-find project handle query #:limit [limit 8] #:offset [offset 0])
      "Find lines containing literal QUERY in PROJECT's snapshot HANDLE.
HANDLE is any reply's: a file read, a search hit, or a process stream
kept with `tool-run` #:keep. Return at most LIMIT items, 1 to 50, from
OFFSET, with line numbers in the snapshot's file and a handle per line.
Unlike `tool-search`, this reads the snapshot, not the disk."
      (%response (%tool-find project handle query limit offset)))

    (define (tool-lines project handle #:from [from 1] #:lines [lines 40] #:chars [chars 8000])
      "Read LINES lines, 1 to 200, from line FROM of PROJECT's snapshot HANDLE.
Lines are numbered as in the snapshot's whole file or output, as
`tool-find` reports them, and stay within HANDLE's region. Return at
most CHARS characters, 1 to 16000, with an expandable handle."
      (%response (%tool-lines project handle from lines chars)))

    (define (tool-apply! project patch)
      "Apply PATCH, search/replace blocks, to files of PROJECT.
Each block is a file line, optionally FILE @REVISION (8 or more hex
characters of it), then <<<<<<< SEARCH, the old lines, =======, the new
lines, >>>>>>> REPLACE. A block without a file line uses the previous
one's file. Each old text must occur exactly once. An empty SEARCH
creates a new file, refused if it exists. Every file is checked before
any is written, but writes are not one transaction: a failure part way
names the files written. From the CLI, pass PATCH on stdin with
`techne tool --stdin ROOT (tool-apply! project input)`; nothing needs escaping."
      (%response (%tool-apply! project patch)))

    (define %check-patterns '("error:" "error[" "panicked at" "FAILED" "test result:" "warning: unused"))

    (define (tool-check project program args #:find [patterns %check-patterns] #:context [context 0] #:limit [limit 30]
                        #:timeout [timeout #f] #:environment [environment '()])
      "Run PROGRAM with ARGS in PROJECT; return the output's lines that matter.
It waits for the end. Those are the lines of either stream containing
any literal of PATTERNS, by default a compiler's errors and a test
run's failures and results, each once, stdout's first, in order, at
most LIMIT items, 1 to 50; CONTEXT, 0 to 40, adds that many lines
after each, a hit among the lines returned shown there. Total counts
the matching lines; truncated says some are in no item. The first
1048576 characters of each stream are kept and searched: lost and
complete #f say there was more. The result names both streams' handles
for `tool-find` and `tool-lines`, and their tails, which `tool-show`
shows when nothing matched and it failed. TIMEOUT and ENVIRONMENT are
`tool-run`'s."
      (%count limit 50 "tool-check")
      (unless (and (integer? context) (exact? context) (<= 0 context 40))
        (error "tool-check: context is 0 to 40 lines" context))
      (unless (and (pair? patterns) (list? patterns) (every (lambda (p) (and (string? p) (not (string=? p "")))) patterns))
        (error "tool-check: patterns are non-empty strings"))
      (let* ((r (tool-process-result
                  (tool-run project program args #:keep %keep-limit #:retain 1000 #:timeout timeout #:environment environment)
                  #:chars 1000))
             ;; Output past what was kept is not searched: say so.
             (lost (any (lambda (s) (> (tool-ref (tool-ref r s) 'characters) (tool-ref (tool-ref r s) 'kept))) '(stdout stderr)))
             ;; Every matching line of each stream, once, in order, one
             ;; pass each: (stream line . item).
             (matching
               (append-map
                 (lambda (stream)
                   (map (lambda (i) (cons stream (cons (tool-ref i 'line) i)))
                        (%matching-lines project (tool-ref (tool-ref r stream) 'handle) patterns)))
                 '(stdout stderr)))
             ;; Up to LIMIT items, each a hit with the lines after it, as
             ;; (item stream first last), LAST the last line it returned
             ;; whole; a hit among those is shown there, not again.
             (shown
               (let loop ((rest matching) (stream #f) (covered 0) (acc '()))
                 (cond ((or (null? rest) (= (length acc) limit)) (reverse acc))
                       ((and (eq? (car (car rest)) stream) (<= (cadr (car rest)) covered)) (loop (cdr rest) stream covered acc))
                       (else
                         (let* ((m (car rest)) (item (cddr m)) (line (cadr m))
                                (more (and (> context 0)
                                           (%tool-lines project (tool-ref (tool-ref r (car m)) 'handle) line (+ 1 context) 16000)))
                                ;; The lines returned whole.
                                (whole (if more
                                           (let ((text (tool-ref more 'text)))
                                             (+ (length (filter (lambda (c) (char=? c #\newline)) (string->list text)))
                                                (if (or (tool-ref more 'truncated) (string-suffix? "\n" text)) 0 1)))
                                           1)))
                           (loop (cdr rest) (car m) (+ line (max whole 1) -1)
                                 (cons (list (if more
                                                 (map (lambda (e)
                                                        (case (car e)
                                                          ((text) (list 'text (tool-ref more 'text)))
                                                          ((handle) (list 'handle (tool-ref more 'handle)))
                                                          (else e)))
                                                      (append item `((context-truncated ,(tool-ref more 'truncated)))))
                                                 item)
                                             (car m) line (+ line (max whole 1) -1))
                                       acc)))))))
             (items (map car shown))
             ;; Some matching line is in no item: what each returned whole.
             (more-hits (not (every (lambda (m)
                                      (any (lambda (s) (and (eq? (cadr s) (car m)) (<= (caddr s) (cadr m) (cadddr s)))) shown))
                                    matching))))
        (%response
          `((program ,program) (status ,(tool-ref r 'status)) (timed-out ,(tool-ref r 'timed-out))
            (hits ((items ,items) (total ,(length matching)) (complete ,(not lost))
                   (truncated ,(or more-hits lost)) (next #f)))
            (lost ,lost)
            (stdout ((handle ,(tool-ref (tool-ref r 'stdout) 'handle)) (characters ,(tool-ref (tool-ref r 'stdout) 'characters))
                     (tail ,(tool-ref (tool-ref r 'stdout) 'text))))
            (stderr ((handle ,(tool-ref (tool-ref r 'stderr) 'handle)) (characters ,(tool-ref (tool-ref r 'stderr) 'characters))
                     (tail ,(tool-ref (tool-ref r 'stderr) 'text))))))))



    ;; What code this world runs its tools with: its program, and every
    ;; library it could load, Techne's own and TECHNE_LIBRARY_PATH's, as
    ;; they were when it was loaded. Any change of them changes it.
    (define (%tools-stamp)
      (let* ((path (or (get-environment-variable "TECHNE_LIBRARY_PATH") ""))
             (dirs (cons (%lisp-directory) (remove (lambda (d) (string=? d "")) (string-split path ":")))))
        (string-join (cons (%program-stamp) (append-map (lambda (d) (list d (%tree-stamp d))) dirs)) " ")))
    ;; #f in a world not granted files, which has no sessions to compare.
    (define %loaded-stamp (guard (e (#t #f)) (%tools-stamp)))

    (define (tool-stamp #:now [now #f])
      "Return what code this world's tools run: the program and sources.
As they were when (techne tools) was loaded, or with NOW as they are on
disk now. `techne tool` compares a session's with its own, restarting
a session that runs older code."
      (if now (%tools-stamp) %loaded-stamp))

    (define (tool-show reply)
      "Return REPLY as plain text for reading, rather than as data.
Search and find hits become FILE:LINE: TEXT lines, definitions
FILE:LINE-END KIND NAME, reads their text with a note how to expand a
clipped one, process results their status and streams, batches each
answer. Lists show their items in turn; anything else is written.
From the CLI, `techne tool --text ROOT (tool-show FORM)` prints it as is."
      (%tool-show reply))
    (include "tools-core.scm")
    (include "tools-help.scm")
))
