;;; Unit tests for `(techne checkdoc)` in a plain VM world (no editor).
(import (techne test) (techne checkdoc))

(define-suite "techne checkdoc"
  (define-test "accepts well-formed procedure docstrings"
    (assert-equal '()
      (check-docstring 'demo 'procedure 'add "Return the sum of A and B." '(a b) #f)))

  (define-test "flags missing docstring, missing parameter, and missing period"
    (let ((missing (check-docstring 'demo 'procedure 'f #f '() #f)))
      (assert-equal 1 (length missing))
      (assert-contains "Document it with a docstring." (problem-text (car missing))))
    (let ((no-param (check-docstring 'demo 'procedure 'f "Compute a value." '(x) #f)))
      (assert-true (pair? no-param))
      (assert-contains "Name the parameter X" (problem-text (car no-param))))
    (let ((no-period (check-docstring 'demo 'procedure 'f "Compute X" '(x) #f)))
      (assert-true (pair? no-period))
      (assert-contains "ending with a period" (problem-text (car no-period)))))

  (define-test "validates key references only when #:known-commands is given"
    (assert-equal '()
      (check-docstring 'demo 'procedure 'f "Run \\[some-cmd]." '() #f))
    (let ((bad (check-docstring 'demo 'procedure 'f "Run \\[missing-cmd]." '() #f #:known-commands '(other-cmd))))
      (assert-equal 1 (length bad))
      (assert-contains "names no command" (problem-text (car bad)))))

  (define-test "module-documentation-problems reports zero issues on (techne checkdoc) itself"
    (assert-equal '() (module-documentation-problems '(techne checkdoc)))))
