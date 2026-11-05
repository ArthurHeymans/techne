;;; End-to-end capture driver. Run only inside headless.scm; the Rust CLI
;;; integration test supplies the binary and artifact directory and compares
;;; the built-in capture against grim's independent compositor capture.
(import (techne))

(define args (cdr (command-line)))
(define binary (car args))
(define dir (cadr args))
(define (at name) (string-append dir "/" name))
(define (check yes message) (unless yes (error message)))
(define (run program args log)
  (call-with-process program args process-wait #:stdin 'null #:stdout (at log) #:stderr (at "commands.err")))
(define (ok program args log) (check (equal? 0 (run program args log)) (string-append "command failed: " program " " log)))
(define (keys . args) (ok "wtype" (append '("-k" "Shift_L" "-s" "300") args) "keys.log"))
(define last-snapshot 0)
(define (capture name)
  (let ((log (string-append name ".json")))
    (ok binary (list "window" "--screenshot" (at name)) log)
    (let ((reply (dict-ref (string->json (file->string (at log))) "Captured")))
      (check (string=? (dict-ref reply "path") (at name)) "wrong capture path")
      (check (> (dict-ref reply "snapshot") last-snapshot) "snapshot did not advance")
      (check (and (> (dict-ref reply "width") 0) (> (dict-ref reply "height") 0)) "invalid capture dimensions")
      (set! last-snapshot (dict-ref reply "snapshot")))))
(define (wait-for predicate what)
  (let ((deadline (+ (current-jiffy) (* 10 (jiffies-per-second)))))
    (let loop ()
      (cond ((predicate) #t)
            ((>= (current-jiffy) deadline) (error "Timed out" what))
            (else (sleep 50) (loop))))))
(define (manifest name) (call-with-input-file (at (string-append name "/recording.scm")) read))
(define (field alist key) (cadr (assq key alist)))

(call-with-output-file (at "example.scm")
  (lambda (p)
    (display "(import (techne))\n\n;; Capturing the real editor: red, blue, gutter and mode lines.\n(define (greet name)\n  (string-append \"Hello, \" name))\n\n(greet \"screenshot\")\n" p)))

(call-with-process binary (list "window" (at "example.scm"))
  (lambda (editor)
    ;; A successful capture is the readiness barrier, not an assumed delay.
    (wait-for (lambda ()
                (when (process-exited? editor) (error "Editor exited"))
                (equal? 0 (run binary (list "window" "--screenshot" (at "ready.png")) "ready.json")))
              "editor ready")
    ;; Split and change the focused pane. Full-frame capture includes both
    ;; mode lines, gutter, inactive caret, echo area and the document.
    (keys "-M" "ctrl" "-k" "x" "-m" "ctrl" "-k" "2"
          "-M" "ctrl" "-k" "x" "-m" "ctrl" "o")
    (capture "split.png")
    (ok "grim" (list (at "compositor.png")) "grim.log")
    ;; Invoke the public command through M-x, including its file reader.
    (keys "-M" "alt" "-k" "x" "-m" "alt" "screenshot" "-k" "Return" "-s" "100"
          "-M" "ctrl" "-k" "a" "-k" "k" "-m" "ctrl" (at "interactive.png") "-k" "Return")
    (wait-for (lambda () (file-exists? (at "interactive.png"))) "interactive screenshot")
    ;; Open a live minibuffer popup and capture it without dismissing it.
    (keys "-M" "alt" "-k" "x" "-m" "alt" "scree")
    (capture "popup.png")
    (keys "-M" "ctrl" "-k" "g" "-m" "ctrl")
    ;; Error paths must not truncate an existing image or leave partial files.
    (check (not (equal? 0 (run binary (list "window" "--screenshot" (at "split.png")) "exists.log"))) "overwrite was allowed")
    (check (not (equal? 0 (run binary (list "window" "--screenshot" (at "missing/frame.png")) "missing.log"))) "missing parent was allowed")
    (capture "after-error.png")
    ;; Bounded recording finishes by itself and produces timestamped metadata.
    (ok binary (list "window" "--record" (at "recording") "--record-frames" "3" "--record-interval" "50") "record.json")
    (wait-for (lambda () (not (field (manifest "recording") 'active))) "bounded recording")
    (let ((frames (cdr (assq 'frames (manifest "recording")))))
      (check (= 3 (length frames)) "wrong frame count")
      (check (< (field (car frames) 'snapshot) (field (cadr frames) 'snapshot) (field (caddr frames) 'snapshot)) "recording snapshots not ordered")
      (check (<= (field (car frames) 'elapsed-ms) (field (cadr frames) 'elapsed-ms) (field (caddr frames) 'elapsed-ms)) "recording timestamps not ordered"))
    ;; Stop early; a frame already in flight is permitted to land afterward.
    (ok binary (list "window" "--record" (at "stopped") "--record-frames" "300") "start.json")
    (ok binary '("window" "--stop-recording") "stop.json")
    (capture "after-stop.png")
    (check (not (field (manifest "stopped") 'active)) "recording did not stop")
    ;; A non-aligned width exercises GPU row padding and window resizing.
    (let* ((runtime (get-environment-variable "XDG_RUNTIME_DIR"))
           (socket (find (lambda (name) (string-prefix? "sway-ipc." name)) (directory-list runtime))))
      (check socket "headless sway IPC socket missing")
      (ok "swaymsg" (list "-s" (string-append runtime "/" socket) "output" "HEADLESS-1" "resolution" "1003x701") "resize.log"))
    (wait-for (lambda ()
                (capture "resized.png")
                (let* ((header (call-with-port (open-binary-input-file (at "resized.png")) (lambda (p) (read-bytevector 24 p))))
                       (u32 (lambda (at) (+ (* (bytevector-u8-ref header at) 16777216)
                                             (* (bytevector-u8-ref header (+ at 1)) 65536)
                                             (* (bytevector-u8-ref header (+ at 2)) 256)
                                             (bytevector-u8-ref header (+ at 3))))))
                  (if (and (= (u32 16) 1003) (= (u32 20) 701)) #t
                      (begin (delete-path (at "resized.png") #f) #f))))
              "resized capture")
    (keys "-M" "ctrl" "-k" "x" "-k" "c" "-m" "ctrl")
    (wait-for (lambda () (process-exited? editor)) "editor shutdown")
    (check (equal? 0 (process-wait editor)) "editor did not exit cleanly"))
  #:stdin 'null #:stdout (at "editor.log") #:stderr (at "editor.err"))
(display "capture end-to-end: passed\n")
