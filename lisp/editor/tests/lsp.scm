;;; The language server client's conversions: frame headers read
;;; strictly and case-insensitively, file URIs round-tripping through
;;; percent-coding, and positions read against a kept text.
;;;
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")
(require "../lsp.scm")

(define (len header) (frame-length (string->utf8 header)))

(check "a plain length" 5 (len "Content-Length: 5"))
(check "names are case-insensitive" 5 (len "content-length: 5"))
(check "other headers ride along" 12
       (len (string-append "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n" "CONTENT-LENGTH: 12")))
(check "a negative length is not a length" 'bad (len "Content-Length: -5"))
(check "trailing junk is not a length" 'bad (len "Content-Length: 5, please"))
(check "no length at all" 'bad (len "Content-Type: text/plain"))
(check "two lengths that agree" 7 (len (string-append "Content-Length: 7\r\n" "Content-Length: 7")))
(check "two lengths that differ" 'bad (len (string-append "Content-Length: 7\r\n" "Content-Length: 8")))
(check "a line with no colon" 'bad (len "not a header"))

(check "what needs coding is coded"
       "file:///tmp/100%25%20a%23b%3F.rs" (file-uri "/tmp/100% a#b?.rs"))
(check "and decoded back" "/tmp/100% a#b?.rs" (uri-file-path "file:///tmp/100%25%20a%23b%3F.rs"))
(check "non-ASCII by its bytes" "/tmp/café.rs" (uri-file-path "file://localhost/tmp/caf%C3%A9.rs"))
(check "another scheme is not a file" #f (uri-file-path "https://example.com/x.rs"))
(check "a host that is not this one" #f (uri-file-path "file://elsewhere/tmp/x.rs"))
(check "a % that codes nothing" #f (uri-file-path "file:///tmp/%GG.rs"))
(check "a trailing %" #f (uri-file-path "file:///tmp/x%"))

(check "a position in a kept text" 7
       (text-position-byte "ab\ncdéf\n" (dict "line" 1 "character" 3)))
(check "past a supplementary character" 5
       (text-position-byte "a😀b" (dict "line" 0 "character" 3)))
(check "a column past a CRLF line's text" 2
       (text-position-byte "ab\r\n" (dict "line" 0 "character" 99)))
(check "past the last line is the end" 3
       (text-position-byte "ab\n" (dict "line" 9 "character" 0)))
;; Deep in a non-ASCII text: one pass over it, not one rescan per
;; character (by character index this took half a minute).
(check "deep in a non-ASCII text" 59999
       (text-position-byte (apply string-append (make-list 20000 "é\n"))
                           (dict "line" 19999 "character" 1)))

(test-failures)
