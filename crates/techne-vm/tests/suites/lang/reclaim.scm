;;; Reclaiming code (language step 7): retired generations and replaced
;;; definitions are freed once unreachable; what is kept keeps working.

(test-begin "reclaim")

(define path (string-append (car (command-line)) ".reclaim.scm"))
(call-with-output-file path
  (lambda (port)
    (for-each (lambda (f) (write f port) (newline port))
              '((define label "generation")
                (define (helper x) (* 2 x))
                (define (work n)
                  (let loop ((i 0) (acc 0))
                    (if (= i n) acc (loop (+ i 1) (+ acc (helper i))))))
                (define (make-counter start)
                  (let ((n start))
                    (lambda () (set! n (+ n 1)) (list n (helper n) label))))))))

;; Load, use and unload; keep a closure of every tenth generation.
(define kept
  (let loop ((i 0) (kept '()))
    (if (= i 60)
        kept
        (begin
          (load-package 'reclaim path)
          (let* ((m (package-module (find-package 'reclaim)))
                 (counter (eval `(make-counter ,i) m)))
            (eval '(work 300) m)
            (unload-package 'reclaim)
            (when (= 0 (modulo i 7)) (collect-garbage 'full))
            (loop (+ i 1) (if (= 0 (modulo i 10)) (cons counter kept) kept)))))))
(collect-garbage 'full)
(test '((51 102 "generation") (41 82 "generation") (31 62 "generation")
        (21 42 "generation") (11 22 "generation") (1 2 "generation"))
      (map (lambda (c) (c)) kept))
(test #t (pair? (why-retained (car kept))))

;; Redefinitions replace code that closures made earlier still run.
(define old #f)
(let loop ((i 0))
  (when (< i 200)
    (eval `(define (versioned) ,i))
    (when (= i 3) (set! old versioned))
    (loop (+ i 1))))
(collect-garbage 'full)
(test 199 (versioned))
(test 3 (old))

(test #f (why-retained 42))
(test-end)
