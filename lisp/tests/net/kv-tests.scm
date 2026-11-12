(require "../../test.scm")
(require "../../cloud/kv.scm")

;; A directory of this run's own, under the system's.
(define root (string-append "/tmp/techne-kv-tests-" (number->string (exact (floor (* 1000 (current-second)))))))
(define now 1000)
(define (open) (make-kv-store (string-append root "/kv/store") #:clock (lambda () now) #:max-bytes 200))
(define store (open))

(define (fails? thunk) (guard (e (#t #t)) (thunk) #f))
(define data (bytevector 0 1 2 255 195 169))
(define (bytes s) (string->utf8 s))

(check "nothing kept yet" #f (kv-store-get store "cache" "a"))
(check "nor listed" '() (kv-store-list store "cache"))
(check "a default" 'none (kv-store-get store "cache" "a" 'none))

(kv-store-put! store "cache" "b" data)
(kv-store-put! store "cache" "a" (bytes "one"))
(kv-store-put! store "cache" "a" (bytes "two"))
(check "the last write wins" (bytes "two") (kv-store-get store "cache" "a"))
(check "bytes, as they were put" data (kv-store-get store "cache" "b"))
(check "keys in order" '("a" "b") (kv-store-list store "cache"))
(check "by prefix" '("b") (kv-store-list store "cache" #:prefix "b"))
(check "at most the limit" '("a") (kv-store-list store "cache" #:limit 1))
(check "a namespace of its own" #f (kv-store-get store "other" "a"))

;; A value kept empty is found, as the operation says.
(kv-store-put! store "cache" "f" (bytevector))
(check "found, and empty" (dict 'found #t 'value (bytevector)) (kv-store-handle store "cache" (dict 'op 'get 'key "f")))
(check "not found" (dict 'found #f) (kv-store-handle store "cache" (dict 'op 'get 'key "g")))
(kv-store-delete! store "cache" "f")
(check "deleted" (dict 'found #f) (kv-store-handle store "cache" (dict 'op 'get 'key "f")))
(kv-store-delete! store "cache" "f")

;; Expiry, by the store's clock.
(kv-store-put! store "cache" "soon" (bytes "x") #:ttl 10)
(set! now 1009)
(check "kept until its time" (bytes "x") (kv-store-get store "cache" "soon"))
(set! now 1010)
(check "expired" #f (kv-store-get store "cache" "soon"))
(check "and not listed" '("a" "b") (kv-store-list store "cache"))

;; Another store on the directory reads what was written.
(set! now 1000)
(kv-store-put! store "cache" "later" (bytes "y") #:ttl 5)
(let ((again (open)))
  (check "kept in its file" (bytes "two") (kv-store-get again "cache" "a"))
  (check "with its bytes" data (kv-store-get again "cache" "b"))
  (check "and its expiry" (bytes "y") (kv-store-get again "cache" "later"))
  (set! now 1005)
  (check "which holds there too" #f (kv-store-get again "cache" "later")))
(set! now 1000)

;; Bounds: refused, and nothing changed.
(check-true "an empty key" (fails? (lambda () (kv-store-put! store "cache" "" data))))
(check-true "a key that is no string" (fails? (lambda () (kv-store-get store "cache" 'a))))
(check-true "a key too long" (fails? (lambda () (kv-store-put! store "cache" (make-string 513 #\k) data))))
(check-true "a value that is no bytes" (fails? (lambda () (kv-store-put! store "cache" "a" "text"))))
(check-true "a namespace that names no file" (fails? (lambda () (kv-store-get store "../cache" "a"))))
(check-true "nor an absolute one" (fails? (lambda () (kv-store-put! store "/tmp/x" "a" data))))
(check-true "a TTL that is none" (fails? (lambda () (kv-store-put! store "cache" "a" data #:ttl -1))))
(check-true "a namespace full" (fails? (lambda () (kv-store-put! store "cache" "big" (make-bytevector 300 1)))))
(check "the one refused is not kept" #f (kv-store-get store "cache" "big"))
(check "and the rest is" (bytes "two") (kv-store-get store "cache" "a"))
(check-true "a limit past the most" (fails? (lambda () (kv-store-list store "cache" #:limit 1001))))
(check-true "an operation unknown" (fails? (lambda () (kv-store-handle store "cache" (dict 'op 'drop)))))
;; Replacing a value counts the room it gives back.
(kv-store-put! store "small" "k" (make-bytevector 150 1))
(kv-store-put! store "small" "k" (make-bytevector 150 2))
(check "replaced within the bound" (make-bytevector 150 2) (kv-store-get store "small" "k"))
;; Bytes are counted, not characters: a key of 100 two-byte characters.
(check-true "bytes are counted" (fails? (lambda () (kv-store-put! store "wide" (make-string 100 #\é) (make-bytevector 1 0)))))
;; What expired takes no room.
(kv-store-put! store "brief" "a" (make-bytevector 150 1) #:ttl 5)
(check-true "full while it lasts" (fails? (lambda () (kv-store-put! store "brief" "b" (make-bytevector 150 1)))))
(set! now 1005)
(kv-store-put! store "brief" "b" (make-bytevector 150 1))
(check "room again once it expired" '("b") (kv-store-list store "brief"))
(set! now 1000)
;; A file that is no namespace's is refused, not read as empty.
(replace-file (string-append root "/kv/store/broken.kv") "((\"k\" not-base64 #f))")
(check-true "a file that is not one" (fails? (lambda () (kv-store-get store "broken" "k"))))
(check-true "nor written over" (fails? (lambda () (kv-store-put! store "broken" "k" data))))

(check "put, as asked" #t (kv-store-handle store "ops" (dict 'op 'put 'key "k" 'value data 'ttl #f)))
(check "list, as asked" '("k") (kv-store-handle store "ops" (dict 'op 'list 'prefix "" 'limit 10)))
(check "delete, as asked" #t (kv-store-handle store "ops" (dict 'op 'delete 'key "k")))

(check-true "namespace names" (and (kv-namespace-name? "cache-1_a") (not (kv-namespace-name? "Cache")) (not (kv-namespace-name? ""))
                                   (not (kv-namespace-name? "a/b")) (not (kv-namespace-name? "a.b"))))

(delete-path root #t)
(test-summary)
