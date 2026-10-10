;;; Numbers as text: fixnums (48 bits) are written in any radix, the same
;;; as the bignums just past them.

(test-begin "numbers")

(test '("0" "-7" "-ff" "11111111" "z" "-12")
      (map number->string '(0 -7 -255 255 35 -10) '(10 10 16 2 36 8)))
(define hi (- (expt 2 47) 1))
(define lo (- (expt 2 47)))
(test '("140737488355327" "-140737488355328" "140737488355328" "-140737488355329")
      (map number->string (list hi lo (+ hi 1) (- lo 1))))
(test (list (make-string 47 #\1) (string-append "-1" (make-string 47 #\0)) (string-append "1" (make-string 47 #\0)))
      (map (lambda (n) (number->string n 2)) (list hi lo (+ hi 1))))
(test '("7fffffffffff" "-800000000000" "1dvxwqtywv" "-1dvxwqtyww")
      (map number->string (list hi lo hi lo) '(16 16 36 36)))
(test-error (number->string 1 37))

(test-end)
