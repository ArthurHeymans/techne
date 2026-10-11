;;; JSON text to data and back.

(test-begin "json")

(test {"a" #(1 2.5 -300.0 #t #f null) "b" {}} (string->json " {\"a\": [1, 2.5, -3e2, true, false, null], \"b\": {}} "))
(test 123456789012345678901234567890 (string->json "123456789012345678901234567890"))
(test -0.5 (string->json "-0.5"))
(test "é😀\n\"/" (string->json "\"\\u00e9\\ud83d\\ude00\\n\\\"\\/\""))
(test "é" (string->json "\"é\""))
(test #(1) (string->json (string->utf8 "[1]")))
;; A key given twice keeps its last value.
(test {"a" 2} (string->json "{\"a\": 1, \"a\": 2}"))

;; An object made at once is the dict made a key at a time.
(let ((keys (map (lambda (i) (string-append "k" (number->string i))) (iota 3000))))
  (test (alist->dict (map (lambda (k) (cons k 1)) keys))
        (string->json (string-append "{" (string-join (map (lambda (k) (string-append "\"" k "\": 1")) keys) ",") "}"))))
;; Keys are shared, and cannot be changed.
(let ((v (string->json "[{\"name\": 1}, {\"name\": 2}]")))
  (test #t (eq? (car (dict-keys (vector-ref v 0))) (car (dict-keys (vector-ref v 1)))))
  (test-error (string-set! (car (dict-keys (vector-ref v 0))) 0 #\x)))

;; What JSON does not allow.
(for-each (lambda (text) (test-error (string->json text)))
          '("[1,]" "{\"a\" 1}" "{1: 2}" "01" "1." ".5" "+1" "1e" "-" "\"\\x\"" "\"\\ud800\"" "\"a\nb\""
            "[1] 2" "" "nul" "[" "{\"a\": 1" "'a'" "NaN"))
(test-error (string->json (bytevector #xff)))
(test-error (string->json (make-string 2000 #\[)))
(test-error (string->json "1e400"))
;; As deep as reading allows, writing allows.
(let ((deep (string-append (make-string 1000 #\[) "0" (make-string 1000 #\]))))
  (test deep (json->string (string->json deep)))
  (test-error (json->string (vector (string->json deep)))))

;; Writing: members in key order, no spaces; the same data, the same text.
(test "{\"a\":[1,2],\"b\":null,\"c\":true,\"d\":\"x\\\"y\\n\",\"e\":{}}"
      (json->string {e {} d "x\"y\n" c #t b null a (1 2)}))
(test "[1.5,-2,123456789012345678901234567890,false]" (json->string (vector 1.5 -2 123456789012345678901234567890 #f)))
(test "[]" (json->string '()))
(test "\"\\u0001\"" (json->string (string (integer->char 1))))
(let ((d (string->json "{\"x\": [1, {\"y\": null}], \"z\": \"é\"}")))
  (test d (string->json (json->string d))))
(test-error (json->string 1/3))
(test-error (json->string +inf.0))
(test-error (json->string 'other))
(test-error (json->string {1 2}))
(test-error (json->string {a 1 "a" 2}))
(test-error (json->string '(1 . 2)))
(test-error (json->string (let ((v (vector 1))) (vector-set! v 0 v) v)))

(test-end)
