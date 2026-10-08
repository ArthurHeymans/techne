;;; The test API of chibi-scheme's (chibi test) and the common part of
;;; SRFI 64, enough to run upstream suites unchanged. The runner
;;; (tests/suites.rs) prepends this file to every suite.
;;;
;;; The runner splits a suite into its top-level forms and passes them, as
;;; strings, to %test-forms, which reads and evaluates each one on its own: a
;;; form the reader rejects or that fails outside a test costs only itself.
;;; Each test prints one line the runner reads:
;;;   ;;;test pass F.N
;;;   ;;;test fail F.N <what was tested>  -- <why>
;;; for the Nth test of the Fth form; F.0 is a form failing outside any test.
;;; ";;;test end" after the last form tells a finished suite from an aborted one.

(define %test-form 0)
(define %test-count 0)

(define (%test-forms forms)
  (for-each
   (lambda (text)
     (set! %test-form (+ %test-form 1))
     (set! %test-count 0)
     (guard (e (#t (set! %test-count -1) ; reported as test 0
                   (%test-report #f text (%test-error-message e))))
       (let ((port (open-input-string text)))
         (let loop ()
           (let ((datum (read port)))
             (unless (eof-object? datum)
               (eval datum)
               (loop)))))))
   forms)
  (display ";;;test end")
  (newline))

(define (%test-report ok what why)
  (set! %test-count (+ %test-count 1))
  (display (if ok ";;;test pass " ";;;test fail "))
  (display %test-form)
  (display ".")
  (display %test-count)
  (unless ok
    (display " ")
    (display (%test-oneline what))
    (display "  -- ")
    (display (%test-oneline why)))
  (newline)
  ;; Unflushed output would be lost if the suite hangs and is killed.
  (flush-output))

(define (%test-oneline x)
  (let ((s (if (string? x) x (call-with-output-string (lambda (p) (write x p))))))
    (list->string (map (lambda (c) (if (char=? c #\newline) #\space c)) (string->list s)))))

(define (%test-show v) (call-with-output-string (lambda (p) (write v p))))

;; Inexact numbers compare approximately, as in (chibi test).
(define (%test-equal? expect got)
  (cond ((and (number? expect) (number? got) (inexact? expect))
         (or (= expect got)
             (and (not (= expect expect)) (not (= got got)))
             (< (magnitude (- expect got)) (* 1e-6 (max 1 (magnitude expect))))))
        ((and (pair? expect) (pair? got))
         (and (%test-equal? (car expect) (car got)) (%test-equal? (cdr expect) (cdr got))))
        ((and (vector? expect) (vector? got))
         (%test-equal? (vector->list expect) (vector->list got)))
        (else (equal? expect got))))

(define (%test-error-message e)
  (if (error-object? e)
      (string-append "error: " (error-object-message e) " " (%test-show (error-object-irritants e)))
      (string-append "raised " (%test-show e))))

;; Compares the values of THUNK and EXPECT-THUNK using SAME?; an error in
;; either fails the test.
(define (%test-run what expect-thunk thunk same?)
  (guard (e (#t (%test-report #f what (%test-error-message e))))
    (let* ((expect (expect-thunk)) (got (thunk)))
      (if (same? expect got)
          (%test-report #t what "")
          (%test-report #f what (string-append "expected " (%test-show expect) ", got " (%test-show got)))))))

;; How `test` compares, as in Chicken's test egg.
(define current-test-comparator (make-parameter %test-equal?))

(define-syntax test
  (syntax-rules ()
    ((_ name expect expr) (%test-run name (lambda () expect) (lambda () expr) (current-test-comparator)))
    ((_ expect expr) (%test-run 'expr (lambda () expect) (lambda () expr) (current-test-comparator)))))

;; SRFI 64's (test-equal [name] expect expr), and (chibi test)'s
;; (test-equal same? [name] expect expr), told apart by a procedure first.
(define-syntax test-equal
  (syntax-rules ()
    ((_ same? name expect expr) (%test-run name (lambda () expect) (lambda () expr) same?))
    ((_ a expect expr)
     (let ((x a))
       (if (procedure? x)
           (%test-run 'expr (lambda () expect) (lambda () expr) x)
           (%test-run x (lambda () expect) (lambda () expr) %test-equal?))))
    ((_ expect expr) (%test-run 'expr (lambda () expect) (lambda () expr) %test-equal?))))

(define-syntax test-eqv
  (syntax-rules ()
    ((_ name expect expr) (%test-run name (lambda () expect) (lambda () expr) eqv?))
    ((_ expect expr) (%test-run 'expr (lambda () expect) (lambda () expr) eqv?))))

(define-syntax test-eq
  (syntax-rules ()
    ((_ name expect expr) (%test-run name (lambda () expect) (lambda () expr) eq?))
    ((_ expect expr) (%test-run 'expr (lambda () expect) (lambda () expr) eq?))))

(define-syntax test-assert
  (syntax-rules ()
    ((_ name expr) (%test-run name (lambda () #t) (lambda () (and expr #t)) eq?))
    ((_ expr) (%test-run 'expr (lambda () #t) (lambda () (and expr #t)) eq?))))

(define-syntax test-not
  (syntax-rules ()
    ((_ name expr) (%test-run name (lambda () #f) (lambda () (and expr #t)) eq?))
    ((_ expr) (%test-run 'expr (lambda () #f) (lambda () (and expr #t)) eq?))))

(define-syntax test-values
  (syntax-rules ()
    ((_ name expect expr)
     (%test-run name (lambda () (call-with-values (lambda () expect) list))
                (lambda () (call-with-values (lambda () expr) list)) %test-equal?))
    ((_ expect expr) (test-values 'expr expect expr))))

(define-syntax test-error
  (syntax-rules ()
    ((_ name type expr) (test-error name expr))
    ((_ name expr)
     (%test-report (guard (e (#t #t)) expr #f) name "no error was raised"))
    ((_ expr) (test-error 'expr expr))))

(define (test-begin . name) #t)
(define (test-end . name) #t)

(define-syntax test-group
  (syntax-rules ()
    ((_ name body ...) (begin (test-begin name) body ... (test-end name)))))

;; chibi's test file closes with (test-exit); SRFI 64 has nothing to exit.
(define (test-exit . o) #t)

;; chibi's exception objects, from its (chibi) library, which SRFI 69's
;; tests store in a table.
(define (make-exception kind message irritants procedure source) (vector 'exception kind message irritants))
(define (exception-kind e) (vector-ref e 1))
