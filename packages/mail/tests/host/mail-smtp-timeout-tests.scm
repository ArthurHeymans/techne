(import (techne) (mail smtp))
(import (techne test))
(test-file #:script #t #:environment 'host)

;; A local peer accepts TCP and reads ClientHello, but never speaks TLS.
(let* ((server (tcp-listen 0))
       (port (cadr (tcp-local-address server)))
       (hello? #f)
       (closed? #f)
       (peer (spawn (lambda ()
                      (call-with-socket (tcp-accept server)
                        (lambda (c)
                          (set! hello? (bytevector? (tcp-read-bytes c)))
                          (let loop ()
                            (if (eof-object? (tcp-read-bytes c)) (set! closed? #t) (loop)))))))))
  (check "SMTP times out during TLS and reports an ordinary submission error"
         "SMTP submission timed out"
         (guard (e (#t (error-object-message e)))
           (mail-submit! "127.0.0.1" port "user" "password" "a@example.org" '("b@example.org") "hello"
                         #:timeout-ms 300 #:on-data (lambda () (error "DATA must not start")))))
  (task-join peer)
  (check "TLS began" #t hello?)
  (check "timed-out submission closed its socket" #t closed?)
  (tcp-close server))
(test-summary)
