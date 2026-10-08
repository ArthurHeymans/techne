;;; SRFI 115: regular expressions written as s-expressions (SREs).
;;;
;;; An SRE translates to the pattern syntax of Rust's regex crate, which
;;; the native `regexp` objects compile and search (crates/techne-vm/src/
;;; regexp.rs), so matching takes time linear in the text and memory
;;; bounded by the pattern. That engine has no look-around and no
;;; backreferences, and no grapheme boundaries (bog, eog): those SREs are
;;; errors. Matches keep string cursors, so folding over the matches of a
;;; long string is linear whatever its characters; indexes are worked out
;;; when asked for.

(define-library (srfi 115)
  (export regexp regexp? valid-sre? rx regexp->sre char-set->sre
          regexp-matches regexp-matches? regexp-search
          regexp-replace regexp-replace-all regexp-match->list
          regexp-fold regexp-extract regexp-split regexp-partition
          regexp-match? regexp-match-count regexp-match-submatch
          regexp-match-submatch-start regexp-match-submatch-end)
  (import (techne))
  (begin

    ;; ----- SREs to patterns -----

    (define (%hex c)
      (string-append "\\x{" (number->string (char->integer c) 16) "}"))

    ;; A literal string: letters and digits as they are, the rest escaped.
    ;; Ignoring case, letters match their other cases: all of them, or in an
    ;; ASCII context only ASCII letters, ASCII ones.
    (define (%literal s nocase ascii)
      (define (plain c) (if (or (char-alphabetic? c) (char-numeric? c)) (string c) (%hex c)))
      (cond ((not nocase) (apply string-append (map plain (string->list s))))
            (ascii (apply string-append
                          (map (lambda (c) (if (and (char<=? c #\x7f) (char-alphabetic? c))
                                               (string #\[ (char-upcase c) (char-downcase c) #\])
                                               (plain c)))
                               (string->list s))))
            (else (string-append "(?i:" (apply string-append (map plain (string->list s))) ")"))))

    (define %never "[^\\x{0}-\\x{10FFFF}]")

    ;; The bracket classes of the named character sets, Unicode and ASCII.
    (define %named-sets
      '((any "[\\x{0}-\\x{10FFFF}]" "[\\x{0}-\\x{7f}]")
        (nonl "[^\\n\\r]" "[\\x{0}-\\x{9}\\x{b}\\x{c}\\x{e}-\\x{7f}]")
        (ascii "[\\x{0}-\\x{7f}]" "[\\x{0}-\\x{7f}]")
        (lower-case "[\\p{Lowercase}]" "[a-z]")
        (upper-case "[\\p{Uppercase}]" "[A-Z]")
        (title-case "[\\p{Lt}]" "[^\\x{0}-\\x{10FFFF}]")
        (alphabetic "[\\p{Alphabetic}]" "[a-zA-Z]")
        (numeric "[\\p{Nd}]" "[0-9]")
        (alphanumeric "[\\p{Alphabetic}\\p{Nd}]" "[a-zA-Z0-9]")
        (punctuation "[\\p{P}]" "[\\x{21}-\\x{23}\\x{25}-\\x{2a}\\x{2c}-\\x{2f}\\x{3a}\\x{3b}\\x{3f}\\x{40}\\x{5b}-\\x{5d}\\x{5f}\\x{7b}\\x{7d}]")
        (symbol "[\\p{S}]" "[\\x{24}\\x{2b}\\x{3c}-\\x{3e}\\x{5e}\\x{60}\\x{7c}\\x{7e}]")
        (graphic "[\\p{Alphabetic}\\p{Nd}\\p{P}\\p{S}]" "[\\x{21}-\\x{7e}]")
        (whitespace "[\\p{White_Space}]" "[\\x{9}-\\x{d}\\x{20}]")
        (printing "[\\p{Alphabetic}\\p{Nd}\\p{P}\\p{S}\\p{White_Space}]" "[\\x{9}-\\x{d}\\x{20}-\\x{7e}]")
        (control "[\\p{C}]" "[\\x{0}-\\x{1f}\\x{7f}]")
        (hex-digit "[0-9a-fA-F]" "[0-9a-fA-F]")))

    (define %aliases
      '((lower . lower-case) (upper . upper-case) (title . title-case) (alpha . alphabetic)
        (num . numeric) (digit . numeric) (alphanum . alphanumeric) (alnum . alphanumeric) (punct . punctuation)
        (graph . graphic) (white . whitespace) (space . whitespace) (print . printing)
        (cntrl . control) (xdigit . hex-digit)))

    (define (%named name)
      (let ((name (cond ((assq name %aliases) => cdr) (else name))))
        (and (assq name %named-sets) name)))

    ;; Characters that are letters or digits or _, which words are made of.
    (define (%word-class ascii) (if ascii "[a-zA-Z0-9_]" "[\\p{Alphabetic}\\p{Nd}_]"))

    ;; An extended grapheme cluster, by UAX 29's regular expression.
    (define %grapheme
      (string-append
       "(?:\\r\\n|[\\p{gcb=Control}\\p{gcb=CR}\\p{gcb=LF}]|\\p{gcb=Prepend}*"
       "(?:\\p{gcb=L}*(?:\\p{gcb=V}+|\\p{gcb=LV}\\p{gcb=V}*|\\p{gcb=LVT})\\p{gcb=T}*|\\p{gcb=L}+|\\p{gcb=T}+"
       "|\\p{gcb=Regional_Indicator}\\p{gcb=Regional_Indicator}"
       "|\\p{Extended_Pictographic}(?:\\p{gcb=Extend}*\\p{gcb=ZWJ}\\p{Extended_Pictographic})*"
       "|[^\\p{gcb=Control}\\p{gcb=CR}\\p{gcb=LF}])[\\p{gcb=Extend}\\p{gcb=ZWJ}\\p{gcb=SpacingMark}]*)"))

    (define (%sre-error what sre) (error (string-append "regexp: " what) sre))

    (define (%cset-form? sre)
      (or (char? sre)
          (and (string? sre) (= (string-length sre) 1))
          (and (symbol? sre) (%named sre))
          (and (pair? sre)
               (or (and (string? (car sre)) (null? (cdr sre)))
                   (memq (car sre) '(char-set / char-range ~ complement - difference & and))
                   (and (memq (car sre) '(or |\||)) (every %cset-form? (cdr sre)))
                   (and (memq (car sre) '(w/case w/nocase w/ascii w/unicode))
                        (= (length sre) 2) (%cset-form? (cadr sre)))))))

    ;; The bracket class of a character set SRE. Ignoring case, each
    ;; character, string, range and named set is taken with the other cases
    ;; of its characters before sets are combined.
    (define (%cset sre nocase ascii)
      (define (fold class) (if nocase (%case-fold-class class ascii) class))
      (define (chars cs)
        (if (null? cs) %never (fold (string-append "[" (apply string-append (map %hex cs)) "]"))))
      (define (union sres)
        (if (null? sres) %never (string-append "[" (apply string-append (map (lambda (s) (%cset s nocase ascii)) sres)) "]")))
      (define (any) (if ascii "[\\x{0}-\\x{7f}]" "[\\x{0}-\\x{10FFFF}]"))
      (cond
       ((char? sre) (chars (list sre)))
       ((string? sre) (chars (string->list sre)))
       ((symbol? sre)
        (let ((name (%named sre)))
          (unless name (%sre-error "unknown character set" sre))
          (fold (let ((classes (cdr (assq name %named-sets)))) (if ascii (cadr classes) (car classes))))))
       ((and (pair? sre) (string? (car sre)) (null? (cdr sre))) (chars (string->list (car sre))))
       ((not (pair? sre)) (%sre-error "not a character set" sre))
       (else
        (case (car sre)
          ((char-set) (chars (string->list (cadr sre))))
          ((/ char-range)
           (let loop ((cs (append-map (lambda (x) (if (string? x) (string->list x) (list x))) (cdr sre))) (acc ""))
             (cond ((null? cs) (if (string-null? acc) %never (fold (string-append "[" acc "]"))))
                   ((null? (cdr cs)) (%sre-error "odd number of range ends" sre))
                   (else (loop (cddr cs) (string-append acc (%hex (car cs)) "-" (%hex (cadr cs))))))))
          ((or |\||) (union (cdr sre)))
          ((& and)
           (if (null? (cdr sre))
               (any)
               (string-append "[" (string-join (map (lambda (s) (%cset s nocase ascii)) (cdr sre)) "&&") "]")))
          ((- difference) (string-append "[" (%cset (cadr sre) nocase ascii) "--" (union (cddr sre)) "]"))
          ((~ complement) (string-append "[" (any) "--" (union (cdr sre)) "]"))
          ((w/case) (%cset (cadr sre) #f ascii))
          ((w/nocase) (%cset (cadr sre) #t ascii))
          ((w/ascii) (%cset (cadr sre) nocase #t))
          ((w/unicode) (%cset (cadr sre) nocase #f))
          (else (%sre-error "not a character set" sre))))))

    ;; The pattern of SRE, and the names of its capturing groups in order (a
    ;; symbol, or #f for a numbered one).
    (define (%translate sre)
      (define names '())
      (define (seq sres nocase ascii capture)
        (apply string-append (map (lambda (s) (tr s nocase ascii capture)) sres)))
      (define (group sres nocase ascii capture)
        (string-append "(?:" (seq sres nocase ascii capture) ")"))
      (define (count n)
        (if (and (exact-integer? n) (>= n 0)) (number->string n) (%sre-error "bad repetition count" n)))
      (define (tr sre nocase ascii capture)
        (cond
         ((string? sre) (%literal sre nocase ascii))
         ((%cset-form? sre) (%cset sre nocase ascii))
         ((symbol? sre)
          (case sre
            ((bos) "\\A")
            ((eos) "\\z")
            ((bol) "(?mR:^)")
            ((eol) "(?mR:$)")
            ((bow) (if ascii "(?-u:\\b{start})" "\\b{start}"))
            ((eow) (if ascii "(?-u:\\b{end})" "\\b{end}"))
            ((nwb) (if ascii "(?-u:\\B)" "\\B"))
            ((word) (tr '(word+ any) nocase ascii capture))
            ((grapheme) (if ascii "[\\x{0}-\\x{7f}]" %grapheme))
            ((bog eog) (%sre-error "grapheme boundaries are not supported" sre))
            (else (%sre-error "unknown SRE" sre))))
         ((not (pair? sre)) (%sre-error "not an SRE" sre))
         (else
          (let ((args (cdr sre)))
            (case (car sre)
              ((: seq) (seq args nocase ascii capture))
              ((or |\||)
               (if (null? args)
                   %never
                   (string-append "(?:" (string-join (map (lambda (s) (tr s nocase ascii capture)) args) "|") ")")))
              ((* zero-or-more) (string-append (group args nocase ascii capture) "*"))
              ((+ one-or-more) (string-append (group args nocase ascii capture) "+"))
              ((? optional) (string-append (group args nocase ascii capture) "?"))
              ((*? non-greedy-zero-or-more) (string-append (group args nocase ascii capture) "*?"))
              ((?? non-greedy-optional) (string-append (group args nocase ascii capture) "??"))
              ((= exactly) (string-append (group (cdr args) nocase ascii capture) "{" (count (car args)) "}"))
              ((>= at-least) (string-append (group (cdr args) nocase ascii capture) "{" (count (car args)) ",}"))
              ((** repeated)
               (string-append (group (cddr args) nocase ascii capture) "{" (count (car args)) "," (count (cadr args)) "}"))
              ((**? non-greedy-repeated)
               (string-append (group (cddr args) nocase ascii capture) "{" (count (car args)) "," (count (cadr args)) "}?"))
              (($ submatch) (submatch #f args nocase ascii capture))
              ((-> submatch-named)
               (unless (and (pair? args) (symbol? (car args))) (%sre-error "a named submatch needs a symbol" sre))
               (submatch (car args) (cdr args) nocase ascii capture))
              ((w/case) (group args #f ascii capture))
              ((w/nocase) (group args #t ascii capture))
              ((w/ascii) (group args nocase #t capture))
              ((w/unicode) (group args nocase #f capture))
              ((w/nocapture) (group args nocase ascii #f))
              ((word) (string-append (tr 'bow nocase ascii capture) (group args nocase ascii capture) (tr 'eow nocase ascii capture)))
              ((word+)
               (string-append (tr 'bow nocase ascii capture)
                              "[" (%word-class ascii) "&&" (%cset (cons 'or args) nocase ascii) "]+"
                              (tr 'eow nocase ascii capture)))
              ((look-ahead look-behind neg-look-ahead neg-look-behind)
               (%sre-error "look-around is not supported" sre))
              ((backref) (%sre-error "backreferences are not supported" sre))
              (else (%sre-error "unknown SRE" sre)))))))
      (define (submatch name args nocase ascii capture)
        (if capture
            (begin (set! names (cons name names))
                   (string-append "(" (seq args nocase ascii capture) ")"))
            (group args nocase ascii capture)))
      (let ((pattern (tr sre #f #f #t)))
        (values pattern (reverse names))))

    ;; ----- regexps -----

    (define (regexp re)
      "Return the compiled regular expression of the SRE RE.
A compiled one is returned as it is."
      (if (regexp? re)
          re
          (call-with-values (lambda () (%translate re))
            (lambda (pattern names)
              (%make-regexp pattern (call-with-output-string (lambda (p) (write re p))) names)))))

    (define-syntax rx
      (syntax-rules ()
        ((_ sre ...) (regexp `(: sre ...)))))

    (define (regexp->sre re)
      "Return an SRE that RE was compiled from."
      (read (open-input-string (%regexp-sre (regexp re)))))

    (define (char-set->sre cs)
      "Return CS as an SRE: a string of its characters.
CS is a string or a list of characters, as SRFI 14 character sets are not
provided."
      (list (if (string? cs) cs (list->string cs))))

    (define (valid-sre? obj)
      "Return #t if OBJ is an SRE that `regexp` compiles."
      (guard (e (#t #f)) (regexp obj) #t))

    ;; ----- matches -----

    (define-record-type regexp-match
      (%make-match string spans names)
      regexp-match?
      (string %match-string)
      ;; Cursors: start and end of the match, then of each group (#f for
      ;; one that did not take part).
      (spans %match-spans)
      ;; The name of each group from 1, or #f.
      (names %match-names))

    ;; Cursors START and END of STR from optional indexes or cursors.
    (define (%start str start) (if start (string-index->cursor str start) (string-cursor-start str)))
    (define (%end str end) (if end (string-index->cursor str end) (string-cursor-end str)))

    ;; The match of RE in STR from FROM to END as if STR began at START.
    (define (%run re str start end from whole)
      (let* ((re (regexp re)) (spans (%regexp-search re str start end from whole)))
        (and spans (%make-match str spans (%regexp-names re)))))

    (define (regexp-matches re str [start #f] [end #f])
      "Return the match of RE with the whole of STR from START to END, or #f."
      (let ((start (%start str start)))
        (%run re str start (%end str end) start #t)))

    (define (regexp-matches? re str [start #f] [end #f])
      "Return #t if RE matches the whole of STR from START to END."
      (and (regexp-matches re str start end) #t))

    (define (regexp-search re str [start #f] [end #f])
      "Return the first match of RE in STR from START to END, or #f."
      (let ((start (%start str start)))
        (%run re str start (%end str end) start #f)))

    (define (regexp-match-count m)
      "Return how many submatches M has, besides the whole match."
      (- (quotient (vector-length (%match-spans m)) 2) 1))

    ;; The group of FIELD, a number or the name of the first group of that
    ;; name that matched.
    (define (%field m field)
      (cond ((and (exact-integer? field) (<= 0 field (regexp-match-count m))) field)
            ((symbol? field)
             (let loop ((names (%match-names m)) (i 1) (first #f))
               (cond ((null? names) (or first (error "regexp-match: no submatch named" field)))
                     ((not (eq? (car names) field)) (loop (cdr names) (+ i 1) first))
                     ((vector-ref (%match-spans m) (* 2 i)) i)
                     (else (loop (cdr names) (+ i 1) (or first i))))))
            (else (error "regexp-match: no submatch" field))))

    (define (%cursor m field side)
      (vector-ref (%match-spans m) (+ (* 2 (%field m field)) side)))

    (define (regexp-match-submatch m field)
      "Return the text of submatch FIELD of M, or #f if it did not match.
FIELD is 0 for the whole match, a submatch's number from 1, or its name."
      (let ((a (%cursor m field 0)))
        (and a (substring/cursors (%match-string m) a (%cursor m field 1)))))

    (define (regexp-match-submatch-start m field)
      "Return the index where submatch FIELD of M starts, or #f."
      (let ((a (%cursor m field 0))) (and a (string-cursor->index (%match-string m) a))))

    (define (regexp-match-submatch-end m field)
      "Return the index where submatch FIELD of M ends, or #f."
      (let ((b (%cursor m field 1))) (and b (string-cursor->index (%match-string m) b))))

    (define (regexp-match->list m)
      "Return the texts of the whole match M and of its submatches."
      (map (lambda (i) (regexp-match-submatch m i)) (iota (+ 1 (regexp-match-count m)))))

    ;; ----- folding over matches -----

    ;; Fold over the matches of RE in STR from START to END (cursors).
    ;; (KONS from m acc) gets the cursor where the text since the last
    ;; match starts; (FINISH from acc) the cursor after the last match. An
    ;; empty match is followed by a search a character on, and ends the
    ;; matches at END.
    (define (%fold re kons knil str finish start end)
      (let ((re (regexp re)))
        (let loop ((i start) (from start) (acc knil))
          (let ((m (and (string-cursor<=? i end) (%run re str start end i #f))))
            (if m
                (let* ((a (%cursor m 0 0)) (b (%cursor m 0 1)) (acc (kons from m acc)))
                  (cond ((string-cursor<? a b) (if (string-cursor<? b end) (loop b b acc) (finish b acc)))
                        ((string-cursor<? b end) (loop (string-cursor-next str b) b acc))
                        (else (finish b acc))))
                (finish from acc))))))

    (define (regexp-fold re kons knil str [finish (lambda (i m str acc) acc)] [start #f] [end #f])
      "Fold KONS over the matches of RE in STR from START to END.
KONS takes the index where the text since the previous match starts, the
match, STR and the result so far, starting with KNIL. FINISH is called
like it after the last match, with #f for the match."
      (let ((start (%start str start)) (count 0) (counted (%start str start)))
        ;; Indexes from cursors, counting only the characters between.
        (define (index c)
          (set! count (+ count (string-cursor-diff str counted c)))
          (set! counted c)
          count)
        (set! count (string-cursor->index str start))
        (%fold re (lambda (from m acc) (kons (index from) m str acc)) knil str
               (lambda (from acc) (finish (index from) #f str acc))
               start (%end str end))))

    (define (regexp-extract re str [start #f] [end #f])
      "Return the texts of the non-empty matches of RE in STR from START to END."
      (reverse (%fold re (lambda (from m acc) (let ((s (regexp-match-submatch m 0))) (if (string-null? s) acc (cons s acc))))
                      '() str (lambda (from acc) acc) (%start str start) (%end str end))))

    (define (regexp-split re str [start #f] [end #f])
      "Return the texts of STR from START to END between non-empty matches of RE."
      (let ((start (%start str start)) (end (%end str end)))
        (%fold re
               (lambda (from m acc)
                 (let ((a (%cursor m 0 0)) (b (%cursor m 0 1)))
                   (if (string-cursor=? a b) acc (cons b (cons (substring/cursors str (car acc) a) (cdr acc))))))
               (list start) str
               (lambda (from acc) (reverse (cons (substring/cursors str (car acc) end) (cdr acc))))
               start end)))

    (define (regexp-partition re str [start #f] [end #f])
      "Return the texts of STR from START to END between and of the matches of RE.
Texts between matches come first and then every other one, empty if a
match follows another; a match at the end ends the list."
      (let ((start (%start str start)) (end (%end str end)))
        (reverse
         (%fold re
                (lambda (from m acc)
                  (let ((a (%cursor m 0 0)) (b (%cursor m 0 1)))
                    (if (string-cursor=? a b)
                        acc
                        (cons b (cons (regexp-match-submatch m 0) (cons (substring/cursors str (car acc) a) (cdr acc)))))))
                (list start) str
                ;; (car acc) is the end of the last non-empty match.
                (lambda (from acc)
                  (if (or (string-cursor<? (car acc) end) (null? (cdr acc)))
                      (cons (substring/cursors str (car acc) end) (cdr acc))
                      (cdr acc)))
                start end))))

    ;; The texts SUBST stands for in match M of STR from START to END.
    (define (%substitute m str subst start end)
      (append-map
       (lambda (s)
         (cond ((string? s) (list s))
               ((exact-integer? s) (list (or (regexp-match-submatch m s) "")))
               ((eq? s 'pre) (list (substring/cursors str start (%cursor m 0 0))))
               ((eq? s 'post) (list (substring/cursors str (%cursor m 0 1) end)))
               ((symbol? s) (list (or (regexp-match-submatch m s) "")))
               ((procedure? s) (list (s m)))
               (else (error "regexp-replace: bad substitution" s))))
       (if (list? subst) subst (list subst))))

    (define (regexp-replace re str subst [start #f] [end #f] [count 0])
      "Return STR from START to END with match COUNT of RE replaced by SUBST.
COUNT counts from 0. SUBST is a string, a submatch's number or name, pre
or post (the text before or after the match), or a list of those."
      (let* ((start (%start str start)) (end (%end str end))
             (m (call/cc
                 (lambda (found)
                   (%fold re (lambda (from m n) (if (zero? n) (found m) (- n 1))) count str
                          (lambda (from n) #f) start end)))))
        (if m
            (apply string-append
                   (append (list (substring/cursors str start (%cursor m 0 0)))
                           (%substitute m str subst start end)
                           (list (substring/cursors str (%cursor m 0 1) end))))
            (substring/cursors str start end))))

    (define (regexp-replace-all re str subst [start #f] [end #f])
      "Return STR from START to END with every match of RE replaced by SUBST.
SUBST is as `regexp-replace` takes it."
      (let ((start (%start str start)) (end (%end str end)))
        (%fold re
               (lambda (from m acc)
                 (append (reverse (%substitute m str subst start end))
                         (cons (substring/cursors str from (%cursor m 0 0)) acc)))
               '() str
               (lambda (from acc) (apply string-append (reverse (cons (substring/cursors str from end) acc))))
               start end)))))
