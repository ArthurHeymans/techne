;;; (techne format): numbers with a fixed number of decimal places, as
;;; printf writes them (these are what Python's "%.Nf" gives).

(import (techne format))

(test-begin "format")

;; Ties go to the even digit.
(test "2" (number->decimal 2.5 0))
(test "4" (number->decimal 3.5 0))
(test "0" (number->decimal 0.5 0))
(test "-2" (number->decimal -1.5 0))
(test "1.2" (number->decimal 1.25 1))
(test "0.12" (number->decimal 0.125 2))
;; An inexact number rounds as its binary value: 1.05 is a little more
;; than 1.05, 2.675 a little less than 2.675.
(test "1.1" (number->decimal 1.05 1))
(test "2.67" (number->decimal 2.675 2))
;; Exact numbers round as they are.
(test "1.0" (number->decimal 21/20 1))
(test "0.33" (number->decimal 1/3 2))
(test "7.00" (number->decimal 7 2))
(test "42" (number->decimal 42 0))
;; Small numbers, zeros, signs.
(test "0.000" (number->decimal 1e-7 3))
(test "0.000" (number->decimal 0.0 3))
(test "-0.0" (number->decimal -0.04 1))
(test "-0.0" (number->decimal -0.0 1))
(test "123456.79" (number->decimal 123456.789 2))
(test "1000000000000000000000" (number->decimal 1e21 0))
(test-error (number->decimal +inf.0 1))
(test-error (number->decimal (/ 0. 0.) 1))
(test-error (number->decimal 1.5 -1))
(test-error (number->decimal 1.5 1.0))
(test-error (number->decimal 1 4097))
(test 4098 (string-length (number->decimal 1 4096)))

(test-end)
