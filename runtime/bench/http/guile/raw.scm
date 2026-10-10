;; The same minimal server as ../techne/server.scm on Guile fibers: a fiber
;; per connection, THREADS schedulers sharing the work (work stealing).
(use-modules (fibers) (ice-9 binary-ports) (ice-9 textual-ports)
             (rnrs bytevectors) (srfi srfi-13))

(define hello
  "HTTP/1.1 200 OK\r\nServer: guile\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!")

(define (work n)
  (let loop ((i 0) (acc 0))
    (if (= i n)
        acc
        (loop (+ i 1) (+ acc (string-length (number->string (* i i))))))))

(define (respond body)
  (string-append "HTTP/1.1 200 OK\r\nServer: guile\r\nContent-Type: text/plain\r\nContent-Length: "
                 (number->string (string-length body)) "\r\n\r\n" body))

(define (answer request)
  (if (string-prefix? "GET /work " request)
      (respond (number->string (work 1000)))
      hello))

(define (read-some port)
  (let ((bv (get-bytevector-some port)))
    (and (not (eof-object? bv)) (utf8->string bv))))

(define (handle port)
  (let loop ((pending ""))
    (let ((chunk (read-some port)))
      (if chunk
          (let next ((buf (if (string-null? pending) chunk (string-append pending chunk)))
                     (out '()))
            (let ((end (string-contains buf "\r\n\r\n")))
              (if end
                  (next (substring buf (+ end 4)) (cons (answer buf) out))
                  (begin
                    (unless (null? out)
                      (put-string port (apply string-append (reverse out)))
                      (force-output port))
                    (loop buf)))))
          (close-port port)))))

(define (serve port)
  (let ((s (socket PF_INET SOCK_STREAM 0)))
    (setsockopt s SOL_SOCKET SO_REUSEADDR 1)
    (bind s AF_INET (inet-pton AF_INET "127.0.0.1") port)
    (listen s 4096)
    (fcntl s F_SETFL (logior O_NONBLOCK (fcntl s F_GETFL)))
    (let loop ()
      (let ((c (car (accept s SOCK_NONBLOCK))))
        (setsockopt c IPPROTO_TCP TCP_NODELAY 1)
        (setvbuf c 'block 16384)
        (spawn-fiber (lambda ()
                       (catch #t (lambda () (handle c)) (lambda _ (close-port c)))))
        (loop)))))

(run-fibers (lambda () (serve (string->number (cadr (command-line)))))
            #:parallelism (string->number (or (getenv "THREADS") "1"))
            #:drain? #f)
