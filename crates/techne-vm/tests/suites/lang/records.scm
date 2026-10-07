;;; Records show their fields to inspectors.

(define-record-type point (make-point x y) point? (x point-x) (y point-y set-point-y!))

(test-begin "records")

(test '((x . 1) (y . (2 3))) (record-fields (make-point 1 (list 2 3))))
(test 'point (type-of (make-point 1 2)))
(test #t (guard (e (#t #t)) (record-fields '(1)) #f))

(test-end)
