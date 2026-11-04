;;; Deterministic performance numbers for techne-vm, for CI and for
;;; comparing builds on a busy machine: wall time depends on the load,
;;; these do not.
;;;
;;; For each program in programs/ it reports
;;;   - instructions executed with the JIT (compiling synchronously, so
;;;     the result does not depend on thread timing) and with the
;;;     interpreter, counted by valgrind's cachegrind (repeat runs agree
;;;     to ~0.001%);
;;;   - the most words one GC pause copied or marked, an upper bound on
;;;     the longest pause's work (from TECHNE_GC_STATS, the JIT compiling
;;;     synchronously too).
;;;
;;; Usage: techne run icount.scm [OUT.json [FOOTPRINT.json]]   (inside
;;; `nix develop`; TECHNE_VM overrides the binary, PROGS the program
;;; list, TECHNE_EDITOR_STARTUP adds the editor starting: the
;;; instructions per datum of the Lisp it loads go to OUT, its whole count
;;; to FOOTPRINT, held to a budget rather than compared;
;;; TECHNE_EDITOR_KEYS adds the instructions per key of its keystroke
;;; workloads, TECHNE_SERVE the instructions per request of the HTTP
;;; server's, TECHNE_COMPILE the instructions per top-level form of
;;; reading and of loading the prelude). The JSON is the format of
;;; github-action-benchmark's customSmallerIsBetter tool; a table goes
;;; to stderr. It exits 1 when a budget is exceeded, after writing it.

(import (techne) (srfi 115))

(define args (cdr (command-line)))
(define out-file (if (pair? args) (car args) #f))
(define footprint-file (if (and (pair? args) (pair? (cdr args))) (cadr args) #f))

;; This script's directory: programs, preludes and the build are there.
(define here
  (regexp-match-submatch
   (regexp-search '(: bos ($ (* any)) "/" (* (~ #\/)) eos) (file-truename (car (command-line))))
   1))
(define (at path) (string-append here "/" path))

(define (env name) (let ((v (get-environment-variable name))) (and v (not (string=? v "")) v)))
(define vm (or (env "TECHNE_VM") (file-truename (at "../../target/release/techne-vm"))))


;; ----- Running things

;; What PROGRAM with ARGS writes, (stdout stderr), once it has ended;
;; ENVIRONMENT is set besides what this has. A program that fails is an
;; error: valgrind counts a program that failed as well as one that did
;; its work.
(define (run program args #:environment [environment '()] #:stdout [stdout #f])
  (call-with-process program args
    (lambda (p)
      (let* ((err (spawn (lambda () (process-read-all p 'stderr))))
             (out (process-read-all p 'stdout))
             (err (task-join err))
             (status (process-wait p)))
        (unless (eqv? status 0)
          (display err (current-error-port))
          (error "icount: failed:" (cons program args) status))
        (list out err)))
    #:directory here #:environment environment #:stdout stdout))

(define (lines text) (filter (lambda (l) (not (string=? l ""))) (string-split text #\newline)))
(define (words text) (regexp-extract '(+ (~ space)) text))

(define programs
  (words (or (env "PROGS") "startup fib tak nqueens bintrees hof qsort mandel hash orgparse callbacks numstr tasks")))

;; The integer after RE (an SRE prefix) in TEXT, commas left out; #f if
;; it is not there.
(define (number-after re text)
  (let ((m (regexp-search `(: ,re ($ (+ (or digit ",")))) text)))
    (and m (string->number (regexp-replace-all "," (regexp-match-submatch m 1) "")))))

(define (valgrind-count tool-args program args environment)
  (let ((err (cadr (run "valgrind" (append tool-args (list program) args)
                        #:environment environment #:stdout 'null))))
    (or (number-after '(: "I" (* space) "refs:" (* space)) err)
        (error "icount: no instruction count from valgrind for" program err))))

;; Instructions PROGRAM executes, by cachegrind.
(define (instructions program args environment)
  (valgrind-count '("--tool=cachegrind" "--cache-sim=no" "--cachegrind-out-file=/dev/null")
                  program args environment))

;; Instructions in the functions TOGGLE names, by callgrind, divided by
;; the count PROGRAM prints last: the instructions per item it timed.
(define (instructions-per program workload toggle environment)
  (let* ((ran (run "valgrind" (list "--tool=callgrind" "--callgrind-out-file=/dev/null"
                                    (string-append "--toggle-collect=" toggle) program workload)
                   #:environment environment))
         (items (string->number (last (lines (car ran)))))
         (collected (number-after '(: "Collected" (* space) ":" (* space)) (cadr ran))))
    (unless (and items collected (positive? items))
      (error "icount: no count from" program workload ran))
    (quotient collected items)))

;; Unbounded per-thread caches keep malloc on the same paths between
;; runs: compiling varies the order of earlier allocations from run to
;; run (randomly seeded hash maps, a second thread), and with it the
;; paths malloc takes in what is counted (up to 1% of the count).
(define steady-malloc '(("GLIBC_TUNABLES" . "glibc.malloc.tcache_count=65535")))

;; ----- Reports

(define entries '())
(define footprint '())
(define (entry! name unit value)
  (set! entries (cons (dict "name" name "unit" unit "value" value) entries)))
(define (footprint! name unit value)
  (set! footprint (cons (dict "name" name "unit" unit "value" value) footprint)))

;; S in WIDTH columns, as printf's %-Ns and %Ns: padded, never cut.
(define (left s width) (string-append s (make-string (max 0 (- width (string-length s))) #\space)))
(define (right s width) (string-append (make-string (max 0 (- width (string-length s))) #\space) s))
(define (report . parts)
  (let ((err (current-error-port)))
    (display (apply string-append (map (lambda (p) (if (number? p) (number->string p) p)) parts)) err)
    (newline err)))

(define (write-entries file items)
  (let ((text (string-append "[\n" (string-join (map json->string (reverse items)) ",\n")
                             (if (null? items) "" "\n") "]\n")))
    (if file
        (call-with-output-file file (lambda (p) (display text p)))
        (display text))))

;; ----- The programs

(make-directory (at "build/techne"))
;; The input of orgparse.
(unless (file-exists? (at "org-sample.org"))
  (run "python3" '("gen-org.py")))

(for-each
 (lambda (program)
   (let ((file (at (string-append "build/techne/" program ".scm"))))
     (call-with-output-file file
       (lambda (p)
         (display (file->string (at "prelude/techne.scm")) p)
         (display (file->string (at (string-append "programs/" program ".scm"))) p)))
     (let* ((jit (instructions vm (list file) '(("TECHNE_JIT_SYNC" . "1"))))
            (interp (instructions vm (list file) '(("TECHNE_JIT" . "0"))))
            ;; The JIT compiling synchronously, as for the counts: compiled
            ;; in the background, code is installed at different points of
            ;; a run, which moves the collections.
            (stats (cadr (run vm (list file) #:stdout 'null
                              #:environment '(("TECHNE_JIT_SYNC" . "1") ("TECHNE_GC_STATS" . "1")))))
            (stats (or (find (lambda (l) (string-contains l "GcStats")) (lines stats)) ""))
            (pause (+ (or (number-after "max_copied_words: " stats) 0)
                      (or (number-after "max_slice_words: " stats) 0))))
       (report (left program 10) " " (right (number->string jit) 16) " instr (jit) "
               (right (number->string interp) 16) " instr (interp) "
               (right (number->string pause) 10) " words/pause")
       (entry! (string-append program " (jit)") "instructions" jit)
       (entry! (string-append program " (interp)") "instructions" interp)
       ;; Pauses of a few words (a program that barely collects) are
       ;; noise, not a pause to compare; 0 would break ratios.
       (when (>= pause 64)
         (entry! (string-append program " GC pause") "words" pause)))))
 programs)

;; ----- The editor starting

;; The editor starting (its Lisp compiled and loaded), if
;; TECHNE_EDITOR_STARTUP names techne-editor's `startup` example. Its
;; count grows with every package the editor ships, so it is not what is
;; compared: the instructions per datum of the Lisp loaded are, which
;; shipping more Lisp leaves alone and a slower reader, expander or
;; compiler does not. The whole count goes to FOOTPRINT and is held to
;; the startup budget instead.
(define startup-budget 700000000) ; instructions, some 100 ms (REQUIREMENTS.md), as keys-budget's 4 ms
(define over-startup #f)
(let ((startup (env "TECHNE_EDITOR_STARTUP")))
  (when startup
    (let ((jit (instructions startup '() '(("TECHNE_JIT_SYNC" . "1"))))
          (interp (instructions startup '() '(("TECHNE_JIT" . "0"))))
          (data (string->number (car (words (car (run startup '("--data"))))))))
      (report (left "editor" 10) " " (right (number->string jit) 16) " instr (jit) "
              (right (number->string interp) 16) " instr (interp) "
              (right (number->string data) 10) " data")
      (entry! "editor load (jit)" "instructions per datum" (quotient jit data))
      (entry! "editor load (interp)" "instructions per datum" (quotient interp data))
      (footprint! "editor startup (jit)" "instructions" jit)
      (footprint! "editor startup (interp)" "instructions" interp)
      (footprint! "editor Lisp loaded" "data" data)
      (set! over-startup (or (> jit startup-budget) (> interp startup-budget))))))

;; ----- Keys, requests and forms

;; Instructions per key of the editor's keystroke workloads, if
;; TECHNE_EDITOR_KEYS names techne-editor's `keys` example: callgrind
;; counts only in the function typing a workload's timed keys, and the
;; count is divided by their number. A workload over the budget per key
;; fails (after the JSON is written), unless it is a known violation
;; (PLAN.md, Stage 1, slice 7).
(define keys-budget 28000000) ; instructions, some 4 ms on the daily hardware
;; Workloads with the PLAN.md item fixing them: typing in a
;; syntax-coloured file collects its window's captures again (Stage 1,
;; slice 7, "Known violation").
(define known-slow '("type-rust" "type-rust-big"))
(define over-budget '())
(let ((keys (env "TECHNE_EDITOR_KEYS")))
  (when keys
    (for-each
     (lambda (w)
       (let ((per-key (instructions-per keys w "*timed_keys*" '(("TECHNE_JIT_SYNC" . "1")))))
         (report (left (string-append "keys " w) 18) " " (right (number->string per-key) 16) " instr per key")
         (entry! (string-append "keys: " w " (jit)") "instructions per key" per-key)
         (when (and (> per-key keys-budget) (not (member w known-slow)))
           (set! over-budget (append over-budget (list w))))))
     (words (car (run keys '("--list")))))))

;; Instructions per request of the HTTP benchmark's server on in-memory
;; connections, if TECHNE_SERVE names techne-vm's `serve` example:
;; callgrind counts only in the function serving the timed requests,
;; after a warm-up in which the JIT compiled everything on first call.
(let ((serve (env "TECHNE_SERVE")))
  (when serve
    (for-each
     (lambda (w)
       (for-each
        (lambda (mode)
          (let ((per-request (instructions-per serve w "*timed_requests*"
                                               (cons (cons "TECHNE_JIT" (cdr mode)) steady-malloc))))
            (report (left (string-append "http " w) 18) " " (right (number->string per-request) 16)
                    " instr per request (" (car mode) ")")
            (entry! (string-append "http: " w " request (" (car mode) ")") "instructions per request" per-request)))
        '(("jit" . "1") ("interp" . "0"))))
     (words (car (run serve '("--list")))))))

;; Instructions per top-level form of the front end's phases, if
;; TECHNE_COMPILE names techne-vm's `compile` example: callgrind counts
;; only in the function passing over the prelude (reading it, or loading
;; it into bare worlds made beforehand), and the count is divided by the
;; forms it timed.
(let ((compile (env "TECHNE_COMPILE")))
  (when compile
    (for-each
     (lambda (w)
       (let ((per-form (instructions-per compile w "*timed_forms*" steady-malloc)))
         (report (left (string-append "compile " w) 18) " " (right (number->string per-form) 16) " instr per form")
         (entry! (string-append "compile: " w) "instructions per form" per-form)))
     (words (car (run compile '("--list")))))))

(write-entries out-file entries)
(when footprint-file (write-entries footprint-file footprint))

(when over-startup
  (report "editor startup over its budget of " startup-budget " instructions"))
(unless (null? over-budget)
  (report "over the keystroke budget of " keys-budget " instructions per key: " (string-join over-budget " ")))
(exit (if (or over-startup (pair? over-budget)) 1 0))
