;;; The slice 2 latency benchmarks (PLAN.md, Stage 1): a 100k-line file,
;;; a file with one 1 MB line and mixed-script Unicode, then the first
;;; again with a busy Lisp task. Each types 300 keys and reports
;;; key-to-frame latency. Run on a quiet machine, in
;;; `nix develop .#window`, from the repository's root:
;;;
;;;   target/debug/techne run crates/techne-window/bench.scm

(import (techne) (techne crypto) (srfi 27) (srfi 115))

(define (run program args)
  (let ((status (process-wait (process-spawn program args #:stdin 'null #:stdout 'inherit #:stderr 'inherit))))
    (unless (eqv? status 0) (error "bench: failed:" program args status))))

;; What PROGRAM with ARGS writes, its two streams together, once it has
;; ended; an error if it fails.
(define (output program args)
  (call-with-process program args
    (lambda (p)
      (let* ((err (spawn (lambda () (process-read-all p 'stderr))))
             (out (process-read-all p 'stdout))
             (both (string-append out (task-join err)))
             (status (process-wait p)))
        (unless (eqv? status 0)
          (display both (current-error-port))
          (error "bench: failed:" (cons program args) status))
        both))
    #:stdin 'null))

(run "cargo" '("build" "--release" "-p" "techne-cli"))

(define dir
  (let ((path (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                             "/techne-bench." (hex-encode (random-bytes 6)))))
    (create-directory path #o700)
    path))
(define (file name) (string-append dir "/" name ".txt"))

;; The same files every run: the random source's own start.
(define source (make-random-source))
(define random (random-source-make-integers source))
(define (pick items) (list-ref items (random (length items))))
(define (between low high) (+ low (random (+ (- high low) 1))))

(define words '("alpha" "beta" "gamma" "delta" "println!" "fn" "let" "mut" "struct" "impl"))
(define samples '("日本語のテキスト" "Ελληνικά κείμενο" "emoji 👩‍💻🇧🇪 mixed" "e\x301; combining"
                  "עברית טקסט" "한국어 텍스트" "ascii text here"))

(define (write-lines name count line)
  (call-with-output-file (file name)
    (lambda (p) (do ((i 0 (+ i 1))) ((= i count)) (display (line) p) (newline p)))))

(define (pieces from low high)
  (let loop ((n (between low high)) (acc '()))
    (if (zero? n) (string-join acc " ") (loop (- n 1) (cons (pick from) acc)))))

;; The key-to-frame lines of the window's benchmark on ARGS, in a
;; headless session.
(define (measure . args)
  (let* ((out (output "target/release/techne"
                      (append (list "run" "crates/techne-window/headless.scm"
                                    "./target/release/techne" "--bench" "300")
                              args)))
         (lines (filter (lambda (line) (string-contains line "key to")) (string-split out #\newline))))
    (when (null? lines)
      (display out (current-error-port))
      (error "bench: no latencies from" args))
    (for-each (lambda (line) (display line) (newline)) lines)))

(define (bench)
  (write-lines "lines" 100000 (lambda () (pieces words 0 14)))
  (call-with-output-file (file "longline")
    (lambda (p)
      ;; One line of words, cut at 1 MiB.
      (let ((out (open-output-string)))
        (let loop ((size 0))
          (when (< size (* 1024 1024))
            (let ((w (string-append (pick words) " ")))
              (write-string w out)
              (loop (+ size (string-length w))))))
        (display "start\n" p)
        (display (substring (get-output-string out) 0 (* 1024 1024)) p)
        (display "\nend\n" p))))
  (write-lines "unicode" 20000 (lambda () (pieces samples 1 6)))
  (for-each (lambda (name)
              (display (string-append name ":\n"))
              (measure (file name)))
            '("lines" "longline" "unicode"))
  (display "lines, with a busy Lisp task:\n")
  (measure "--load" (file "lines")))

(dynamic-wind (lambda () #f) bench (lambda () (delete-path dir #t)))
