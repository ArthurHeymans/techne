;;; Shared fixtures for the Scheme tool suites, not a second test framework.
(require "../../test.scm")
(import (techne tools) (techne crypto))
(provide expect fails tool-test-case with-tool-project call-with-tool-directory
         fixture-write test-summary)

(define case-name (make-parameter "tools"))
(define case-check (make-parameter (vector 0)))

(define (expect expected actual)
  (let ((n (+ 1 (vector-ref (case-check) 0))))
    (vector-set! (case-check) 0 n)
    (check (string-append (case-name) " #" (number->string n)) expected actual)))

(define (fails thunk)
  (guard (e (#t #t)) (thunk) #f))

(define (tool-test-case name thunk)
  (parameterize ((case-name name) (case-check (vector 0)))
    (guard (e (#t (check-true (string-append name " unexpected exception") #f)
                 (displayln (condition/report-string e))))
      (thunk))))

(define (call-with-tool-directory thunk)
  ;; Keep paths short enough for the CLI's Unix-domain sockets too.
  (let make ((tries 16))
    (let ((root (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                               "/tt-" (hex-encode (random-bytes 6)))))
      (if (guard (e (#t #f)) (create-directory root #o700) #t)
          (dynamic-wind (lambda () #t) (lambda () (thunk root))
                        (lambda () (delete-path root #t)))
          (if (zero? tries) (error "cannot create tool-test directory" root)
              (make (- tries 1)))))))

(define (fixture-write root path contents)
  (call-with-port
    ((if (bytevector? contents) open-binary-output-file open-output-file) (string-append root "/" path))
    (lambda (port)
      (if (bytevector? contents) (write-bytevector contents port) (display contents port)))))

(define (with-tool-project name fixtures thunk)
  (tool-test-case name
    (lambda ()
      (call-with-tool-directory
        (lambda (root)
          (for-each (lambda (file) (fixture-write root (car file) (cadr file))) fixtures)
          (thunk root (tool-project root)))))))
