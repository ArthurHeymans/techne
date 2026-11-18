;;; Noninteractive selection for jj: edit only the temporary right tree.
(provide jj-hunk-options)

(define (octal-argument text)
  (let ((bytes (string->utf8 text)))
    (call-with-output-string
      (lambda (p)
        (do ((i 0 (+ i 1))) ((= i (bytevector-length bytes)))
          (let ((digits (number->string (bytevector-u8-ref bytes i) 8)))
            (display "\\0" p)
            (display (make-string (- 3 (string-length digits)) #\0) p)
            (display digits p)))))))

;; The diff editor's right tree is what jj keeps: what a split's first
;; change, a squash's moved part has, or what a restore leaves. With
;; BASE `left`, it is the left file with PATCH applied: the selection
;; alone. With BASE `right`, it is the right file with PATCH applied,
;; REVERSE to take a hunk out: all but the selection.
(define (jj-hunk-options path patch #:base [base 'left] #:reverse [reverse #f])
  "Return jj's diff editor arguments selecting PATCH of the file PATH.
BASE and REVERSE are as the comment above says."
  ;; Keep dynamic data out of both jj's $variable expansion and the shell's
  ;; grammar. The size cap keeps one encoded argument within common OS limits.
  (when (> (bytevector-length (string->utf8 patch)) 12000)
    (error "This hunk is too large for the noninteractive selector"))
  (let* ((script (string-append
                   "set -eu; p=$(printf '%b!' \"$3\"); p=${p%!}; "
                   "[ -f \"$1/$p\" ] && [ ! -L \"$1/$p\" ] && [ -f \"$2/$p\" ] && [ ! -L \"$2/$p\" ] || exit 1; "
                   (if (eq? base 'left) "cp -- \"$1/$p\" \"$2/$p\"; " "")
                   "printf '%b' \"$4\" | git -C \"$2\" -c apply.ignoreWhitespace=false apply --no-index --whitespace=nowarn"
                   (if reverse " --reverse" "") " -"))
         (args (vector "-c" script "techne-vc-hunk" "$left" "$right" (octal-argument path) (octal-argument patch))))
    (list "--tool" "techne-vc-hunk"
          "--config" "merge-tools.techne-vc-hunk.program=\"sh\""
          "--config" (string-append "merge-tools.techne-vc-hunk.edit-args=" (json->string args)))))
