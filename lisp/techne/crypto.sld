;;; Encodings, digests and MACs: base64 (RFC 4648), hex, SHA-256 and
;;; SHA-512 (FIPS 180-4), HMAC (RFC 2104), and comparing secrets in
;;; constant time.
;;;
;;; DATA is a bytevector, or a string taken as its UTF-8. Digests and MACs
;;; are bytevectors; `hex-encode` writes them as text. The work is done in
;;; Rust (crates/techne-vm/src/crypto.rs); hashing a long input stops on an
;;; interrupt.

(define-library (techne crypto)
  (export base64-encode base64-decode base64url-encode base64url-decode
          hex-encode hex-decode
          sha-256 sha-512 hmac-sha-256 hmac-sha-512 digest=?)
  (import (techne))
  (begin

    (define (base64-encode data)
      "Return DATA in base64, padded with `=`."
      (%base64-encode data #f))

    (define (base64-decode text)
      "Return the bytevector TEXT encodes in padded base64.
Anything else in TEXT, whitespace included, is an error."
      (%base64-decode text #f))

    (define (base64url-encode data)
      "Return DATA in URL-safe base64 (`-` and `_`), without padding."
      (%base64-encode data #t))

    (define (base64url-decode text)
      "Return the bytevector TEXT encodes in URL-safe base64.
Padding is optional; anything else in TEXT is an error."
      (%base64-decode text #t))

    (define (hex-encode data)
      "Return DATA in lowercase hexadecimal, two digits a byte."
      (%hex-encode data))

    (define (hex-decode text)
      "Return the bytevector TEXT writes in hexadecimal, in either case."
      (%hex-decode text))

    (define (sha-256 data)
      "Return the SHA-256 digest of DATA, a bytevector of 32 bytes."
      (%digest 'sha-256 data))

    (define (sha-512 data)
      "Return the SHA-512 digest of DATA, a bytevector of 64 bytes."
      (%digest 'sha-512 data))

    (define (hmac-sha-256 key data)
      "Return the HMAC-SHA-256 of DATA under KEY, a bytevector of 32 bytes."
      (%hmac 'sha-256 key data))

    (define (hmac-sha-512 key data)
      "Return the HMAC-SHA-512 of DATA under KEY, a bytevector of 64 bytes."
      (%hmac 'sha-512 key data))

    (define (digest=? a b)
      "Return #t if A and B hold the same bytes.
It takes a time that depends only on their lengths: compare MACs and
tokens with it, not `equal?`, which returns as soon as a byte differs."
      (%digest=? a b))))
