;;; Opt-in tracing of module bindings. The wrappers, bounded log and
;;; lifecycle are Scheme; repr's byte bound and box-cas! are the small
;;; runtime mechanisms (heap printing and non-preemptible publication).
(define-library (techne debug)
  (export trace untrace traced-procedures
          make-trace-log trace-log? current-trace-log
          trace-log-events trace-log-truncated? clear-trace-log!)
  (import (techne))
  (begin
    (define-record-type trace-log
      (%make-trace-log limit port state)
      trace-log?
      (limit %log-limit)
      (port %log-port)
      ;; One immutable snapshot: #(count reverse-events truncated?).
      (state %log-state))

    (define (make-trace-log #:limit [limit 200] #:port [port 'current])
      "Return a trace log keeping LIMIT events, then suppressing more.
LIMIT is 1 to 1000. PORT is an output port, #f for no streaming, or
`current` for the output port of each call (including nREPL output).
Values are diagnostic text of at most 256 bytes, not retained objects.
After the limit, streaming prints one notice. Clear the log to rearm it."
      (unless (and (exact-integer? limit) (<= 1 limit 1000))
        (error "make-trace-log: limit must be from 1 to 1000"))
      (unless (or (eq? port 'current) (eq? port #f) (output-port? port))
        (error "make-trace-log: expected an output port, #f or current"))
      (%make-trace-log limit port (box (vector 0 '() #f))))

    (define current-trace-log
      (make-parameter (make-trace-log)
        (lambda (value)
          (unless (or (not value) (trace-log? value))
            (error "current-trace-log: expected a trace log or #f"))
          value))
      "Return the log for traced calls, a parameter; #f disables recording.
Tasks inherit it. A shared log bounds recording and streaming across tasks.")
    (define %busy (make-parameter #f))
    ;; Spawn inherits parameters; the task number prevents inheriting its
    ;; parent's nesting depth. Neither context nor events hold a task.
    (define %context (make-parameter '(0 . 0)))
    (define %tasks (make-weak-hash-table))
    (define %serial (box 0))
    (define %entries (box '()))
    ;; No value refers back to its wrapper key. Retired aliases can be
    ;; traced again without building chains of pass-through wrappers.
    (define %originals (make-weak-hash-table))

    (define (%next!)
      (let loop ()
        (let* ((old (unbox %serial)) (new (+ old 1)))
          (if (box-cas! %serial old new) new (loop)))))

    (define (%task-number)
      (let ((task (current-task)))
        (if task
            (or (hash-table-ref/default %tasks task #f)
                (let ((id (%next!))) (hash-table-set! %tasks task id) id))
            0)))

    (define (trace-log-events log)
      "Return LOG's events in recording order, as dicts of data.
Each has call, task, depth, phase, module, name and detail. CALL pairs
entry and exit; TASK is this world's weakly assigned task number.
PHASE is call, return, condition or unwind. DETAIL summarizes arguments,
returned values (as a list), or the raised condition. No value is live."
      (reverse (vector-ref (unbox (%log-state log)) 1)))

    (define (trace-log-truncated? log)
      "Return whether LOG has suppressed an event past its limit."
      (vector-ref (unbox (%log-state log)) 2))

    (define (clear-trace-log! log)
      "Discard LOG's events and rearm its limit; call numbers stay unique.
Calls already running may subsequently record their return or unwind."
      (set-box! (%log-state log) (vector 0 '() #f)))

    (define (%stream log text)
      ;; A broken output port must not disable the in-memory recorder.
      ;; Cancellation, interrupts and resource errors still propagate.
      (guard (e ((or (file-error? e)
                     (and (error-object? e)
                          (string-suffix? "not an open output port" (error-object-message e)))) #f)
                (#t (raise e)))
        (let ((port (%log-port log)))
          (when port
            (display text (if (eq? port 'current) (current-output-port) port))))))

    (define (%event-text event)
      ;; Put pairing information first, and print each already bounded
      ;; summary once rather than escaping it again as a dict's string.
      (string-append "; trace task " (number->string (dict-ref event 'task))
                     " call " (number->string (dict-ref event 'call))
                     " depth " (number->string (dict-ref event 'depth))
                     " phase " (symbol->string (dict-ref event 'phase))
                     " " (dict-ref event 'name) " " (dict-ref event 'detail)
                     " in " (dict-ref event 'module) "\n"))

    (define (%emit log call task depth phase entry value)
      ;; No user printer runs, and stops delivered during recording are
      ;; never consumed by an instrumentation handler.
      (parameterize ((%busy #t))
          (let* ((cell (%log-state log))
                 (event #f))
            (let loop ()
              (let* ((old (unbox cell)) (count (vector-ref old 0)))
                (cond
                  ((>= count (%log-limit log))
                   (unless (vector-ref old 2)
                     (if (box-cas! cell old (vector count (vector-ref old 1) #t))
                         (%stream log "; trace limit reached; clear-trace-log! to rearm\n")
                         (loop)))
                   #f)
                  (else
                   (unless event
                     (set! event (dict 'call call 'task task 'depth depth 'phase phase
                                       'module (repr (%entry-module entry) 160)
                                       'name (repr (%entry-name entry) 96)
                                       'detail (repr value 256))))
                   (if (box-cas! cell old (vector (+ count 1) (cons event (vector-ref old 1)) #f))
                       (begin (%stream log (%event-text event)) #t)
                       (loop)))))))))

    (define-record-type trace-entry
      (%make-entry key module name original getter swap state)
      %entry?
      (key %entry-key)
      (module %entry-module)
      (name %entry-name)
      (original %entry-original)
      (getter %entry-getter)
      ;; The hygienic compare/set has no call or back edge between its
      ;; comparison and write, so task preemption cannot split them.
      (swap %entry-swap)
      (wrapper %entry-wrapper %set-wrapper!)
      (state %entry-state))

    (define (%entry-active? entry)
      (eq? (unbox (%entry-state entry)) 'active))

    (define (%call entry log args task depth)
      (let ((id (%next!)) (returned #f))
        ;; Establish cleanup before publishing the call: cancellation may
        ;; arrive while its entry is being streamed, after publication.
        (dynamic-wind
          (lambda () #f)
          (lambda ()
            (if (not (%emit log id task depth 'call entry args))
                (begin (set! returned #t) (apply (%entry-original entry) args))
                (parameterize ((%context (cons task (+ depth 1))))
                  (call-with-values
                    (lambda ()
                      (with-exception-handler
                        (lambda (condition)
                          (%emit log id task depth 'condition entry condition)
                          ;; Preserve continuable raises and live restarts.
                          (raise-continuable condition))
                        (lambda () (apply (%entry-original entry) args))))
                    (lambda results
                      (%emit log id task depth 'return entry results)
                      (set! returned #t)
                      (apply values results))))))
          (lambda ()
            ;; A local flag set after emit would miss cancellation between
            ;; the log's CAS and that assignment. Inspect the bounded log
            ;; for this call's committed entry instead; clear/limit may
            ;; legitimately remove or suppress either side of a pair.
            (when (and (not returned)
                       (any (lambda (event)
                              (and (= id (dict-ref event 'call))
                                   (eq? 'call (dict-ref event 'phase))))
                            (trace-log-events log)))
              (%emit log id task depth 'unwind entry #f))))))

    (define (%wrapper entry)
      (lambda args
        (let ((log (current-trace-log)))
          (if (or (not (%entry-active? entry)) (%busy) (not log)
                  (vector-ref (unbox (%log-state log)) 2))
              (apply (%entry-original entry) args)
              (let* ((task (%task-number)) (context (%context))
                     (depth (if (= task (car context)) (cdr context) 0)))
                ;; Collecting returns costs a frame. Beyond 32 levels, or
                ;; once full, tail-call the original without instrumentation.
                (if (>= depth 32)
                    (apply (%entry-original entry) args)
                    (%call entry log args task depth)))))))

    (define (%same-binding? entry identity)
      (= (%entry-key entry) (car identity)))

    (define (%retire! entry)
      ;; Saved aliases of a wrapper become pass-through too. A definition
      ;; installed since tracing is never overwritten by untrace.
      (set-box! (%entry-state entry) 'retired)
      ((%entry-swap entry) (%entry-wrapper entry) (%entry-original entry)))

    (define (%discard! entry)
      (%retire! entry)
      (let loop ()
        (let ((before (unbox %entries)))
          (unless (box-cas! %entries before (remove (lambda (e) (eq? e entry)) before))
            (loop)))))

    (define (%install! identity getter swap)
      (let loop ()
        (let* ((module (cadr identity)) (name (caddr identity))
               (before (unbox %entries))
               (previous (find (lambda (e) (%same-binding? e identity)) before))
               (value (getter)))
          (cond
            ((and previous (%entry-active? previous) (eq? value (%entry-wrapper previous))) name)
            (else
             (unless (procedure? value) (error "trace: expected a procedure" name))
             (when (and (not previous) (>= (length before) 64))
               (error "trace: at most 64 bindings; untrace one first"))
             (let* ((original (hash-table-ref/default %originals value value))
                    (entry (%make-entry (car identity) module name original getter swap (box 'installing)))
                    (wrapper (%wrapper entry))
                    (after (cons entry (remove (lambda (e) (%same-binding? e identity)) before))))
               (%set-wrapper! entry wrapper)
               (hash-table-set! %originals wrapper original)
               ;; Publish before touching the binding. A kill mid-install
               ;; leaves an inactive entry untrace can still clean up.
               ;; Retire the predecessor while it is still discoverable.
               ;; Cancellation/kill before publication cannot orphan it.
               (when previous (%retire! previous))
               (if (not (box-cas! %entries before after))
                   (loop)
                   (guard (condition (#t (%discard! entry) (raise condition)))
                     (unless (and (swap (if (and previous (eq? value (%entry-wrapper previous)))
                                           (%entry-original previous) value)
                                        wrapper)
                                  (box-cas! (%entry-state entry) 'installing 'active))
                       (error "trace: binding changed while installing" name))
                     name))))))))

    (define (%remove! identity getter)
      ;; Both control forms carry a real getter: its global reference keeps
      ;; the slot's module/generation alive even in a retained disable
      ;; closure, so an identity cannot accidentally name a recycled slot.
      (let ((entry (find (lambda (e) (%same-binding? e identity)) (unbox %entries))))
        (and entry (begin (getter) (%discard! entry) (caddr identity)))))

    (define (%remove-all!)
      (let loop ()
        (let ((before (unbox %entries)))
          (for-each %retire! before)
          (unless (box-cas! %entries before '()) (loop)))))

    (define (traced-procedures)
      "Return (MODULE NAME) for each still-wrapped binding.
Redefinition replaces a wrapper; trace the name again to trace new code.
Untrace still retires the old wrapper without changing that definition."
      (filter-map (lambda (e)
                    (and (%entry-active? e)
                         (eq? ((%entry-getter e)) (%entry-wrapper e))
                         (list (%entry-module e) (%entry-name e))))
                  (unbox %entries)))

    (define-syntax trace
      (syntax-rules ()
        "Trace module-bound NAMEs, or list the bindings traced with no NAME.
Import (techne debug), then (trace foo). Calls through that binding are
wrapped, including guarded JIT calls. Earlier aliases to its original
procedure and lexically inlined calls are not intercepted. Sealed root
primitives cannot be changed. Tracing adds frames (at most 32 nested
traced calls); deeper calls run normally without trace events."
        ((_) (traced-procedures))
        ((_ name ...)
         (begin (%install! (%binding-identity name) (lambda () name)
                          (lambda (expected replacement)
                            (if (eq? name expected)
                                (begin (set! name replacement) #t)
                                #f))) ...))))

    (define-syntax untrace
      (syntax-rules ()
        "Stop tracing module-bound NAMEs; with none, stop all tracing.
Restore an original only if its binding still holds our wrapper."
        ((_) (%remove-all!))
        ((_ name ...) (begin (%remove! (%binding-identity name) (lambda () name)) ...))))))
