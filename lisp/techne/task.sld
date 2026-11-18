;;; Shared Scheme classification for the runtime's cooperative cancellation.
(define-library (techne task)
  (export task-cancelled-condition?)
  (import (techne))
  (begin
    (define (task-cancelled-condition? condition)
      "Return whether CONDITION is the kernel's cooperative task cancellation.
Fallback guards must propagate it, not turn stopped work into successful data."
      (and (error-object? condition)
           (equal? (error-object-message condition) "task cancelled")))))
