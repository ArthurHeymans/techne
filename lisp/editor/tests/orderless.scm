;;; Orderless completion across the shared matcher, minibuffer, action
;;; chooser, file completion, and in-buffer completion popup.

(import (techne test))
(test-file #:environment 'editor)

(require "../session.scm")
(require "../dispatch.scm")
(require "../modes.scm")
(require "../commands.scm")
(require "../targets.scm")
(require "../orderless.scm")
(require "../files.scm")
(require "../minibuffer.scm")
(require "../buffers.scm")
(require "../emacs.scm")
(require "../completion.scm")

(define (row-text row)
  (apply string-append (map (lambda (run) (if (string? run) run (car run))) (car row))))

(define (row-match-texts row)
  (filter-map (lambda (run) (and (pair? run) (eq? (cadr run) 'match) (car run))) (car row)))

(define (mb-shown s)
  (let ((mb (editor-minibuffer s)))
    (if mb (map row-text (caddr mb)) '())))

(define (mb-highlights s)
  (let ((mb (editor-minibuffer s)))
    (if mb (map row-match-texts (caddr mb)) '())))

(define (popup-shown s)
  (let ((pop (editor-completion s)))
    (if pop (map row-text (caddr pop)) '())))

(define (make-test-session)
  (let* ((d (make-document ""))
         (s (make-session d "user" emacs-profile)))
    (set-current-session! s)
    (values d s)))

(define-test "orderless-split-and-styles"
  (check "split plain words" '("foo" "bar") (orderless-split "  foo   bar  "))
  (check "escaped space stays inside part" '("foo bar" "baz") (orderless-split "foo\\ bar baz"))
  (check "double backslash before space preserves both and splits" '("foo\\\\" "bar") (orderless-split "foo\\\\ bar"))
  (check "triple backslash before space keeps two and literal space" '("foo\\\\ bar") (orderless-split "foo\\\\\\ bar"))
  (check "quad backslash before space preserves four and splits" '("foo\\\\\\\\" "bar") (orderless-split "foo\\\\\\\\ bar"))
  (check "regexp escapes preserved" '("foo\\d+" "a\\.b") (orderless-split "foo\\d+ a\\.b"))
  (check "trailing escaped space" '("foo ") (orderless-split "foo\\ "))

  ;; =literal vs literal-or-regexp union
  (let ((lit (orderless-parse "=foo.bar"))
        (re (orderless-parse "foo.bar"))
        (plus (orderless-parse "foo+bar")))
    (check "=literal matches literal dot" #t (orderless-matches? "foo.bar" lit))
    (check "=literal rejects arbitrary char" #f (orderless-matches? "fooxbar" lit))
    (check "plain literal-or-regexp matches arbitrary char" #t (orderless-matches? "fooxbar" re))
    (check "plain foo+bar still matches literal foo+bar" #t (orderless-matches? "foo+bar" plus))
    (check "plain foo+bar also matches regexp foooobar" #t (orderless-matches? "foooobar" plus))
    (check "bare = matches anything with empty spans" '() (orderless-match-spans "anything" (orderless-parse "="))))

  ;; Expanded Unicode case-folding maps highlight spans back to original char indices
  (check "expanded fold =x highlights x in İx!" '((1 2))
         (orderless-match-spans "İx!" (orderless-parse "=x")))
  (check "expanded fold ~x highlights x in İx!" '((1 2))
         (orderless-match-spans "İx!" (orderless-parse "~x")))
  (check "expanded fold =x highlights x in İx" '((1 2))
         (orderless-match-spans "İx" (orderless-parse "=x")))

  ;; ~flex
  (let ((flex (orderless-parse "~dscrb")))
    (check "~flex matches subsequence" #t (orderless-matches? "describe-named-command" flex))
    (check "~flex rejects out-of-order" #f (orderless-matches? "brcsd" flex))
    (check "~flex spans are merged" '((0 1) (2 5) (6 7))
           (orderless-match-spans "describe-named-command" flex))
    (check "~flex smart-case respects uppercase" #f
           (orderless-matches? "describe-named-command" (orderless-parse "~Dscrb")))
    (check "bare ~ matches anything with empty spans" '() (orderless-match-spans "anything" (orderless-parse "~"))))

  ;; invalid regexp falls back to literal
  (let ((bad (orderless-parse "foo(")))
    (check "invalid regexp falls back to literal" 'literal (orderless-part-kind (car bad)))
    (check "invalid regexp matches literally" #t (orderless-matches? "call foo(x)" bad))
    (check "invalid regexp rejects non-literal" #f (orderless-matches? "call foo" bad)))

  ;; unmatched closing delimiters are plain literals, not regexps
  (for-each (lambda (pat)
              (check (string-append "simple pattern: " pat) #t (orderless-simple-pattern? pat))
              (check (string-append "literal kind: " pat) 'literal (orderless-part-kind (car (orderless-parse pat)))))
            '("]a" "}a" ")a"))

  ;; UTF-8 regexp spans return character indices, not byte indices
  (let ((spans (orderless-match-spans "éa b" (orderless-parse "a.b"))))
    (check "utf-8 regexp char spans" '((1 4)) spans)
    (check "substring with char spans" "a b" (substring "éa b" (caar spans) (cadar spans))))

  ;; Overlapping parts merge cleanly
  (check "overlapping spans merge" '((0 5))
         (orderless-match-spans "ref-vector" (orderless-parse "ref ef-v"))))

(define-test "minibuffer-m-spc-and-orderless-matching"
  (let-values (((d s) (make-test-session)))
    (let ((chosen #f))
      (completing-read s "Pick: "
                       '("describe-named-command" "describe-key" "foo.bar" "fooxbar" "alpha-one" "beta-two" "hello world")
                       #:accept (lambda (_s c) (set! chosen (candidate-text c))))

      ;; M-SPC separates unordered parts just like SPC
      (type-text s "named")
      (press-keys s "M-SPC")
      (type-text s "desc")
      (check "M-SPC inserts space" "named desc" (minibuffer-input s))
      (check "M-SPC unordered match" '("describe-named-command") (mb-shown s))
      (check "M-SPC highlights both parts" '(("desc" "named")) (mb-highlights s))

      ;; Clear and test ~flex with M-SPC
      (press-keys s "C-a C-k")
      (type-text s "~dscrb")
      (press-keys s "M-SPC")
      (type-text s "~nmd")
      (check "~flex with M-SPC" '("describe-named-command") (mb-shown s))

      ;; =literal vs regexp
      (press-keys s "C-a C-k")
      (type-text s "=foo.bar")
      (check "=literal excludes fooxbar" '("foo.bar") (mb-shown s))
      (press-keys s "C-a C-k")
      (type-text s "foo.bar")
      (check "regexp matches both foo.bar and fooxbar" '("foo.bar" "fooxbar") (mb-shown s))

      ;; Escaped space matches literal space
      (press-keys s "C-a C-k")
      (type-text s "hello\\ world")
      (check "escaped space matches literal space" '("hello world") (mb-shown s))

      ;; Regexp broadening: typing |beta after alpha must broaden matches!
      (press-keys s "C-a C-k")
      (type-text s "alpha")
      (check "narrowed to alpha" '("alpha-one") (mb-shown s))
      (type-text s "|beta")
      (check "regexp alternation broadens matches" '("alpha-one" "beta-two") (mb-shown s))

      (press-keys s "RET")
      (check "accepts first match" "alpha-one" chosen))

    ;; TAB preserves the selected candidate even when its text contains
    ;; regexp metacharacters that also match an earlier candidate.
    (let ((chosen #f))
      (completing-read s "Pick: " '("fooXbar" "foo.bar")
                       #:accept (lambda (_s c) (set! chosen (candidate-text c))))
      (type-text s "foo")
      (press-keys s "C-n TAB RET")
      (check "TAB preserves selected foo.bar over earlier fooXbar" "foo.bar" chosen))))

(define-test "action-chooser-orderless-and-m-spc"
  (let-values (((d s) (make-test-session)))
    (let ((ran #f))
      (define-action orderless-test-kind (alpha-inspect-item _s v) "Inspect item." (set! ran (list 'inspect v)))
      (define-action orderless-test-kind (beta-delete-item _s v) "Delete item." (set! ran (list 'delete v)))
      (act-on! s (target 'orderless-test-kind "payload") "item")
      (press-keys s "?")
      (type-text s "item")
      (press-keys s "M-SPC")
      (type-text s "~dl")
      (check "action chooser matches via M-SPC and ~flex" '("beta-delete-item") (mb-shown s))
      (press-keys s "RET")
      (check "action ran" '(delete "payload") ran))))

(define-test "file-completion-orderless-preserves-directory-prefix"
  (let-values (((d s) (make-test-session)))
    (let ((chosen #f))
      (completing-read s "File: "
                       (lambda (input)
                         (let ((dir (directory-of input)))
                           (map (lambda (name) (candidate name #:target (target 'file (string-append dir name))))
                                '("orderless.scm" "minibuffer.scm" "notes v1.txt" "foo.bar" "fooxbar"))))
                       #:initial "/tmp/dir/"
                       #:pattern file-name
                       #:keymap file-name-map
                       #:accept (lambda (_s c) (set! chosen (target-value (candidate-target c)))))
      (type-text s "~ord")
      (press-keys s "M-SPC")
      (type-text s "scm")
      (check "file completion preserves dir prefix and applies orderless to filename"
             '("orderless.scm") (mb-shown s))
      (press-keys s "RET")
      (check "accepted full path" "/tmp/dir/orderless.scm" chosen))))

(define-test "popup-orderless-and-zero-match-persistence"
  (define-mode test-orderless-popup-mode "Fixed completion source for Orderless popup tests."
    #:keys '(("C-M-i" completion-at-point))
    #:complete (lambda (b p)
                 (cons (car (identifier-span (buffer-document b) p))
                       (map candidate '("vector-ref" "ref-vector" "list-ref" "foo.bar" "fooxbar")))))
  (let* ((d (make-document ""))
         (_ (remember-buffer! (make-buffer d "*orderless-popup*" 'test-orderless-popup-mode)))
         (s (make-session d "user" emacs-profile)))
    (set-current-session! s)

    ;; Open popup, insert M-SPC, type a temporarily non-matching query -> popup stays open!
    (type-text s "(ref")
    (press-keys s "C-M-i")
    (check "initial popup matches" '("vector-ref" "ref-vector" "list-ref") (popup-shown s))
    (press-keys s "M-SPC")
    (type-text s "zzz")
    (check "popup stays open on 0 matches after M-SPC" #t (and (editor-completion s) #t))
    (check "popup shows visible non-selectable No match row" '("No match") (popup-shown s))
    (check "No match row is not selected" #f (list-ref (editor-completion s) 3))
    ;; Navigating with 0 matches must not error or select No match
    (press-keys s "C-n")
    (check "no error navigating 0-match popup" #f (sget s 'message))
    (check "No match row remains unselected after C-n" #f (list-ref (editor-completion s) 3))
    ;; Deleting back restores matches immediately
    (press-keys s "DEL DEL DEL")
    (type-text s "~vc")
    (check "deleting back and typing ~flex restores matches" '("vector-ref" "ref-vector") (popup-shown s))
    ;; Deleting back through the separator clears separated state
    (press-keys s "DEL DEL DEL DEL")
    (type-text s " ")
    (check "deleting separator restores space-closes-popup behavior" #f (editor-completion s))

    ;; Re-open and accept multi-part ~flex query
    (type-text s "(ref")
    (press-keys s "C-M-i")
    (press-keys s "M-SPC")
    (type-text s "~vc")
    (press-keys s "C-n RET")
    (check "accept replaces whole multi-part query" "(ref (vector-ref" (document-string d))
    (check "popup closes on accept" #f (editor-completion s))

    ;; Regexp character class after M-SPC stays open and matches
    (type-text s " (ref")
    (press-keys s "C-M-i")
    (press-keys s "M-SPC")
    (type-text s "[vl]")
    (check "regexp bracket class after M-SPC keeps popup open and matches"
           '("vector-ref" "ref-vector" "list-ref") (popup-shown s))
    (press-keys s "C-g")
    (check "C-g closes popup after regexp" #f (editor-completion s))

    ;; =literal in popup
    (type-text s " (foo")
    (press-keys s "C-M-i")
    (press-keys s "M-SPC")
    (type-text s "=foo.bar")
    (check "=literal in popup filters to foo.bar" '("foo.bar") (popup-shown s))
    (press-keys s "C-g")
    (check "C-g closes popup" #f (editor-completion s))))
