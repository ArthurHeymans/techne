;;; -*- lexical-binding: t -*-
(setq max-lisp-eval-depth 100000)
(defun out (x) (princ (format "%s\n" x)))
(defun tak (x y z) (if (not (< y x)) z (tak (tak (- x 1) y z) (tak (- y 1) z x) (tak (- z 1) x y))))
(defun tak-repeat (n acc) (if (= n 0) acc (tak-repeat (- n 1) (+ acc (tak 22 16 8)))))
(out (tak-repeat 10 0))
