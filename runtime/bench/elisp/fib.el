;;; -*- lexical-binding: t -*-
(setq max-lisp-eval-depth 100000)
(defun out (x) (princ (format "%s\n" x)))
(defun fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
(out (fib 32))
