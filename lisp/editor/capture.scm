;;; Capturing the editor's own rendered pixels. The frontend supplies a PNG
;;; of a successfully drawn snapshot; Scheme owns destinations, completion,
;;; recording cadence and bounds. Nothing reads the desktop.

(require "session.scm")
(require "commands.scm")
(require "dispatch.scm")
(require "buffers.scm")
(require "files.scm")
(require "remote.scm")
(require "minibuffer.scm")
(import (techne crypto))

(provide screenshot! recording-start! recording-stop!
         editor-capture-request! editor-take-captures! editor-captured! editor-cancel-capture! editor-cancel-captures!
         editor-record! editor-stop-record!)

(define %capture-next 0)
(define capture-max-pending 4)
(define recording-max-frames 300)
(define recording-max-bytes (* 256 1024 1024))

;; A job: id, absolute path, success callback, failure callback, publication lease.
(define (capture! s path done failed #:lease [lease #f])
  (unless s (error "No frontend is attached"))
  (when (remote-path? path) (error "Captures need a local path"))
  (let ((path (absolute-path path)) (jobs (or (sget s 'capture-jobs) '())))
    (when (>= (length jobs) capture-max-pending) (error "Too many pending captures"))
    (when (file-attributes path) (error "Capture destination already exists" path))
    (set! %capture-next (+ %capture-next 1))
    (sset! s 'capture-jobs (cons (vector %capture-next path done failed lease) jobs))
    (sset! s 'capture-queue (append (or (sget s 'capture-queue) '()) (list %capture-next)))
    %capture-next))

(define (screenshot! s path)
  "Capture S's next rendered frame to a new PNG at PATH.
Return a request id, not a claim that drawing or saving has completed.
The frontend may draw a newer snapshot; completion identifies that frame."
  (capture! s path
            (lambda (snapshot width height bytes) (message! s (string-append "Screenshot: " (absolute-path path))))
            (lambda () #f)))

(define (editor-capture-request! path lease)
  "Request a screenshot at PATH in the selected session, guarded by LEASE."
  ;; An agent looking must not itself change the echo area being observed.
  (capture! (current-session) path (lambda (snapshot width height bytes) #f) (lambda () #f) #:lease lease))

(define (editor-take-captures! s)
  "Take the capture request ids queued in S since last asked."
  (let ((ids (or (sget s 'capture-queue) '())))
    (sset! s 'capture-queue '())
    ids))

;; Write privately, then publish complete bytes without replacing a name.
;; The OS boundary checks the caller's lease atomically with publication,
;; so cancelling during file I/O cannot leave a late screenshot behind.
(define (save-capture! path png lease snapshot width height)
  (let ((tmp (string-append path ".capture-" (hex-encode (random-bytes 8)))))
    (create-directory tmp #o700)
    (dynamic-wind
      (lambda () #f)
      (lambda ()
        (let ((file (string-append tmp "/frame.png")))
          (call-with-port (open-binary-output-file file) (lambda (p) (write-bytevector png p)))
          (set-file-mode! file #o600)
          (%capture-publish! lease file path snapshot width height)))
      (lambda () (delete-path tmp #t)))))

(define (editor-captured! s id snapshot width height png failure)
  "Save S's completed capture ID from SNAPSHOT, WIDTH by HEIGHT pixels.
PNG holds the encoded bytes; nonempty FAILURE reports a frontend error.
Return its path or raise. Remove the job even on failure; recordings stop."
  (let* ((jobs (or (sget s 'capture-jobs) '()))
         (job (find (lambda (j) (= id (vector-ref j 0))) jobs)))
    ;; A worker may finish after cancellation. Ignore it, without recreating
    ;; the job, touching disk or changing the echo area.
    (and job
      (begin
        (sset! s 'capture-jobs (remove (lambda (j) (eq? job j)) jobs))
        (guard (e (#t ((vector-ref job 3)) (raise e)))
          (unless (string=? failure "") (error failure))
          (unless (and (> snapshot 0) (> width 0) (> height 0) (> (bytevector-length png) 0))
            (error "Invalid captured frame"))
          (save-capture! (vector-ref job 1) png (vector-ref job 4) snapshot width height)
          ((vector-ref job 2) snapshot width height (bytevector-length png))
          (vector-ref job 1))))))

(define-record-type <recording>
  (make-recording directory limit interval started frames bytes active)
  recording?
  (directory recording-directory)
  (limit recording-limit)
  (interval recording-interval)
  (started recording-started)
  (frames recording-frames set-recording-frames!)
  (bytes recording-bytes set-recording-bytes!)
  (active recording-active? set-recording-active!))

(define (recording-manifest! r)
  (replace-file (string-append (recording-directory r) "/recording.scm")
    (call-with-output-string
      (lambda (p)
        (write `((format techne-frames-v1) (active ,(recording-active? r))
                 (interval-ms ,(recording-interval r)) (bytes ,(recording-bytes r))
                 (frames ,@(reverse (recording-frames r)))) p)
        (newline p)))))

(define (recording-end! s r)
  (set-recording-active! r #f)
  ;; Release even when the destination disappeared or the disk filled up.
  (when (eq? (sget s 'recording) r) (sset! s 'recording #f))
  (recording-manifest! r))

(define (recording-frame! s r)
  (when (recording-active? r)
    (let* ((index (length (recording-frames r)))
           (digits (number->string index))
           (name (string-append (make-string (- 6 (string-length digits)) #\0) digits ".png")))
      (guard (e (#t (recording-end! s r) (raise e)))
        (capture! s (string-append (recording-directory r) "/" name)
          (lambda (snapshot width height bytes)
            (set-recording-bytes! r (+ (recording-bytes r) bytes))
            (set-recording-frames! r
              (cons `((file ,name) (snapshot ,snapshot) (width ,width) (height ,height)
                      (elapsed-ms ,(quotient (* 1000 (- (current-jiffy) (recording-started r))) (jiffies-per-second))))
                    (recording-frames r)))
            (cond ((or (not (recording-active? r))
                       (>= (length (recording-frames r)) (recording-limit r))
                       (>= (recording-bytes r) recording-max-bytes))
                   (recording-end! s r))
                  (else
                    (recording-manifest! r)
                    ;; Only one recording capture in flight. Slow rendering
                    ;; lowers the frame rate instead of accumulating work.
                    (spawn (lambda () (sleep (recording-interval r)) (recording-frame! s r))))))
          (lambda () (recording-end! s r)))))))

(define (recording-start! s directory #:frames [frames 100] #:interval [interval 100])
  "Record S to a new private DIRECTORY: PNG frames and recording.scm.
FRAMES is 1..300; INTERVAL is 50..10000 milliseconds between completions
and requests. Stop at 256 MiB plus at most one frame. This is a timestamped
frame sequence, not constant-rate video. A failed capture ends recording."
  (unless s (error "No frontend is attached"))
  (when (sget s 'recording) (error "Already recording"))
  (unless (and (exact-integer? frames) (<= 1 frames recording-max-frames)
               (exact-integer? interval) (<= 50 interval 10000))
    (error "Recording needs 1..300 frames and a 50..10000 ms interval"))
  (when (remote-path? directory) (error "Recordings need a local directory"))
  (let* ((dir (absolute-path directory))
         (r (make-recording dir frames interval (current-jiffy) '() 0 #t)))
    (create-directory dir #o700)
    (sset! s 'recording r)
    (guard (e (#t (set-recording-active! r #f) (sset! s 'recording #f) (raise e)))
      (recording-manifest! r)
      (recording-frame! s r)
      dir)))

(define (recording-stop! s)
  "Stop S's recording. One already requested frame may still complete."
  (let ((r (and s (sget s 'recording))))
    (unless r (error "Not recording"))
    (recording-end! s r)
    (recording-directory r)))

(define (editor-cancel-capture! s id)
  "Cancel S's capture ID, releasing its queue slot; return #t if found."
  (let* ((jobs (or (sget s 'capture-jobs) '()))
         (job (find (lambda (j) (= id (vector-ref j 0))) jobs)))
    (and job
      (begin
        (sset! s 'capture-jobs (remove (lambda (j) (eq? job j)) jobs))
        (sset! s 'capture-queue (remove (lambda (n) (= n id)) (or (sget s 'capture-queue) '())))
        ((vector-ref job 3))
        #t))))

(define (editor-cancel-captures! s)
  "Cancel S's captures and recording on detach, returning the request ids."
  (let ((jobs (or (sget s 'capture-jobs) '())) (r (sget s 'recording)))
    (sset! s 'capture-jobs '())
    (sset! s 'capture-queue '())
    (when r (guard (e (#t #f)) (recording-end! s r)))
    (for-each (lambda (job) (guard (e (#t #f)) ((vector-ref job 3)))) jobs)
    (map (lambda (job) (vector-ref job 0)) jobs)))

(define (editor-record! directory frames interval)
  "Record FRAMES frames in DIRECTORY in the selected session for a caller.
INTERVAL is the number of milliseconds between completions and requests."
  (recording-start! (current-session) directory #:frames frames #:interval interval))
(define (editor-stop-record!)
  "Stop recording in the selected session for a caller."
  (recording-stop! (current-session)))

(define-command (screenshot s file)
  "Save the next rendered editor frame as a new PNG at FILE."
  #:arguments '((file file #:doc "New local PNG file; existing files are never replaced."))
  #:read '((file file #:prompt "Screenshot PNG: "))
  (screenshot! s file))

(define-command (start-recording s file)
  "Record up to 100 frames in a new directory FILE, 100 ms apart.
Use stop-recording to finish early. recording.scm describes the PNG frames."
  #:arguments '((file file #:doc "New local directory for the recording."))
  #:read '((file file #:prompt "Recording directory: "))
  (recording-start! s file))

(define-command (stop-recording s n)
  "Stop recording editor frames; an in-flight frame may still complete."
  (message! s (string-append "Recording: " (recording-stop! s))))
