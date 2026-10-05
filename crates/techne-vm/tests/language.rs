//! Language features end to end. Each program's output must match exactly,
//! normally and under GC stress (`1`: collect on every allocation, `full`:
//! full collections).
use std::process::Command;

fn run_file(path: &std::path::Path, stress: Option<&str>) -> (String, String, bool) {
    run_file_env(path, stress, &[])
}

fn run_file_env(path: &std::path::Path, stress: Option<&str>, env: &[(&str, &str)]) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
    cmd.arg(path).envs(env.iter().copied());
    if let Some(mode) = stress {
        cmd.env("TECHNE_GC_STRESS", mode);
    }
    let out = cmd.output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

fn run(name: &str, src: &str, stress: Option<&str>) -> (String, String, bool) {
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{name}.scm"));
    std::fs::write(&file, src).unwrap();
    run_file(&file, stress)
}

fn check(name: &str, src: &str, expected: &str) {
    for stress in [None, Some("1"), Some("full")] {
        let (out, err, ok) = run(name, src, stress);
        assert!(ok && out == expected, "{name} (stress={stress:?})\nexpected {expected:?}\ngot {out:?}\nstderr: {err}");
    }
}

#[test]
fn macros() {
    check("swap", "(define-syntax swap! (syntax-rules () ((_ a b) (let ((tmp a)) (set! a b) (set! b tmp)))))
        (define tmp 1) (define y 2) (swap! tmp y) (displayln (list tmp y))", "(2 1)\n");
    // A user binding named like a template's free identifier cannot capture it.
    check("free-ids", "(define-syntax my-or2 (syntax-rules () ((_ a b) (let ((t a)) (if t t b)))))
        (define (f) (let ((if list) (t 5)) (my-or2 #f t))) (displayln (f))", "5\n");
    check("ellipsis", "(define-syntax my-list (syntax-rules () ((_ (a b) ...) (list (cons a b) ...))))
        (displayln (my-list (1 2) (3 4)))", "((1 . 2) (3 . 4))\n");
    check("nested-ellipsis", "(define-syntax flat (syntax-rules () ((_ (x ...) ...) '(x ... ...))))
        (displayln (flat (1 2) (3) (4 5)))", "(1 2 3 4 5)\n");
    check("tail-pattern", "(define-syntax lastof (syntax-rules () ((_ x ... y) 'y)))
        (displayln (lastof 1 2 3))", "3\n");
    check("literals", "(define-syntax kw (syntax-rules (=>) ((_ a => b) (+ a b)) ((_ a b) (* a b))))
        (displayln (list (kw 2 => 3) (kw 2 3)))", "(5 6)\n");
    check("recursive", "(define-syntax my-and (syntax-rules () ((_) #t) ((_ e) e) ((_ e r ...) (if e (my-and r ...) #f))))
        (displayln (list (my-and 1 2 3) (my-and 1 #f 3)))", "(3 #f)\n");
    check("local-macro", "(define (f x) (define-syntax twice (syntax-rules () ((_ e) (begin e e)))) (define n 0) (twice (set! n (+ n x))) n)
        (displayln (f 5))", "10\n");
    check("let-syntax", "(displayln (let-syntax ((inc (syntax-rules () ((_ x) (+ x 1))))) (inc 41)))", "42\n");
    check("macro-defines", "(define-syntax def2 (syntax-rules () ((_ a b v) (begin (define a v) (define b v)))))
        (def2 p q 7) (displayln (+ p q))", "14\n");
    check("do", "(displayln (do ((i 0 (+ i 1)) (acc '() (cons i acc))) ((= i 4) acc)))", "(3 2 1 0)\n");
    check("case-lambda", "(define f (case-lambda ((x) (list 'one x)) ((x y) (list 'two x y)) ((x . r) (list 'many x r))))
        (displayln (list (f 1) (f 1 2) (f 1 2 3)))", "((one 1) (two 1 2) (many 1 (2 3)))\n");
    check("quasiquote", "(define x 5) (define l '(a b)) (displayln `(x ,x ,@l (nested ,(+ x 1)) . end))
        (displayln `#(1 ,x)) (displayln `(1 `(2 ,(3 ,x))))", "(x 5 a b (nested 6) . end)\n#(1 5)\n(1 (quasiquote (2 (unquote (3 5)))))\n");
}

#[test]
fn records_and_values() {
    check("records", "(define-record-type <point> (make-point x y) point? (x point-x set-point-x!) (y point-y))
        (define p (make-point 1 2)) (set-point-x! p 10)
        (displayln (list (point-x p) (point-y p) (point? p) (point? 5))) (displayln p)", "(10 2 #t #f)\n#<point 10 2>\n");
    check("record-type-error", "(define-record-type a (make-a x) a? (x a-x)) (define-record-type b (make-b x) b? (x b-x))
        (displayln (guard (e (#t (error-object-message e))) (a-x (make-b 1))))", "record accessor: expected a, got #<b 1>\n");
    check("values", "(call-with-values (lambda () (values 1 2 3)) (lambda (a b c) (displayln (+ a b c))))
        (let-values (((q r) (values 7 2)) ((s) (values 1))) (displayln (list q r s)))
        (define-values (u v) (values 'x 'y)) (displayln (list u v))
        (receive (a . rest) (values 1 2 3) (displayln (list a rest)))", "6\n(7 2 1)\n(x y)\n(1 (2 3))\n");
    check("apply", "(displayln (list (apply + 1 2 '(3 4)) (apply max '(3 9 2)) (apply (lambda args args) '(a b))))
        (define (f . xs) (if (null? xs) 0 (+ (car xs) (apply f (cdr xs))))) (displayln (f 1 2 3))", "(10 9 (a b))\n6\n");
}

#[test]
fn conditions() {
    check("guard-error", "(displayln (guard (e ((error-object? e) (list (error-object-message e) (error-object-irritants e))))
        (error \"bad thing\" 1 'two)))", "(bad thing (1 two))\n");
    check("guard-raise", "(displayln (guard (e ((symbol? e) (list 'sym e)) ((string? e) 'str)) (raise 'oops)))", "(sym oops)\n");
    check("guard-reraise", "(displayln (guard (outer (#t (list 'outer outer))) (guard (inner ((string? inner) 'no)) (raise 42))))", "(outer 42)\n");
    check("runtime-errors", "(define (f x) (car x))
        (displayln (guard (e (#t (error-object-message e))) (f 5)))
        (displayln (guard (e (#t 'caught)) (vector-ref (vector 1) 9)))
        (displayln (guard (e (#t (error-object-message e))) (undefined-thing)))", "car: expected pair, got 5\ncaught\nunbound variable: undefined-thing\n");
    check("guard-unwinds-frames", "(define (deep n) (if (= n 0) (raise 'bottom) (+ 1 (deep (- n 1)))))
        (define (deep2 n) n) (displayln (guard (e (#t e)) (deep 1000))) (displayln (deep2 3))", "bottom\n3\n");
    check("guard-in-loop", "(define (safe-div a b) (guard (e (#t 'div0)) (if (= b 0) (raise 'z) (quotient a b))))
        (displayln (map (lambda (b) (safe-div 10 b)) '(1 0 5)))", "(10 div0 2)\n");
    check("with-exception-handler", "(displayln (with-exception-handler (lambda (e) (* e 2)) (lambda () (+ 1 (raise-continuable 20)))))", "41\n");
    check("handler-in-native-call", "(displayln (guard (e (#t (list 'caught e))) (map (lambda (x) (if (= x 2) (raise x) x)) '(1 2 3))))", "(caught 2)\n");
    check("dynamic-wind", "(define log '())
        (guard (e (#t #f)) (dynamic-wind (lambda () (set! log (cons 'in log))) (lambda () (raise 'x)) (lambda () (set! log (cons 'out log)))))
        (displayln (reverse log))", "(in out)\n");
    check("call-cc", "(displayln (call/cc (lambda (k) (for-each (lambda (x) (when (> x 2) (k x))) '(1 2 3 4)) 'none)))
        (displayln (+ 1 (call/cc (lambda (k) 1))))", "3\n2\n");
    check("parameterize", "(define p (make-parameter 1)) (define (show) (p))
        (displayln (list (show) (parameterize ((p 2)) (show)) (show)))", "(1 2 1)\n");
    check("promises", "(define n 0) (define pr (delay (begin (set! n (+ n 1)) n))) (force pr) (displayln (list (force pr) n))", "(1 1)\n");
    check("assert", "(displayln (guard (e (#t (error-object-irritants e))) (assert (= 1 2))))", "((= 1 2))\n");
}

#[test]
fn library() {
    check("strings", "(displayln (list (string-split \"a,b,,c\" \",\") (string-join '(\"x\" \"y\") \"-\") (string-contains \"hello\" \"ll\")
        (string-upcase \"abc\") (string-trim \"  x \") (string-index \"abc\" #\\c) (string-replace \"aXbX\" \"X\" \"-\")))",
        "((a b  c) x-y 2 ABC x 2 a-b-)\n");
    check("ports", "(define p (open-output-string)) (write 'sym p) (display \" \" p) (write \"s\" p) (displayln (get-output-string p))
        (displayln (with-output-to-string (lambda () (display 1) (display 2))))
        (define in (open-input-string \"line1\\nline2\\n(a b) 42\"))
        (displayln (list (read-line in) (read-line in) (read in) (read in) (eof-object? (read in))))",
        "sym \"s\"\n12\n(line1 line2 (a b) 42 #t)\n");
    check("lists", "(displayln (list (sort '(3 1 2) <) (sort < '(5 4)) (filter odd? (iota 6)) (fold + 0 '(1 2 3)) (reduce max 0 '(3 7 2))
        (delete-duplicates '(1 2 1 3 2)) (list-index even? '(1 3 4)) (take '(1 2 3) 2) (drop '(1 2 3) 2) (last '(1 2 3))
        (any even? '(1 3)) (every odd? '(1 3)) (append-map (lambda (x) (list x x)) '(1 2)) (assq 'b '((a 1) (b 2)))))",
        "((1 2 3) (4 5) (1 3 5) 6 7 (1 2 3) 2 (1 2) (3) 3 #f #t (1 1 2 2) (b 2))\n");
    check("map-multi", "(displayln (map + '(1 2 3) '(10 20 30))) (for-each (lambda (a b) (display (* a b))) '(1 2) '(3 4)) (newline)", "(11 22 33)\n38\n");
    check("hash-tables", "(define h (make-hash-table))
        (for-each (lambda (i) (hash-table-set! h i (* i i))) (iota 100))
        (for-each (lambda (i) (hash-table-delete! h i)) (iota 90))
        (for-each (lambda (round) (for-each (lambda (i) (hash-table-set! h (+ 1000 i) i) (hash-table-delete! h (+ 1000 i))) (iota 50))) (iota 20))
        (hash-table-update!/default h 'n (lambda (x) (+ x 1)) 0) (hash-table-update!/default h 'n (lambda (x) (+ x 1)) 0)
        (displayln (list (hash-table-count h) (hash-table-ref h 95 #f) (hash-table-ref h 5 'gone) (hash-table-ref h 'n 0)
          (length (hash-table-keys h)) (sort (map cdr (filter (lambda (kv) (number? (car kv))) (hash-table->alist h))) <)))",
        "(11 9025 gone 2 11 (8100 8281 8464 8649 8836 9025 9216 9409 9604 9801))\n");
    check("vectors", "(define v (vector 1 2 3)) (displayln (list (vector-map (lambda (x) (* x 10)) v) (vector-copy v 1) (vector-append v #(4))))",
        "(#(10 20 30) #(2 3) #(1 2 3 4))\n");
    check("eval", "(displayln (eval '(+ 1 2))) (eval '(define evald 9)) (displayln evald)", "3\n9\n");
    check("bodies", "(define (f) (define a 1) (displayln 'mid) (define b (+ a 1)) (list a b)) (displayln (f))", "mid\n(1 2)\n");
    check("curried-define", "(define ((adder n) x) (+ n x)) (displayln ((adder 3) 4))", "7\n");
}

#[test]
fn modules() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/modules/main.scm");
    for stress in [None, Some("1")] {
        let (out, err, ok) = run_file(&path, stress);
        assert!(ok && out == "25\n49\nuser-helper\n", "stress={stress:?} got {out:?}\n{err}");
    }
}

#[test]
fn error_locations() {
    let (out, err, ok) = run("loc", "(define (inner x)\n  (car x))\n(define (outer y)\n  (+ 1 (inner y)))\n(outer 5)\n", None);
    assert!(!ok && out.is_empty());
    assert!(err.contains("car: expected pair, got 5"), "{err}");
    assert!(err.contains("inner (") && err.contains("loc.scm:2:3"), "{err}");
    assert!(err.contains("outer (") && err.contains("loc.scm:4:8"), "{err}");
    let (_, err, ok) = run("unclosed", "(define (f x)\n  (let ((y 1)\n    (+ x y))\n", None);
    assert!(!ok && err.contains("unclosed.scm:3:12: Unexpected EOF"), "{err}");
}

#[test]
fn jit() {
    // Compile every loop at its first back-edge; the output must match the
    // interpreter's.
    let src = r#"(define (sum-to n) (let loop ((i 0) (acc 0)) (if (= i n) acc (loop (+ i 1) (+ acc i)))))
        (define (grow n) (let loop ((i 0) (x 1)) (if (= i n) x (loop (+ i 1) (* x 3)))))
        (define (mix n) (let loop ((i 0) (x 0)) (if (= i n) x (loop (+ i 1) (+ x 0.5)))))
        (define (fsum n) (let loop ((i 0) (x 0.0)) (if (< i n) (loop (+ i 1) (+ x (* 1.5 2.0))) x)))
        (define (walk v) (let loop ((i 0)) (if (< i 10) (begin (vector-ref v i) (loop (+ i 1))) 'done)))
        (define (cars l) (let loop ((l l) (n 0)) (if (= n 3) n (begin (car l) (loop (cdr l) (+ n 1))))))
        (define (unbound n) (let loop ((i 0)) (if (< i n) (loop (+ i no-such-global)) i)))
        (define (build n) (let loop ((i 0) (l '())) (if (= i n) (list (length l) (car l)) (loop (+ i 1) (cons i l)))))
        (define (boxed n) (let ((c 0)) (let loop ((i 0)) (when (< i n) (set! c (+ c i)) (loop (+ i 1)))) (list c ((lambda () c)))))
        (define (fill n) (let ((v (make-vector n #f))) (let loop ((i 0)) (when (< i n) (vector-set! v i (list i)) (loop (+ i 1)))) (vector-ref v (- n 1))))
        (define g 0)
        (define (bump n) (let loop ((i 0)) (when (< i n) (set! g (+ g 1)) (loop (+ i 1)))) g)
        (define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
        (define (deep n) (if (= n 0) 0 (+ 1 (deep (- n 1)))))
        ;; The comparator's recursion reallocates the register stack under JIT code.
        (define (sorts k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (car (sort (list 3 1 2) (lambda (a b) (deep (* i 40000)) (< a b)))))) acc)))
        (displayln (list (sum-to 100000) (grow 35) (mix 10) (fsum 4)))
        (displayln (report (lambda () (walk (make-vector 5 0)))))
        (displayln (report (lambda () (cars '(1 2)))))
        (displayln (report (lambda () (unbound 3))))
        (displayln (list (build 5000) (boxed 100) (fill 3000) (bump 5000) (sorts 3)))
        (displayln (let loop ((i 0) (acc '())) (if (< i 3) (loop (+ i 1) (cons (guard (e (#t i)) (car i)) acc)) acc)))
        ;; Native calls: deeper than the native depth limit, errors deep inside,
        ;; tail-call trampolines, self tail calls to another closure of the same
        ;; code, preemption and waits inside recursion.
        (define (len l) (if (null? l) 0 (+ 1 (len (cdr l)))))
        (define (down n) (if (= n 0) (car '()) (+ 1 (down (- n 1)))))
        (define (ev? n) (if (= n 0) #t (od? (- n 1))))
        (define (od? n) (if (= n 0) #f (ev? (- n 1))))
        (define (mk k) (define (f n) (if (= n 0) k ((mk (+ k 1)) (- n 1)))) f)
        (define log '())
        (define (rec name n) (when (= 0 (modulo n 4000)) (set! log (cons name log))) (if (= n 0) name (rec name (- n 1))))
        (define (fibr n) (if (< n 2) n (+ (fibr (- n 1)) (fibr (- n 2)))))
        (define ch (make-channel))
        (define (recv-sum k) (if (= k 0) 0 (+ (channel-recv ch) (recv-sum (- k 1)))))
        (displayln (list (len (iota 5000)) (ev? 100001) ((mk 0) 10) (fibr 15)))
        (displayln (guard (e (#t (condition/report-string e))) (down 3000)))
        (define a (spawn (lambda () (rec 'a 20000))))
        (define b (spawn (lambda () (rec 'b 20000))))
        (displayln (list (task-join a) (task-join b) (reverse log)))
        (define consumer (spawn (lambda () (recv-sum 5))))
        (spawn (lambda () (for-each (lambda (i) (channel-send ch (* i 10))) (iota 5))))
        (displayln (task-join consumer))
        (define (divs f) (map (lambda (p) (f (car p) (cdr p))) '((7 . 2) (-7 . 2) (7 . -2) (-7 . -2) (0 . 5) (-140737488355328 . -1))))
        (displayln (list (divs quotient) (divs remainder) (divs modulo) (report (lambda () (modulo 5 0)))))
        ;; `last` is changed in a compiled loop and read only by the handler.
        (define (h l) (let ((last -1)) (guard (e (#t last)) (let loop ((l l) (i 0)) (set! last i) (loop (cdr l) (+ i 1))))))
        (displayln (list (h '(1 2 3)) (h (iota 2000))))"#;
    let expected = "(4999950000 50031545098999707 5.0 12.0)
vector-ref: bad index 5 for #(0 0 0 0 0)
car: expected pair, got ()
unbound variable: no-such-global
((5000 4999) (4950 4950) (2999) 5000 3)
(2 1 0)
(5000 #f 10 610)
car: expected pair, got ()
(a b (a a a b b b a a b b a b))
100
((3 -3 -3 3 0 140737488355328) (1 -1 1 -1 0 0) (1 1 -1 -1 0 0) modulo: division by zero)
(3 2000)
";
    for stress in [None, Some("1"), Some("full")] {
        for jit in ["0", "1"] {
            let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("jit.scm");
            std::fs::write(&file, src).unwrap();
            let (out, err, ok) = run_file_env(&file, stress, &[("TECHNE_JIT", jit)]);
            assert!(ok && out == expected, "jit={jit} stress={stress:?}\nexpected {expected:?}\ngot {out:?}\nstderr: {err}");
        }
    }
}

#[test]
fn documentation() {
    let defs = r#"(define (greet name #:greeting [greeting "hi"]) "Greet NAME." (string-append greeting name))
        (define (just-string) "not a docstring")"#;
    check("docstrings", &format!("{defs}\n(displayln (list (documentation greet) (documentation just-string) (just-string) (documentation car)))"),
        "(Greet NAME. #f not a docstring #f)\n");
    let (out, err, ok) = run("help", &format!("{defs}\n(help greet) (help when) (help car) (help no-such-name)"), None);
    assert!(ok, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with(r#"(greet name #:greeting (greeting "hi"))  procedure, "#) && lines[0].ends_with("help.scm:1"), "{out}");
    assert_eq!(&lines[1..], ["", "Greet NAME.", "when: special form", "car: built-in procedure, 1 argument", "no-such-name: unbound"], "{out}");
}

#[test]
fn keywords_and_match() {
    check("keywords", "(define (greet name #:greeting [greeting \"hello\"] #:punct [punct \"!\"]) (string-append greeting \", \" name punct))
        (displayln (list (greet \"a\") (greet \"b\" #:greeting \"hi\") (greet \"c\" #:punct \"?\" #:greeting \"yo\")))
        (define (opt a [b 10] [c (* b 2)]) (list a b c)) (displayln (list (opt 1) (opt 1 2) (opt 1 2 3)))
        (define (req #:k k) k) (displayln (guard (e (#t (error-object-message e))) (req)))
        (displayln (guard (e (#t (error-object-message e))) (greet \"x\" #:nope 1)))
        (define (rest-opt a [b 0] . more) (list a b more)) (displayln (list (rest-opt 1) (rest-opt 1 2 3 4)))
        (displayln (apply greet (list \"d\" #:greeting \"hey\")))",
        "(hello, a! hi, b! yo, c?)\n((1 10 20) (1 2 4) (1 2 3))\nreq: missing required keyword argument #:k\ngreet: unknown keyword argument #:nope\n((1 0 ()) (1 2 (3 4)))\nhey, d!\n");
    check("match", "(define-record-type point (make-point x y) point? (x point-x) (y point-y))
        (define (classify v) (match v [0 'zero] [(? string? s) (list 'string s)] [(point 0 y) (list 'y-axis y)]
          [(point x y) #:when (> x y) (list 'below x y)] [(point x y) (list 'point x y)] [(list 'add a b) (+ a b)]
          [(list 'sum xs ...) (apply + xs)] [(cons 'pair r) (list 'rest r)] [#(a b) (list 'vec a b)]
          [(list (list k v) ...) (list 'alist k v)] [(or 'x 'y) 'xy] [_ 'other]))
        (displayln (map classify (list 0 \"s\" (make-point 0 5) (make-point 3 1) (make-point 1 3) '(add 1 2) '(sum 1 2 3) '(pair 1 2) #(1 2) '((a 1) (b 2)) 'y 'z)))
        (displayln (guard (e (#t (error-object-message e))) (match 5 [0 'no])))
        (match-let (((list a b) '(1 2)) (c 3)) (displayln (+ a b c)))",
        "(zero (string s) (y-axis 5) (below 3 1) (point 1 3) 3 6 (rest (1 2)) (vec 1 2) (alist (a b) (1 2)) xy other)\nmatch: no clause matches\n6\n");
}

#[test]
fn generics_and_restarts() {
    check("generics", "(define-record-type point (make-point x y) point? (x point-x) (y point-y))
        (define-generic (describe x)) (define-method (describe x) 'thing) (define-method (describe (n number)) 'number)
        (define-method (describe (i integer)) 'integer) (define-method (describe (l list)) 'list)
        (define-method (describe (p point)) (list 'point (point-x p))) (define-method (describe (r record)) 'record)
        (define-record-type other (make-other) other?)
        (displayln (map describe (list 1 2.5 '(1) '() (make-point 7 0) (make-other) 'sym)))
        (define-generic (area s)) (displayln (guard (e (#t (error-object-irritants e))) (area 5)))
        (displayln (list (procedure? describe) (applicable? area 1) (type-of (make-point 1 1))))",
        "(integer number list list (point 7) record thing)\n(area 5)\n(#t #f point)\n");
    check("restarts", "(define (parse-entry s) (restart-case (or (string->number s) (error \"bad number:\" s)) (use-value (v) v) (skip () 'skipped)))
        (displayln (handler-bind ((error-object? (lambda (c) (invoke-restart 'use-value -1)))) (map parse-entry '(\"1\" \"x\" \"3\"))))
        (displayln (with-exception-handler (lambda (c) (invoke-restart 'skip)) (lambda () (map parse-entry '(\"7\" \"y\")))))
        (displayln (list (compute-restarts) (restart-case (+ 1 2) (never () 'no))))
        (displayln (guard (e (#t 'outer)) (handler-bind ((symbol? (lambda (c) 'declined))) (raise 'x))))",
        "(1 -1 3)\n(7 skipped)\n(() 3)\nouter\n");
}

#[test]
fn tasks() {
    check("tasks", "(define log '())
        (define (spin name n) (let loop ((i 0)) (when (< i n) (when (= 0 (modulo i 20000)) (set! log (cons name log))) (loop (+ i 1)))) name)
        (define a (spawn (lambda () (spin 'a 100000)))) (define b (spawn (lambda () (spin 'b 100000))))
        (displayln (list (task-join a) (task-join b))) (displayln (reverse log))
        (define ch (make-channel))
        (define consumer (spawn (lambda () (let loop ((acc '())) (let ((v (channel-recv ch))) (if (eq? v 'done) (reverse acc) (loop (cons v acc))))))))
        (spawn (lambda () (for-each (lambda (i) (channel-send ch (* i i)) (yield)) (iota 5)) (channel-send ch 'done)))
        (displayln (task-join consumer))
        (define out (make-channel)) (for-each (lambda (ms) (spawn (lambda () (sleep ms) (channel-send out ms)))) '(30 10 20))
        (displayln (list (channel-recv out) (channel-recv out) (channel-recv out)))
        (displayln (guard (e (#t (error-object-message e))) (task-join (spawn (lambda () (sleep 1) (car 5))))))
        (displayln (task-join (spawn (lambda () (guard (e (#t (list 'caught e))) (sleep 5) (raise 'late))))))
        (define (wait-tail c) (channel-recv c)) (define c2 (make-channel)) (define t (spawn (lambda () (wait-tail c2))))
        (channel-send c2 'tail-ok) (displayln (task-join t))
        (displayln (apply + (map task-join (map (lambda (i) (spawn (lambda () (sleep (modulo i 7)) (* i 2)))) (iota 200)))))
        (define wind-log '())
        (displayln (task-join (spawn (lambda () (dynamic-wind (lambda () (set! wind-log (cons 'in wind-log))) (lambda () (sleep 1) 'slept-in-wind) (lambda () (set! wind-log (cons 'out wind-log))))))))
        (displayln (reverse wind-log))
        (define p (make-parameter 'top))
        (define seen (make-channel))
        (define t1 (spawn (lambda () (parameterize ((p 'one)) (sleep 10) (channel-send seen (list 'one (p)))))))
        (define t2 (spawn (lambda () (sleep 5) (channel-send seen (list 'two (p))))))
        (task-join t1) (task-join t2)
        (displayln (sort (list (channel-recv seen) (channel-recv seen)) (lambda (a b) (eq? (car a) 'one))))
        (displayln (parameterize ((p 'inherited)) (task-join (spawn (lambda () (p))))))
        (displayln (task-join (spawn (lambda () (with-output-to-string (lambda () (display 'a) (sleep 1) (display 'b)))))))
        (displayln (task-join (spawn (lambda () (call/cc (lambda (k) (sleep 1) (k 'escaped-after-sleep)))))))
        (displayln (guard (e (#t 'restart-ok)) (task-join (spawn (lambda () (restart-case (begin (sleep 1) (error \"x\")) (use-value (v) v)))))))
        (displayln (guard (e (#t 'deadlock)) (channel-recv (make-channel))))
        (define (deep-raise n) (if (= n 0) (with-exception-handler (lambda (c) 'declined) (lambda () (raise 'inner))) (+ 1 (deep-raise (- n 1)))))
        (displayln (task-join (spawn (lambda () (guard (e (#t 'task-guard)) (sleep 1) (deep-raise 50))))))
        (displayln (guard (e (#t 'top-guard)) (with-exception-handler (lambda (c) 'declined) (lambda () (raise 'x)))))",
        "(a b)\n(a b a b a b a b a b)\n(0 1 4 9 16)\n(10 20 30)\ncar: expected pair, got 5\n(caught late)\ntail-ok\n39800\nslept-in-wind\n(in out)\n((one one) (two top))\ninherited\nab\nescaped-after-sleep\nrestart-ok\ndeadlock\ntask-guard\ntop-guard\n");
}
