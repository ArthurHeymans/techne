;;; (techne crypto): the vectors of RFC 4648, FIPS 180-4 and RFC 4231.

(import (techne crypto))

(test-begin "crypto")

;; RFC 4648, section 10.
(for-each (lambda (plain encoded)
            (test encoded (base64-encode plain))
            (test (string->utf8 plain) (base64-decode encoded)))
          '("" "f" "fo" "foo" "foob" "fooba" "foobar")
          '("" "Zg==" "Zm8=" "Zm9v" "Zm9vYg==" "Zm9vYmE=" "Zm9vYmFy"))
(test "666f6f626172" (hex-encode "foobar"))
(test (string->utf8 "foobar") (hex-decode "666F6f626172"))

;; URL-safe: its own two letters, no padding written, either read.
(test "+/8=" (base64-encode (bytevector 251 255)))
(test "-_8" (base64url-encode (bytevector 251 255)))
(test (bytevector 251 255) (base64url-decode "-_8"))
(test (bytevector 251 255) (base64url-decode "-_8="))
(for-each (lambda (bad) (test-error (base64-decode bad)))
          ;; Unpadded, whitespace, the other alphabet, padding inside, a
          ;; lone digit, bits past the last byte.
          '("Zg" "Zm9v\n" "-_8=" "Zm=v" "Z" "Zh=="))
;; Padding is optional in URL-safe base64, but whole when there.
(for-each (lambda (bad) (test-error (base64url-decode bad))) '("+/8" "Zg=" "Zh"))
;; Inputs longer than a chunk decode as a whole, and errors say where.
(let ((long (make-string 300000 #\A)))
  (test (make-bytevector 225000 0) (base64-decode long))
  (test "base64-decode: \"!\" at 299999 is not a base64 digit there"
        (guard (e (#t (error-object-message e))) (base64-decode (string-append (substring long 0 299999) "!")) #f))
  ;; Padding ends the text, even where a chunk ends.
  (let ((padded-at-chunk-end (string-append (substring long 0 262140) "Zg==" "Zg==")))
    (test-error (base64-decode padded-at-chunk-end))
    (test-error (base64url-decode padded-at-chunk-end))))
(for-each (lambda (bad) (test-error (hex-decode bad))) '("abc" "0g" " 00"))
(test-error (base64-encode 'foo))

;; FIPS 180-4 examples.
(test "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" (hex-encode (sha-256 "abc")))
(test "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
      (hex-encode (sha-256 "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")))
(test (string-append "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a"
                     "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f")
      (hex-encode (sha-512 "abc")))
;; A string hashes as its UTF-8.
(test (sha-256 (string->utf8 "é")) (sha-256 "é"))
;; Longer than the chunks hashing stops between.
(test "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
      (hex-encode (sha-256 (make-bytevector 1000000 (char->integer #\a)))))

;; RFC 4231, test cases 1, 2 and 6.
(test "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
      (hex-encode (hmac-sha-256 (make-bytevector 20 #x0b) "Hi There")))
(test (string-append "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde"
                     "daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854")
      (hex-encode (hmac-sha-512 (make-bytevector 20 #x0b) "Hi There")))
(test "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
      (hex-encode (hmac-sha-256 "Jefe" "what do ya want for nothing?")))
(test "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
      (hex-encode (hmac-sha-256 (make-bytevector 131 #xaa)
                                "Test Using Larger Than Block-Size Key - Hash Key First")))

(test #t (digest=? (sha-256 "a") (sha-256 "a")))
(test #f (digest=? (sha-256 "a") (sha-256 "b")))
(test #f (digest=? "abc" "ab"))
(test #t (digest=? "abc" (string->utf8 "abc")))

;; Random bytes: as many as asked for, different each time.
(test 0 (bytevector-length (random-bytes 0)))
(test 100000 (bytevector-length (random-bytes 100000)))
(test #f (equal? (random-bytes 16) (random-bytes 16)))
(test-error (random-bytes -1))

(test-end)
