;;; -*- lexical-binding: t -*-
(setq max-lisp-eval-depth 100000)
(defun out (x) (princ (format "%s\n" x)))
(defun iota1 (n) (let ((l nil)) (while (> n 0) (push n l) (setq n (1- n))) l))
(defun ok-p (row dist placed)
  (if (null placed) t
    (and (not (= (car placed) (+ row dist))) (not (= (car placed) (- row dist)))
         (ok-p row (+ dist 1) (cdr placed)))))
(defun try-q (x y z)
  (if (null x) (if (null y) 1 0)
    (+ (if (ok-p (car x) 1 z) (try-q (append (cdr x) y) nil (cons (car x) z)) 0)
       (try-q (cdr x) (cons (car x) y) z))))
(defun q-repeat (n acc) (if (= n 0) acc (q-repeat (- n 1) (+ acc (try-q (iota1 10) nil nil)))))
(out (q-repeat 10 0))
