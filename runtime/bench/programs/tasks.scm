;; The task scheduler, as a server with many idle connections uses it:
;; tasks wait on channels nobody sends to while two others pass a message
;; back and forth, and short tasks are spawned and joined in batches. Its
;; cost per message should not grow with the idle tasks.
(define idle-count 1000)
(define messages 2000)
(define idle-channels (map (lambda (i) (make-channel)) (iota idle-count)))
(define idle (map (lambda (ch) (spawn (lambda () (channel-recv ch)))) idle-channels))

(define ping (make-channel))
(define pong (make-channel))
(define ponger
  (spawn (lambda ()
           (let loop ()
             (let ((v (channel-recv ping)))
               (unless (eof-object? v)
                 (channel-send pong (+ v 1))
                 (loop)))))))
(define pinger
  (spawn (lambda ()
           (let loop ((i 0) (acc 0))
             (if (= i messages)
                 (begin (channel-close ping) acc)
                 (begin (channel-send ping i)
                        (loop (+ i 1) (+ acc (channel-recv pong)))))))))
(out (task-join pinger))
(task-join ponger)

;; Short tasks, 100 at a time.
(define (batch k)
  (fold + 0 (map task-join (map (lambda (i) (spawn (lambda () (* i k)))) (iota 100)))))
(out (fold + 0 (map batch (iota 20))))

;; The idle ones end: half see their channel closed, half are cancelled.
(for-each (lambda (ch i) (when (even? i) (channel-close ch))) idle-channels (iota idle-count))
(for-each (lambda (t i) (when (odd? i) (task-cancel t))) idle (iota idle-count))
(out (length (filter (lambda (t) (guard (e (#t #f)) (eof-object? (task-join t)))) idle)))
(out (%live-task-count))
