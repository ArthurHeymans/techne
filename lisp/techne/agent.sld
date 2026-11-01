;;; Compact repository questions for trusted agents. Files, snapshots and
;;; processes stay in the world; only bounded answers cross the connection.
(define-library (techne agent)
  (export agent-project agent-session-project agent-root agent-files agent-read agent-expand
          agent-search agent-symbols agent-read-symbol agent-edit! agent-refresh!
          agent-discover agent-describe agent-ref agent-batch
          agent-run agent-process-result agent-process-done? agent-process-cancel!)
  (import (techne))
  (begin
    (define (agent-ref reply key)
      "Return KEY's value in the structured REPLY, or #f if absent."
      (let ((entry (assq key reply))) (and entry (cadr entry))))

    (define (agent-project root)
      "Open ROOT as a repository workspace for bounded queries and edits.
File lists respect ignore rules. Snapshots live in a 16 MiB, 128-entry
cache per project; an evicted handle expires. This is not a filesystem sandbox."
      (%agent-open root))

    (define %projects (quote ()))

    (define (agent-session-project root)
      "Return the project for ROOT retained in this world's session.
Repeated calls share its cache; the eight most recent roots are retained.
Those caches hold at most 128 MiB of source and 1024 snapshots in total.
Use `agent-project` for an independently owned project, or `agent-refresh!`
to expire the shared snapshots."
      (let* ((key (%agent-canonical-root root))
             (existing (assoc key %projects))
             (chosen (if existing (cdr existing) (agent-project key)))
             (rest (remove (lambda (entry) (string=? key (car entry))) %projects)))
        (set! %projects (cons (cons key chosen) (take rest (min 7 (length rest)))))
        chosen))

    (define (agent-root project)
      "Return the canonical root directory of PROJECT."
      (%agent-root project))

    (define (agent-files project #:prefix [prefix ""] #:limit [limit 20] #:offset [offset 0])
      "List PROJECT's file names starting with PREFIX, with bounded output.
Return LIMIT names at most, from OFFSET, and total, complete and next.
LIMIT is 1 to 50. The revision hashes the cached file-name list, not
file contents. Skipped and problems describe incomplete traversal. Call
`agent-refresh!` to relist after files change."
      (%agent-files project prefix limit offset))

    (define (agent-read project path #:from [from 1] #:lines [lines 40] #:chars [chars 8000])
      "Read a bounded region of PATH in PROJECT, starting at line FROM.
Read LINES lines at most, 1 to 200; return CHARS characters at most,
1 to 16000. Lines are 1-based: a trailing newline ends the final line,
not an extra empty line. An empty file has line 1. The reply names its
revision and expandable snapshot handle.
Use `agent-expand` with the returned next offset for clipped text."
      (%agent-read project path from lines chars))

    (define (agent-expand project handle #:offset [offset 0] #:chars [chars 8000])
      "Read CHARS characters of PROJECT's snapshot HANDLE from OFFSET.
CHARS is 1 to 16000. OFFSET and next count Unicode characters, not bytes.
A handle is immutable: it still reads the old text after a file changes.
Characters gives the full length of the selected handle, even when this
reply is clipped. An evicted, refreshed or foreign handle is an error;
query again."
      (%agent-expand project handle offset chars))

    (define (agent-search project query #:prefix [prefix ""] #:limit [limit 8] #:offset [offset 0])
      "Find lines containing literal QUERY in PROJECT, bounded by LIMIT.
Search files whose names start with PREFIX. Return at most LIMIT items,
1 to 50, from OFFSET, with paths, revisions, lines and expandable handles.
Previews center on the match and contain at most 240 characters; inspect
preview-truncated and preview-offset for clipping, and column for the
1-based Unicode match column. Total
counts matching lines, not occurrences. Complete is false if files were
skipped or a scan limit was reached; inspect skipped and problems.
Binary files are excluded from text search and counted separately."
      (%agent-search project query prefix limit offset))

    (define (agent-symbols project path #:name [name ""] #:limit [limit 10] #:offset [offset 0])
      "List definitions of PATH in PROJECT with source spans and handles.
NAME selects an exact name, or all names when empty. Return LIMIT items
at most, 1 to 50, from OFFSET. Scope distinguishes module, member, local
and library definitions. Line and end-line are 1-based and inclusive.
Rust spans include attached outer attributes and documentation comments.
Complete covers syntactic enumeration, not generated runtime bindings or
conditional-compilation semantics. Scheme uses the reader; Rust uses its
syntax tree. Unsupported languages are an error, not guessed symbols."
      (%agent-symbols project path name limit offset))

    (define (agent-read-symbol project path name #:chars [chars 8000])
      "Read the single definition NAME in PATH of PROJECT, within CHARS.
CHARS is 1 to 16000. An absent or ambiguous name is an error: use
`agent-symbols` to choose a handle when names are repeated."
      (let* ((reply (agent-symbols project path #:name name #:limit 2))
             (items (agent-ref reply 'items)))
        (unless (= 1 (agent-ref reply 'total))
          (error "agent-read-symbol: expected one definition; use agent-symbols" name (agent-ref reply 'total)))
        (cons (list (quote complete) (agent-ref reply (quote complete)))
              (agent-expand project (agent-ref (car items) (quote handle)) #:chars chars))))

    (define (agent-edit! project path revision edits)
      "Apply checked replacements EDITS to PATH in PROJECT at REVISION.
EDITS is a list of (old new) strings. Every old text must occur exactly
once in the original file, and replacements must not overlap. Validate
all edits before writing; stale or invalid requests change nothing.
Return the new revision and bounded change summaries. Writes use a
conditional atomic rename, not a lock against other programs."
      (%agent-edit! project path revision edits))

    (define (agent-refresh! project)
      "Forget PROJECT's file list and cached snapshots, expiring handles."
      (%agent-refresh! project)
      #t)

    (define (%count n max who)
      (unless (and (integer? n) (exact? n) (<= 1 n max))
        (error who "count out of range" n)))
    (define (%offset n)
      (unless (and (integer? n) (exact? n) (>= n 0))
        (error "agent: offset must be a non-negative integer")))
    (define (%clip s n)
      (substring s 0 (min n (string-length s))))
    (define (%binding description key)
      (let ((entry (assq key description))) (and entry (cdr entry))))
    (define (%first-line s)
      (car (string-split s "\n")))
    (define (%page all limit offset)
      (let* ((total (length all)) (rest (list-tail all (min total offset)))
             (items (take rest (min limit (length rest)))) (next (+ offset (length items))))
        `((items ,items) (total ,total) (complete #t)
          (truncated ,(< next total)) (next ,(and (< next total) next)))))

    (define (agent-discover query #:module [module "user"] #:limit [limit 6] #:offset [offset 0])
      "Find live bindings matching every word of QUERY, within LIMIT.
Inspect MODULE's visible bindings and loaded definitions, not a separate
registry. Prefer name matches; return signatures, short docs and source
locations. LIMIT is 1 to 50; OFFSET pages the results. Discovery is a live
view, not a stable snapshot. Lexical variables inside a CLI form are not
module bindings. Use `agent-describe` for full docs or signatures; a
clipped signature has an explicit signature-truncated flag."
      (%count limit 50 "agent-discover") (%offset offset)
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
                                        (string<? (symbol->string (agent-ref (cadr a) 'name))
                                                  (symbol->string (agent-ref (cadr b) 'name)))
                                        (> (car a) (car b)))))))
        (%page (map cadr sorted) limit offset)))

    (define (agent-describe name #:module [module "user"] #:chars [chars 4000] #:offset [offset 0] #:part [part (quote doc)])
      "Describe NAME in MODULE, with a bounded docstring or signature.
Return CHARS characters at most, 1 to 16000, from OFFSET of PART, `doc`
or `signature`. Text is the selected part; doc remains the docstring
window by default. Signature metadata is a preview with its own explicit
signature-truncated flag. Next and truncated describe the selected part."
      (%count chars 16000 "agent-describe") (%offset offset)
      (unless (memq part (quote (doc signature))) (error "agent-describe: expected doc or signature part" part))
      (let ((description (binding-description name module)))
        (unless description (error "agent-describe: unknown binding" name))
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

    (define (agent-batch thunks)
      "Call each of THUNKS, returning separate success or error replies.
Batch up to 20 bounded reads, regions, expansions or other data queries
in one request. A failure does not discard other answers. Each thunk
must take no arguments; successful values should themselves be bounded."
      (unless (and (list? thunks) (<= (length thunks) 20))
        (error "agent-batch: expected at most 20 thunks"))
      (map (lambda (thunk)
             (guard (e (#t (let ((message (condition/report-string e)))
                            `((ok #f) (error ,(%clip message 1600))
                              (error-truncated ,(> (string-length message) 1600))))))
               `((ok #t) (value ,(thunk)))))
           thunks))

    ;; Each stream keeps only its newest bounded text, while always draining
    ;; the process. A full stderr pipe must never deadlock a stdout reader.
    (define-record-type agent-job
      (%job program process out err stdout stderr wait)
      agent-job?
      (program %job-program) (process %job-process)
      (out %job-out) (err %job-err)
      (stdout %job-stdout) (stderr %job-stderr) (wait %job-wait))

    (define (%drain p stream state retain)
      (let loop ()
        (let ((chunk (process-read p stream)))
          (unless (eof-object? chunk)
            (let* ((joined (string-append (vector-ref state 0) chunk)) (n (string-length joined)))
              (vector-set! state 0 (substring joined (max 0 (- n retain)) n))
              (vector-set! state 1 (+ (vector-ref state 1) (string-length chunk))))
            (loop)))))

    (define (agent-run project program args #:retain [retain 4000] #:environment [environment '()])
      "Start PROGRAM with ARGS in PROJECT and keep bounded output tails.
RETAIN is 1 to 16000 characters per stream. Both streams drain
concurrently, even after their retained text is full. The current scope
owns the process. Wait with `agent-process-result`, or cancel explicitly;
this does not run a shell unless PROGRAM is a shell. ENVIRONMENT is a
list of (name . value) strings added to the inherited environment."
      (%count retain 16000 "agent-run")
      (unless (and (list? environment)
                   (every (lambda (entry)
                            (and (pair? entry) (string? (car entry)) (string? (cdr entry))
                                 (not (string=? (car entry) ""))
                                 (not (string-contains (car entry) "="))
                                 (not (string-contains (car entry) (string (integer->char 0))))
                                 (not (string-contains (cdr entry) (string (integer->char 0))))))
                          environment))
        (error "agent-run: environment must be a list of (name . value) strings; names must be nonempty without =, and strings without NUL"))
      (let* ((p (process-spawn program args #:directory (agent-root project) #:environment environment))
             (out (vector "" 0)) (err (vector "" 0))
             (stdout (spawn (lambda () (%drain p 'stdout out retain))))
             (stderr (spawn (lambda () (%drain p 'stderr err retain))))
             (wait (spawn (lambda () (process-wait p)))))
        (%job program p out err stdout stderr wait)))

    (define (agent-process-done? job)
      "Return whether JOB has exited and both output streams have drained."
      (and (task-done? (%job-wait job)) (task-done? (%job-stdout job)) (task-done? (%job-stderr job))))

    (define (agent-process-result job #:chars [chars 2000])
      "Wait for JOB and return its status and bounded stdout and stderr.
CHARS is 1 to 8000 characters per stream. Characters counts all text
seen; retained counts the saved tail. Dropped reports retention loss,
clipped a smaller report window, and truncated either kind of omission.
This is a tail, not parsed diagnostics. Repeated reads are safe."
      (define (stream state)
        (let* ((s (vector-ref state 0)) (n (string-length s)) (seen (vector-ref state 1)))
          `((text ,(substring s (max 0 (- n chars)) n))
            (characters ,seen) (retained ,n) (dropped ,(> seen n))
            (clipped ,(> n chars)) (truncated ,(> seen (min n chars))))))
      (%count chars 8000 "agent-process-result")
      (let ((status (task-join (%job-wait job))))
        (task-join (%job-stdout job)) (task-join (%job-stderr job))
        `((program ,(%job-program job)) (status ,status)
          (stdout ,(stream (%job-out job))) (stderr ,(stream (%job-err job))))))

    (define (agent-process-cancel! job)
      "Kill JOB's process group and drain the remaining bounded output."
      (process-kill (%job-process job))
      (agent-process-result job))))
