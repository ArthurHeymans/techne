;;; Workloads matching el-be-back/bench/ebb-bench.el, at 80 by 24 cells.
(import (techne terminal))
(require "../editor/terminal-render.scm")
(provide terminal-bench-data terminal-bench-iteration)

(define esc (string #\x1b))
(define csi (string-append esc "["))
(define rows 24)
(define cols 80)

(define (repeat-to-size fragment size)
  (let ((out (open-output-string)) (n (string-length fragment)))
    (let loop ((left size))
      (if (< left n)
          (begin (display (substring fragment 0 left) out) (get-output-string out))
          (begin (display fragment out) (loop (- left n)))))))

(define (sgr-cycle)
  (let ((out (open-output-string)))
    (do ((i 0 (+ i 1))) ((= i 256) (get-output-string out))
      (display csi out) (display "38;5;" out) (display i out)
      (display (string-append "mabcdefghij" csi "0m\r\n") out))))

(define (repaint line trailer)
  (let ((out (open-output-string)))
    (display (string-append csi "2J" csi "H") out)
    (do ((r 1 (+ r 1))) ((> r rows) (display trailer out) (get-output-string out))
      (line r out))))

(define (uniform-frame)
  (repaint (lambda (r out)
             (display (string-append csi (number->string r) ";1H" csi (if (even? r) "44m" "42m")
                                     (make-string cols (if (even? r) #\- #\=))) out))
           (string-append csi "0m")))

(define (checker-frame)
  (let ((red (string-append csi "31ma")) (blue (string-append csi "34ma")))
    (repaint (lambda (r out)
               (do ((c 1 (+ c 1))) ((> c cols) (display (string-append csi "0m\r\n") out))
                 (display (if (even? (+ r c)) red blue) out)))
             "")))

(define (terminal-bench-data name size)
  "Return SIZE ASCII bytes of terminal workload NAME."
  (repeat-to-size
   (case name
     ((plain) (string-append (make-string 78 #\A) "\r\n"))
     ((sgr) (sgr-cycle))
     ((tui) (uniform-frame))
     ((checker) (checker-frame))
     (else (error "Unknown terminal workload" name)))
   size))


(define (terminal-bench-iteration data)
  "Measure one fresh terminal parsing DATA and projecting it to a buffer.
Return elapsed milliseconds; the caller warms up and collects between runs."
  (let ((start (current-jiffy)))
    (let* ((d (make-document "")) (t (make-terminal rows cols #:history 2000))
           (p (make-terminal-projection d 2000)))
      (terminal-feed! t data)
      (terminal-project! p t)
      (let ((elapsed (- (current-jiffy) start)))
        (when (zero? (document-length d)) (error "Terminal benchmark projected no text"))
        (/ (* 1000.0 elapsed) (jiffies-per-second))))))
