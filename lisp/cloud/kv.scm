;;; A KV store for deployments' bindings (doc/serverless.md, "Storage"):
;;; namespaces of keys, strings, and values, bytevectors, each of which
;;; may expire. The last write to a key wins. What a value's bytes stand
;;; for is its writer's to know: the supervisor keeps a deployment's data
;;; encoded, so that nothing of a tenant's is read into this world but
;;; strings and bytes.
;;;
;;;   (define store (make-kv-store "/var/lib/techne/kv"))
;;;   (kv-store-put! store "cache" "weather" (string->utf8 "21") #:ttl 600)
;;;   (kv-store-get store "cache" "weather")   ; => #u8(50 49), or #f
;;;
;;; A namespace is kept in a file of its own in the store's directory,
;;; written whole, in place of the one before, at each change: what a
;;; `put!` or `delete!` returned from is in the file, and a file is never
;;; half written. (It is not forced to the disk: a power loss may lose
;;; the last changes. Writing it whole costs its size at each change;
;;; a log would not, and needs files opened to append.) A namespace is
;;; read when first used, and kept in memory from then on: one store owns
;;; a directory at a time.
;;;
;;; `kv-serve` answers the operations a supervisor's workers ask
;;; (`%store-next`, `%store-reply`: crates/techne-host/src/supervisor.rs),
;;; each a dict of its `op` and fields, as `kv-store-handle` takes them.
;;; The namespace of each was looked up by the supervisor from the
;;; deployment's binding: nothing here decides who may reach what.
;;;
;;; Bounds: a key is 1 to 512 bytes of UTF-8, a value at most a mebibyte,
;;; a namespace at most MAX-BYTES of keys and values that have not
;;; expired. What is past them is an error, and changes nothing.

(import (techne crypto))

(provide make-kv-store kv-store? kv-store-get kv-store-put! kv-store-delete! kv-store-list
         kv-store-handle kv-serve kv-serve-tenants kv-namespace-name?)

(define max-key-bytes 512)
(define max-value-bytes (* 1024 1024))
(define max-listed 1000)

(define-record-type kv-store
  (%make-kv-store directory clock max-bytes namespaces)
  kv-store?
  (directory kv-store-directory)
  (clock kv-store-clock)
  (max-bytes kv-store-max-bytes)
  ;; By name: a dict of each key's entry, #(value expires size).
  (namespaces kv-store-namespaces set-kv-store-namespaces!))

(define (make-kv-store directory #:clock [clock current-second] #:max-bytes [max-bytes (* 64 1024 1024)])
  "Return a KV store keeping its namespaces in DIRECTORY.
The directory is made when something is first kept. CLOCK returns the
time in seconds, by which values expire; a namespace holds at most
MAX-BYTES of keys and values."
  (%make-kv-store directory clock max-bytes (dict)))

;; Make DIRECTORY and those above it that are missing.
(define (make-directories directory)
  (unless (file-exists? directory)
    (let ((parent (path-parent directory)))
      (when (and parent (not (string=? parent directory))) (make-directories parent)))
    (create-directory directory #o700)))

;; The directory PATH is in, or #f if it names none.
(define (path-parent path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) #f)
          ((char=? (string-ref path i) #\/) (if (= i 0) "/" (substring path 0 i)))
          (else (loop (- i 1))))))

(define (kv-namespace-name? name)
  "Return #t if NAME can name a namespace.
A string of 1 to 64 lowercase letters, digits, `-` and `_`: it names a
file."
  (and (string? name)
       (<= 1 (string-length name) 64)
       (chars-every? (lambda (c) (or (char<=? #\a c #\z) (char<=? #\0 c #\9) (char=? c #\-) (char=? c #\_))) name)))

(define (chars-every? pred s)
  (let loop ((i 0))
    (or (= i (string-length s)) (and (pred (string-ref s i)) (loop (+ i 1))))))

(define (namespace-file store name)
  (string-append (kv-store-directory store) "/" name ".kv"))

(define (entry-value e) (vector-ref e 0))
(define (entry-expires e) (vector-ref e 1))
(define (entry-size e) (vector-ref e 2))

(define (live? store e)
  (let ((expires (entry-expires e)))
    (or (not expires) (< ((kv-store-clock store)) expires))))

(define (entry-of key value expires)
  (vector value expires (+ (bytevector-length (string->utf8 key)) (bytevector-length value))))

;; ENTRIES without those that expired.
(define (live-entries store entries)
  (fold (lambda (key kept) (if (live? store (dict-ref entries key)) kept (dict-delete kept key)))
        entries (dict-keys entries)))

;; The entries of namespace NAME, read from its file when first used:
;; a list of (key value expires) each, the value in base64, those that
;; expired left out. A file that is not that is an error, each time:
;; nothing is kept over what cannot be read.
(define (namespace store name)
  (unless (kv-namespace-name? name) (error "Not a namespace name:" name))
  (or (dict-ref (kv-store-namespaces store) name #f)
      (let* ((file (namespace-file store name))
             (kept (if (file-exists? file) (call-with-input-file file read) '()))
             (entries (fold (lambda (item entries)
                              (unless (and (list? item) (= (length item) 3) (string? (car item)) (string? (cadr item))
                                           (or (not (caddr item)) (real? (caddr item))))
                                (error "The namespace's file is not one:" name))
                              (dict-set entries (car item) (entry-of (car item) (base64-decode (cadr item)) (caddr item))))
                            (dict)
                            (if (list? kept) kept (error "The namespace's file is not one:" name))))
             (entries (live-entries store entries)))
        (set-kv-store-namespaces! store (dict-set (kv-store-namespaces store) name entries))
        entries)))

;; Namespace NAME is ENTRIES from now on, in its file first: if that
;; fails, it is what it was.
(define (set-namespace! store name entries)
  (let ((live (filter (lambda (key) (live? store (dict-ref entries key))) (dict-keys entries))))
    (make-directories (kv-store-directory store))
    (replace-file (namespace-file store name)
                  (call-with-output-string
                   (lambda (port)
                     (write (map (lambda (key)
                                   (let ((e (dict-ref entries key)))
                                     (list key (base64-encode (entry-value e)) (entry-expires e))))
                                 (list-sort string<? live))
                            port))))
    (set-kv-store-namespaces! store
                              (dict-set (kv-store-namespaces store) name
                                        (fold (lambda (key kept) (dict-set kept key (dict-ref entries key))) (dict) live)))))

(define (check-key key)
  (unless (and (string? key) (<= 1 (bytevector-length (string->utf8 key)) max-key-bytes))
    (error "A key is a string of 1 to 512 bytes:" key)))

(define (kv-store-get store name key [default #f])
  "Return the value kept under KEY in namespace NAME of STORE.
DEFAULT if none is, or it has expired."
  (check-key key)
  (let ((e (dict-ref (namespace store name) key #f)))
    (if (and e (live? store e)) (entry-value e) default)))

(define (kv-store-put! store name key value #:ttl [ttl #f])
  "Keep VALUE, a bytevector, under KEY in namespace NAME of STORE.
With TTL, a positive number of seconds, for that long. A value past a
mebibyte, or one the namespace has no room for beside what has not
expired, is an error and changes nothing."
  (check-key key)
  (unless (bytevector? value) (error "A value is a bytevector, under the key:" key))
  (unless (or (not ttl) (and (real? ttl) (> ttl 0))) (error "A TTL is a positive number of seconds:" ttl))
  (when (> (bytevector-length value) max-value-bytes) (error "A value is at most a mebibyte, under the key:" key))
  (let* ((entries (live-entries store (namespace store name)))
         (entry (entry-of key value (and ttl (+ ((kv-store-clock store)) ttl))))
         (before (let ((e (dict-ref entries key #f))) (if e (entry-size e) 0)))
         (used (fold (lambda (k total) (+ total (entry-size (dict-ref entries k)))) 0 (dict-keys entries))))
    (when (> (+ (- used before) (entry-size entry)) (kv-store-max-bytes store)) (error "The namespace is full:" name))
    (set-namespace! store name (dict-set entries key entry))))

(define (kv-store-delete! store name key)
  "Keep nothing under KEY in namespace NAME of STORE any more."
  (check-key key)
  (let ((entries (namespace store name)))
    (when (dict-ref entries key #f)
      (set-namespace! store name (dict-delete entries key)))))

(define (kv-store-list store name #:prefix [prefix ""] #:limit [limit max-listed])
  "Return the keys of namespace NAME of STORE that start with PREFIX.
In order, those expired left out, at most LIMIT of them, which is at
most 1000."
  (unless (string? prefix) (error "A prefix is a string:" prefix))
  (unless (and (exact-integer? limit) (<= 1 limit max-listed)) (error "A limit is an integer from 1 to 1000:" limit))
  (let* ((entries (namespace store name))
         (keys (list-sort string<? (filter (lambda (key) (and (string-prefix? prefix key) (live? store (dict-ref entries key))))
                                           (dict-keys entries)))))
    (if (> (length keys) limit) (take keys limit) keys)))

(define (kv-store-handle store name request)
  "Return STORE's answer to REQUEST, an operation on its namespace NAME.
REQUEST is a dict of its `op` and fields, as the supervisor hands on
what a deployment's `kv-get`, `kv-put!`, `kv-delete!` and `kv-list`
ask, values as bytevectors: `get` (`key`) answers a dict of `found` and
`value`; `put` (`key`, `value`, `ttl`) and `delete` (`key`) answer #t;
`list` (`prefix`, `limit`) answers the keys."
  (unless (dict? request) (error "Not an operation:" request))
  (case (dict-ref request 'op #f)
    ((get)
     (let* ((missing (list 'missing))
            (value (kv-store-get store name (dict-ref request 'key #f) missing)))
       (if (eq? value missing) (dict 'found #f) (dict 'found #t 'value value))))
    ((put)
     (kv-store-put! store name (dict-ref request 'key #f) (dict-ref request 'value #f) #:ttl (dict-ref request 'ttl #f))
     #t)
    ((delete) (kv-store-delete! store name (dict-ref request 'key #f)) #t)
    ((list) (kv-store-list store name #:prefix (dict-ref request 'prefix "") #:limit (dict-ref request 'limit max-listed)))
    (else (error "No such operation:" (dict-ref request 'op #f)))))

(define (kv-serve-tenants directory #:max-bytes [max-bytes (* 64 1024 1024)])
  "Answer the operations of a fleet's tenants, for ever.
Each tenant has a store of its own, in a directory of its own under
DIRECTORY, by its name, which must be one that names a file as a
namespace's does: no tenant reaches another's namespaces, whatever they
are called. A namespace holds at most MAX-BYTES."
  (let loop ((stores (dict)))
    (let* ((asked (%store-next))
           (id (car asked))
           (what (cadr asked))
           (tenant (car what))
           (store (and (kv-namespace-name? tenant)
                       (or (dict-ref stores tenant #f)
                           (make-kv-store (string-append directory "/" tenant) #:max-bytes max-bytes)))))
      (guard (e (#t (%store-reply id #f (if (error-object? e) (condition/report-string e) "the store failed"))))
        (unless store (error "Not a tenant's name:" tenant))
        (%store-reply id #t (kv-store-handle store (cadr what) (caddr what))))
      (loop (if store (dict-set stores tenant store) stores)))))

(define (kv-serve store)
  "Answer with STORE the operations the supervisor's workers ask, for ever.
Each is answered before the next is taken; one that fails is answered
with why."
  (let loop ()
    (let* ((asked (%store-next))
           (id (car asked))
           (what (cadr asked)))
      (guard (e (#t (%store-reply id #f (if (error-object? e) (condition/report-string e) "the store failed"))))
        (%store-reply id #t (kv-store-handle store (car what) (cadr what))))
      (loop))))
