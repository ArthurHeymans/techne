;; A minimal HTTP/1.1 keep-alive server, one task per connection.
;; GET /work answers with (work 1000); anything else with "Hello, World!".
;; Every server in ../ does exactly this.

(define hello
  "HTTP/1.1 200 OK\r\nServer: techne\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!")

(define (work n)
  (let loop ((i 0) (acc 0))
    (if (= i n)
        acc
        (loop (+ i 1) (+ acc (string-length (number->string (* i i))))))))

(define (respond body)
  (string-append "HTTP/1.1 200 OK\r\nServer: techne\r\nContent-Type: text/plain\r\nContent-Length: "
                 (number->string (string-length body)) "\r\n\r\n" body))

(define (answer request)
  (if (string-prefix? "GET /work " request)
      (respond (number->string (work 1000)))
      hello))

;; Answer every complete request in what has arrived, in one write.
(define (handle conn)
  (let loop ((pending ""))
    (let ((chunk (tcp-read conn)))
      (if chunk
          (let next ((buf (if (string=? pending "") chunk (string-append pending chunk)))
                     (out '()))
            (let ((end (string-contains buf "\r\n\r\n")))
              (if end
                  (next (substring buf (+ end 4) (string-length buf)) (cons (answer buf) out))
                  (begin
                    (cond ((null? out))
                          ((null? (cdr out)) (tcp-write conn (car out)))
                          (else (tcp-write conn (apply string-append (reverse out)))))
                    (loop buf)))))
          (tcp-close conn)))))

(define (serve port)
  (let ((listener (tcp-listen port)))
    (let loop ()
      (let ((conn (tcp-accept listener)))
        (spawn (lambda ()
                 (guard (e (#t (tcp-close conn)))
                   (handle conn))))
        (loop)))))
