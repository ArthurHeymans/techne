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
    check_env(name, src, &[], expected);
}

fn check_env(name: &str, src: &str, env: &[(&str, &str)], expected: &str) {
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{name}.scm"));
    std::fs::write(&file, src).unwrap();
    for stress in [None, Some("1"), Some("full")] {
        let (out, err, ok) = run_file_env(&file, stress, env);
        assert!(ok && out == expected, "{name} (stress={stress:?})\nexpected {expected:?}\ngot {out:?}\nstderr: {err}");
    }
}

#[test]
fn short_circuit_forms() {
    let source = r#"
        (define seen '())
        (define (note x) (set! seen (cons x seen)) x)
        (displayln (list (and) (or) (and 'one) (or 'one)))
        (displayln (list
          (and (note 1) (note #f) (error "skipped"))
          (or (note #f) (note 2) (error "skipped"))
          (reverse seen)))
        (set! seen '())
        (and (note 1) (note #t) (note 2))
        (or (note #f) (note 3) (error "skipped"))
        (and (not (= 0 0)) (note 'bad) (error "skipped"))
        (and (not (= 0 1)) (note 4) (note 5))
        (displayln (reverse seen))
        (displayln (list
          (if (and (< 1 2) (not #f) #t) 'yes 'no)
          (if (and (> 1 2) (error "skipped")) 'yes 'no)
          (if (or #f (null? '()) (error "skipped")) 'yes 'no)))
        (displayln (let ((p (list 1))) (eq? (or #f p (error "skipped")) p)))
        (displayln (call-with-values (lambda () (and #t (values 1 2))) list))
        (displayln (call-with-values (lambda () (or #f (values 3 4))) list))
        (define (negated x) (and (not (= x 0)) (not (< x 0)) 'positive))
        (displayln (list (negated 0) (negated -1) (negated 1)))
        (displayln (list
          (and (= 1 1) (not (= 1 2)) 'ok)
          (and (not (= 1 2)) (= 1 1) 'ok)
          (and (not (= 1 1)) (error "skipped"))))
        (define (logical-loop n)
          (or (= n 0) (and (> n 0) #t (logical-loop (- n 1)))))
        (displayln (logical-loop 10000))
    "#;
    let expected =
        "(#t #f one one)\n(#f 2 (1 #f #f 2))\n(1 #t 2 #f 3 4 5)\n(yes no yes)\n#t\n(1 2)\n(3 4)\n(#f #f positive)\n(ok ok #f)\n#t\n";
    for jit in ["0", "1"] {
        check_env("short-circuit", source, &[("TECHNE_JIT", jit), ("TECHNE_JIT_SYNC", "1")], expected);
    }
}

#[test]
fn macros() {
    check(
        "swap",
        "(define-syntax swap! (syntax-rules () ((_ a b) (let ((tmp a)) (set! a b) (set! b tmp)))))
        (define tmp 1) (define y 2) (swap! tmp y) (displayln (list tmp y))",
        "(2 1)\n",
    );
    // A user binding named like a template's free identifier cannot capture it.
    check(
        "free-ids",
        "(define-syntax my-or2 (syntax-rules () ((_ a b) (let ((t a)) (if t t b)))))
        (define (f) (let ((if list) (t 5)) (my-or2 #f t))) (displayln (f))",
        "5\n",
    );
    check(
        "ellipsis",
        "(define-syntax my-list (syntax-rules () ((_ (a b) ...) (list (cons a b) ...))))
        (displayln (my-list (1 2) (3 4)))",
        "((1 . 2) (3 . 4))\n",
    );
    check(
        "nested-ellipsis",
        "(define-syntax flat (syntax-rules () ((_ (x ...) ...) '(x ... ...))))
        (displayln (flat (1 2) (3) (4 5)))",
        "(1 2 3 4 5)\n",
    );
    check(
        "tail-pattern",
        "(define-syntax lastof (syntax-rules () ((_ x ... y) 'y)))
        (displayln (lastof 1 2 3))",
        "3\n",
    );
    check(
        "literals",
        "(define-syntax kw (syntax-rules (=>) ((_ a => b) (+ a b)) ((_ a b) (* a b))))
        (displayln (list (kw 2 => 3) (kw 2 3)))",
        "(5 6)\n",
    );
    check(
        "recursive",
        "(define-syntax my-and (syntax-rules () ((_) #t) ((_ e) e) ((_ e r ...) (if e (my-and r ...) #f))))
        (displayln (list (my-and 1 2 3) (my-and 1 #f 3)))",
        "(3 #f)\n",
    );
    check(
        "local-macro",
        "(define (f x) (define-syntax twice (syntax-rules () ((_ e) (begin e e)))) (define n 0) (twice (set! n (+ n x))) n)
        (displayln (f 5))",
        "10\n",
    );
    check("let-syntax", "(displayln (let-syntax ((inc (syntax-rules () ((_ x) (+ x 1))))) (inc 41)))", "42\n");
    // let-syntax transformers refer to the enclosing macros, letrec-syntax
    // ones to their siblings.
    check(
        "let-syntax-scope",
        "(define-syntax m (syntax-rules () ((_) 'outer)))
        (displayln (let-syntax ((m (syntax-rules () ((_) 'inner))) (n (syntax-rules () ((_) (m))))) (n)))
        (displayln (letrec-syntax ((m (syntax-rules () ((_) 'inner))) (n (syntax-rules () ((_) (m))))) (n)))",
        "outer\ninner\n",
    );
    check(
        "macro-defines",
        "(define-syntax def2 (syntax-rules () ((_ a b v) (begin (define a v) (define b v)))))
        (def2 p q 7) (displayln (+ p q))",
        "14\n",
    );
    check("do", "(displayln (do ((i 0 (+ i 1)) (acc '() (cons i acc))) ((= i 4) acc)))", "(3 2 1 0)\n");
    check(
        "case-lambda",
        "(define f (case-lambda ((x) (list 'one x)) ((x y) (list 'two x y)) ((x . r) (list 'many x r))))
        (displayln (list (f 1) (f 1 2) (f 1 2 3)))",
        "((one 1) (two 1 2) (many 1 (2 3)))\n",
    );
    check(
        "quasiquote",
        "(define x 5) (define l '(a b)) (displayln `(x ,x ,@l (nested ,(+ x 1)) . end))
        (displayln `#(1 ,x)) (displayln `(1 `(2 ,(3 ,x))))",
        "(x 5 a b (nested 6) . end)\n#(1 5)\n(1 (quasiquote (2 (unquote (3 5)))))\n",
    );
}

#[test]
fn records_and_values() {
    check(
        "records",
        "(define-record-type <point> (make-point x y) point? (x point-x set-point-x!) (y point-y))
        (define p (make-point 1 2)) (set-point-x! p 10)
        (displayln (list (point-x p) (point-y p) (point? p) (point? 5))) (displayln p)",
        "(10 2 #t #f)\n#<point 10 2>\n",
    );
    check(
        "record-type-error",
        "(define-record-type a (make-a x) a? (x a-x)) (define-record-type b (make-b x) b? (x b-x))
        (displayln (guard (e (#t (error-object-message e))) (a-x (make-b 1))))",
        "record accessor: expected a, got #<b 1>\n",
    );
    check(
        "values",
        "(call-with-values (lambda () (values 1 2 3)) (lambda (a b c) (displayln (+ a b c))))
        (let-values (((q r) (values 7 2)) ((s) (values 1))) (displayln (list q r s)))
        (define-values (u v) (values 'x 'y)) (displayln (list u v))
        (receive (a . rest) (values 1 2 3) (displayln (list a rest)))",
        "6\n(7 2 1)\n(x y)\n(1 (2 3))\n",
    );
    check(
        "apply",
        "(displayln (list (apply + 1 2 '(3 4)) (apply max '(3 9 2)) (apply (lambda args args) '(a b))))
        (define (f . xs) (if (null? xs) 0 (+ (car xs) (apply f (cdr xs))))) (displayln (f 1 2 3))",
        "(10 9 (a b))\n6\n",
    );
}

#[test]
fn conditions() {
    check(
        "guard-error",
        "(displayln (guard (e ((error-object? e) (list (error-object-message e) (error-object-irritants e))))
        (error \"bad thing\" 1 'two)))",
        "(bad thing (1 two))\n",
    );
    check("guard-raise", "(displayln (guard (e ((symbol? e) (list 'sym e)) ((string? e) 'str)) (raise 'oops)))", "(sym oops)\n");
    check(
        "guard-reraise",
        "(displayln (guard (outer (#t (list 'outer outer))) (guard (inner ((string? inner) 'no)) (raise 42))))",
        "(outer 42)\n",
    );
    check(
        "runtime-errors",
        "(define (f x) (car x))
        (displayln (guard (e (#t (error-object-message e))) (f 5)))
        (displayln (guard (e (#t 'caught)) (vector-ref (vector 1) 9)))
        (displayln (guard (e (#t (error-object-message e))) (undefined-thing)))",
        "car: expected pair, got 5\ncaught\nunbound variable: undefined-thing\n",
    );
    check(
        "guard-unwinds-frames",
        "(define (deep n) (if (= n 0) (raise 'bottom) (+ 1 (deep (- n 1)))))
        (define (deep2 n) n) (displayln (guard (e (#t e)) (deep 1000))) (displayln (deep2 3))",
        "bottom\n3\n",
    );
    check(
        "guard-in-loop",
        "(define (safe-div a b) (guard (e (#t 'div0)) (if (= b 0) (raise 'z) (quotient a b))))
        (displayln (map (lambda (b) (safe-div 10 b)) '(1 0 5)))",
        "(10 div0 2)\n",
    );
    check(
        "with-exception-handler",
        "(displayln (with-exception-handler (lambda (e) (* e 2)) (lambda () (+ 1 (raise-continuable 20)))))",
        "41\n",
    );
    check(
        "handler-in-native-call",
        "(displayln (guard (e (#t (list 'caught e))) (map (lambda (x) (if (= x 2) (raise x) x)) '(1 2 3))))",
        "(caught 2)\n",
    );
    check("dynamic-wind", "(define log '())
        (guard (e (#t #f)) (dynamic-wind (lambda () (set! log (cons 'in log))) (lambda () (raise 'x)) (lambda () (set! log (cons 'out log)))))
        (displayln (reverse log))", "(in out)\n");
    check(
        "call-cc",
        "(displayln (call/cc (lambda (k) (for-each (lambda (x) (when (> x 2) (k x))) '(1 2 3 4)) 'none)))
        (displayln (+ 1 (call/cc (lambda (k) 1))))",
        "3\n2\n",
    );
    check(
        "parameterize",
        "(define p (make-parameter 1)) (define (show) (p))
        (displayln (list (show) (parameterize ((p 2)) (show)) (show)))",
        "(1 2 1)\n",
    );
    check("promises", "(define n 0) (define pr (delay (begin (set! n (+ n 1)) n))) (force pr) (displayln (list (force pr) n))", "(1 1)\n");
    check("assert", "(displayln (guard (e (#t (error-object-irritants e))) (assert (= 1 2))))", "((= 1 2))\n");
}

#[test]
fn library() {
    check(
        "strings",
        "(displayln (list (string-split \"a,b,,c\" \",\") (string-join '(\"x\" \"y\") \"-\") (string-contains \"hello\" \"ll\")
        (string-upcase \"abc\") (string-trim \"  x \") (string-index \"abc\" #\\c) (string-replace \"aXbX\" \"X\" \"-\")))",
        "((a b  c) x-y 2 ABC x 2 a-b-)\n",
    );
    check(
        "ports",
        "(define p (open-output-string)) (write 'sym p) (display \" \" p) (write \"s\" p) (displayln (get-output-string p))
        (displayln (with-output-to-string (lambda () (display 1) (display 2))))
        (define in (open-input-string \"line1\\nline2\\n(a b) 42\"))
        (displayln (list (read-line in) (read-line in) (read in) (read in) (eof-object? (read in))))",
        "sym \"s\"\n12\n(line1 line2 (a b) 42 #t)\n",
    );
    check(
        "lists",
        "(displayln (list (sort '(3 1 2) <) (sort < '(5 4)) (filter odd? (iota 6)) (fold + 0 '(1 2 3)) (reduce max 0 '(3 7 2))
        (delete-duplicates '(1 2 1 3 2)) (list-index even? '(1 3 4)) (take '(1 2 3) 2) (drop '(1 2 3) 2) (last '(1 2 3))
        (any even? '(1 3)) (every odd? '(1 3)) (append-map (lambda (x) (list x x)) '(1 2)) (assq 'b '((a 1) (b 2)))))",
        "((1 2 3) (4 5) (1 3 5) 6 7 (1 2 3) 2 (1 2) (3) 3 #f #t (1 1 2 2) (b 2))\n",
    );
    check(
        "map-multi",
        "(displayln (map + '(1 2 3) '(10 20 30))) (for-each (lambda (a b) (display (* a b))) '(1 2) '(3 4)) (newline)",
        "(11 22 33)\n38\n",
    );
    check("hash-tables", "(define h (make-hash-table))
        (for-each (lambda (i) (hash-table-set! h i (* i i))) (iota 100))
        (for-each (lambda (i) (hash-table-delete! h i)) (iota 90))
        (for-each (lambda (round) (for-each (lambda (i) (hash-table-set! h (+ 1000 i) i) (hash-table-delete! h (+ 1000 i))) (iota 50))) (iota 20))
        (hash-table-update!/default h 'n (lambda (x) (+ x 1)) 0) (hash-table-update!/default h 'n (lambda (x) (+ x 1)) 0)
        (displayln (list (hash-table-count h) (hash-table-ref/default h 95 #f) (hash-table-ref/default h 5 'gone) (hash-table-ref/default h 'n 0)
          (length (hash-table-keys h)) (sort (map cdr (filter (lambda (kv) (number? (car kv))) (hash-table->alist h))) <)))",
        "(11 9025 gone 2 11 (8100 8281 8464 8649 8836 9025 9216 9409 9604 9801))\n");
    check(
        "vectors",
        "(define v (vector 1 2 3)) (displayln (list (vector-map (lambda (x) (* x 10)) v) (vector-copy v 1) (vector-append v #(4))))",
        "(#(10 20 30) #(2 3) #(1 2 3 4))\n",
    );
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
    // Runaway recursion is a catchable error, not an abort.
    let (out, err, ok) = run(
        "overflow",
        "(define (deep n) (+ 1 (deep (+ n 1))))\n(define (deep-rest n . more) (+ 1 (deep-rest (+ n 1) n)))\n\
         (displayln (guard (e (#t (condition/report-string e))) (deep 0)))\n\
         (displayln (guard (e (#t (condition/report-string e))) (deep-rest 0)))\n",
        None,
    );
    assert!(ok && out == "stack overflow: recursion too deep\nstack overflow: recursion too deep\n", "{out}{err}");
    let (_, err, ok) = run("unclosed", "(define (f x)\n  (let ((y 1)\n    (+ x y))\n", None);
    assert!(!ok && err.contains("unclosed.scm:2:3: unexpected end of input"), "{err}");
    // Code from an included file is located in that file, also when
    // `include-ci` folds case and the including file has multibyte text.
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("included.scm"), "(define (Boom)\n  (car 1))\n").unwrap();
    let (_, err, ok) = run("includer", ";;; ééééééééé\n(include-ci \"included.scm\")\n(boom)\n", None);
    assert!(!ok && err.contains("included.scm:2:3"), "{err}");
    // The including file's top level has no position for included code.
    std::fs::write(dir.join("included-top.scm"), "(define x 1)\n\n  (car x)\n").unwrap();
    let (_, err, ok) = run("includer-top", ";;; ééééééééé\n(include \"included-top.scm\")\n", None);
    assert!(!ok && err.contains("car: expected pair") && !err.contains("includer-top.scm:"), "{err}");
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
        (displayln (list (h '(1 2 3)) (h (iota 2000))))
        ;; Call sites specialised for a global's closure, after redefinition.
        (define (callee x) (+ x 1))
        (define (caller k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (callee i))) acc)))
        (define (tail-caller x) (callee x))
        (define (adder n) (lambda (x) (+ x n)))
        (define before (list (caller 100) (tail-caller 5)))
        (set! callee (lambda (x) (* x 2)))
        (define doubled (list (caller 100) (tail-caller 5)))
        (set! callee (adder 10))
        (define added (list (caller 100) (tail-caller 5)))
        (set! callee (adder 20))
        (define added2 (list (caller 100) (tail-caller 5)))
        (set! callee abs)
        (displayln (list before doubled added added2 (caller 100) (tail-caller -5)))
        ;; Native calls to rest-argument functions.
        (define (rest-sum a . r) (+ a (length r) (apply + r)))
        (define (kw x #:by [by 2]) (* x by))
        (define (calls k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (rest-sum i) (rest-sum i 1 2) (car (map + (list i) (list 1))) (kw i) (kw i #:by 3) (length (map (lambda (x) x) (list i i))))) acc)))
        (displayln (list (calls 100) (map (lambda (x y) (cons x y)) '(1 2) '(a b))))
        ;; Natives called directly that move the register stack (a comparator
        ;; recursing deeply) or the globals (new definitions), or fail.
        (define (members k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (car (member 2 (list 1 2) (lambda (a b) (deep (* i 100000)) (= a b)))))) acc)))
        (define evals 0)
        (define (fresh i) `(begin (define ,(string->symbol (string-append "fresh" (number->string i))) ,i) (set! evals (+ evals 1)) ,i))
        (define (defines k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (eval (fresh i)) evals)) acc)))
        (displayln (list (members 3) (defines 1500) (report (lambda () (char-upcase 1)))))
        ;; Known fixnums (j, i + 1) live across a native call that moves the
        ;; register stack, which native code stores only then.
        (define cur 0)
        (define (cmp a b) (deep (* cur 300000)) (= a b))
        (define lst (list 1 2))
        (define (moves k) (let loop ((i 0) (acc 0)) (if (< i k) (begin (set! cur i) (let ((j (* i 7))) (loop (+ i 1) (+ acc j (car (member 2 lst cmp)))))) acc)))
        (displayln (moves 4))
        ;; A native that moves the register stack or the globals, then
        ;; fails: the handler reads what the function changed before.
        (define marker 0)
        (define (moving-error k)
          (let ((seen 'none))
            (guard (e (#t (list seen marker (condition/report-string e))))
              (set! seen (list k))
              (set! marker k)
              (if (odd? k)
                  (member 2 (list 1 2) (lambda (a b) (deep 1200000) (car '())))
                  (eval `(begin (define ,(string->symbol (string-append "moved" (number->string k))) 1) (car '())))))))
        (displayln (map moving-error '(1 2 3 4)))
        ;; A loop native code knows to count in fixnums, re-entered with the
        ;; bignum the interpreter computed when it overflowed.
        (define (count-up n) (let loop ((i 0) (x 140737488355320)) (if (= i n) x (loop (+ i 1) (+ x 1)))))
        (displayln (count-up 20))
        ;; A native returning only fixnums, through a global redefined to
        ;; return other things.
        (define len string-length)
        (define (lens k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (len "abc"))) acc)))
        (define lens-before (lens 100))
        (set! len (lambda (s) 0.5))
        (define lens-float (lens 100))
        (set! len string->number)
        (displayln (list lens-before lens-float (report (lambda () (lens 3)))))"#;
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
((5050 6) (9900 10) (5950 15) (6950 25) 4950 5)
(40400 ((1 . a) (2 . b)))
(6 2250000 char-upcase: argument: expected char, got 1)
50
(((1) 1 car: expected pair, got ()) ((2) 2 car: expected pair, got ()) ((3) 3 car: expected pair, got ()) ((4) 4 car: expected pair, got ()))
140737488355340
(300 50.0 +: expected number, got #f)
";
    for stress in [None, Some("1"), Some("full")] {
        for jit in ["0", "1", "20"] {
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
fn bignums() {
    check(
        "bignums",
        r#"(define (fact n) (if (= n 0) 1 (* n (fact (- n 1)))))
(displayln (fact 30))
(displayln (list (* 140737488355327 2) (+ 9223372036854775807 1) (- -9223372036854775808 1) (* -9223372036854775808 -1)))
(displayln (list (quotient (fact 30) (fact 28)) (remainder (fact 25) 1000007) (modulo (- (fact 25)) 1000007) (modulo (fact 25) -1000007)))
(displayln (list (expt 2 100) (expt -3 41) (sqrt (expt 10 40)) (sqrt (+ (expt 10 40) 1))))
(displayln (list (= (expt 2 64) (* (expt 2 32) (expt 2 32))) (< (expt 2 64) (expt 2 65)) (< (- (expt 2 64)) 5) (eqv? (expt 2 70) (expt 2 70)) (equal? (list (expt 2 70)) (list (expt 2 70)))))
(displayln (list (number->string (expt 2 70) 16) (string->number "123456789012345678901234567890") (exact->inexact (expt 2 70)) (exact 1e20) (abs (- (expt 2 70)))))
(displayln (list (even? (expt 2 70)) (odd? (+ (expt 2 70) 1)) (integer? (expt 2 70)) (exact? (expt 2 70)) (/ (expt 2 70) (expt 2 68)) (/ (expt 2 70) 3)))
(displayln (- (expt 2 70) (expt 2 70)))
(displayln 123456789012345678901234567890123)
(define h (make-hash-table)) (hash-table-set! h (expt 2 80) 'big) (displayln (hash-table-ref/default h (* (expt 2 40) (expt 2 40)) #f))
(displayln (case (expt 2 70) ((1180591620717411303424) 'matched) (else 'no)))
(displayln (eval (list '+ (expt 2 70) 1)))
(define (sum-to n) (let loop ((i 0) (acc 0)) (if (= i n) acc (loop (+ i 1) (+ acc (* i 100000000000))))))
(displayln (sum-to 200000))
(displayln (list (/ (- (expt 2 60)) (expt 2 50)) (/ (expt 2 60) 3) (- (expt 2 62) (expt 2 63)) (* (- (expt 2 31)) (expt 2 32)) (+ (- (expt 2 62)) (- (expt 2 62)))))"#,
        "265252859812191058636308480000000
(281474976710654 9223372036854775808 -9223372036854775809 9223372036854775808)
(870 913534 86473 -86473)
(1267650600228229401496703205376 -36472996377170786403 100000000000000000000 100000000000000000000.0)
(#t #t #t #t #t)
(400000000000000000 123456789012345678901234567890 1.1805916207174113e+21 100000000000000000000 1180591620717411303424)
(#t #t #t #t 4 1180591620717411303424/3)
0
123456789012345678901234567890123
big
matched
1180591620717411303425
1999990000000000000000
(-1024 1152921504606846976/3 -4611686018427387904 -9223372036854775808 -9223372036854775808)
",
    );
}

#[test]
fn incremental_gc() {
    // Old objects move between containers, through registers and fresh
    // wrappers while marking is in progress; each GC invariant this needs
    // (write barrier, root rescan, shading promoted objects' fields) was
    // checked to fail without it.
    check("gc-torture", include_str!("gc-torture.scm"), "(0 #t #t)\n");
}

#[test]
fn objects_larger_than_the_nursery() {
    // The smallest nursery (32 KiB) is below the large-object size.
    check_env(
        "small-nursery",
        "(define s (make-string 40000 #\\a)) (define v (make-vector 5000 s)) (define b (make-bytevector 40000 7))
        (displayln (list (string-length (vector-ref v 4999)) (vector-length v) (bytevector-u8-ref b 39999)))",
        &[("TECHNE_NURSERY_KB", "32")],
        "(40000 5000 7)\n",
    );
}

/// A huge number with a small one: the gcd reducing a ratio takes a
/// remainder where num-integer's took a step per bit of the huge one
/// (seconds each here, the parse too, as an integer became a ratio).
#[test]
fn huge_numbers_meet_small_ones_quickly() {
    let start = std::time::Instant::now();
    let (out, err, ok) = run(
        "lopsided",
        "(define x (- (expt 2 1000000) 1))
        (display (list (= (* 6 (/ x 6)) x) (gcd x 6) (= (string->number (number->string x 16) 16) x) (denominator (+ (/ x 3) 1/7)) (round (/ 1 x)) (= (rationalize (/ x 7) 0) (/ x 7))))",
        None,
    );
    assert!(ok && out == "(#t 3 #t 7 0 #t)", "{out} {err}");
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
}

#[test]
fn compiled_fixnum_arithmetic_at_its_limits() {
    // Native code checks overflow at 48 bits and compares with fixnum
    // constants without converting; floats and bignums take the slow paths.
    check_env(
        "jit-fixnum-limits",
        "(define hi 140737488355327) (define lo -140737488355328)
        (define (add a b) (+ a b)) (define (sub a b) (- a b)) (define (mul a b) (* a b)) (define (inc a) (+ a 1))
        (define (is-zero x) (if (= x 0) 'zero 'other))
        (displayln (list (add hi 1) (add lo -1) (add hi lo) (sub lo 1) (sub hi -1) (sub 0 lo)))
        (displayln (list (mul hi 2) (mul lo -1) (mul lo 1) (mul 65536 2147483648) (mul -65536 2147483648) (inc hi) (inc -1)))
        (displayln (list (add 1.5 1) (mul 2 0.5) (is-zero 0) (is-zero 0.0) (is-zero -0.0) (is-zero 1) (is-zero 0.5) (is-zero (* hi 4))))",
        &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1")],
        "(140737488355328 -140737488355329 -1 -140737488355329 140737488355328 140737488355328)
(281474976710654 140737488355328 -140737488355328 140737488355328 -140737488355328 140737488355328 0)
(2.5 1.0 zero zero zero other other other)
",
    );
}

#[test]
fn fixnum_entry_checks() {
    // Entry points of compiled code check the fixnums it knows there: a
    // bignum the interpreter computed leaves that frame to the
    // interpreter, but nothing else may (a loop past a `guard`'s handler,
    // entered from the interpreter, knows nothing of its arguments).
    let src = r#"(define (count-up n) (let loop ((i 0) (x 140737488355320)) (if (= i n) x (loop (+ i 1) (+ x 1)))))
        (define (work n xs) (guard (e (#t 'error)) (let loop ((i 0)) (if (< i n) (loop (+ i (car xs))) i))))
        (displayln (list (count-up 20) (work 100000 '(1)) (work 10 '(1.5))))"#;
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("entry-checks.scm");
    std::fs::write(&file, src).unwrap();
    let (out, err, ok) = run_file_env(&file, None, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_LOG", "1")]);
    assert!(ok && out == "(140737488355340 100000 10.5)\n", "{out:?}\n{err}");
    let handed_back: Vec<_> = err.lines().filter(|l| l.contains("interprets a frame")).collect();
    assert!(handed_back.len() == 1 && handed_back[0].starts_with("jit: count-up "), "{err}");
}

#[test]
fn inlined_natives() {
    // Natives compiled code runs inline: their boundaries (anything but the
    // common case calls the native), a string whose bytes move when it
    // stops being ASCII, and their results live across collections and in
    // a `guard` landing.
    let src = r#"(define (report thunk) (guard (e (#t (list 'err (condition/report-string e)))) (thunk)))
        (define (slen s) (string-length s))
        (define (sref s i) (string-ref s i))
        (define (vlen v) (vector-length v))
        (define (bref b i) (bytevector-u8-ref b i))
        (define (c->i c) (char->integer c))
        (define (ceq a b) (char=? a b))
        (define (try f . args) (report (lambda () (apply f args))))
        (define bv (bytevector 0 127 128 255))
        (define big 140737488355327)
        (displayln (map (lambda (i) (try bref bv i)) (list 0 3 4 -1 1.0 'x big (- -1 big))))
        (displayln (map (lambda (i) (try sref "abc" i)) (list 0 2 3 -1 1.0 'x big)))
        (displayln (list (try sref "" 0) (try slen 'abc) (try slen "aλ") (try sref "aλb" 1) (try vlen "x") (try vlen (vector)) (try bref "ab" 0)))
        (displayln (list (try c->i 65) (try c->i #\λ) (try c->i #\x10FFFF) (try ceq #\a #\a) (try ceq #\a #\b) (try ceq #\a 1) (try ceq "a" "a")))
        ;; A string changed to non-ASCII (its bytes move: indirect) and back.
        (define s (string-copy "abcd"))
        (define (probe) (list (slen s) (sref s 1) (sref s 3) (try sref s 4)))
        (define p1 (probe))
        (string-set! s 1 #\λ)
        (define p2 (probe))
        (string-set! s 1 #\b)
        (displayln (list p1 p2 (probe)))
        ;; Inline results and pointers live across allocation (collections under
        ;; stress) and an error landing in a guard that reads them.
        (define (keep n)
          (let loop ((i 0) (acc '()))
            (if (= i n)
                (car acc)
                (let* ((str (string-copy "xyz")) (len (string-length str)) (c (string-ref str 2)) (v (make-vector 100 i)))
                  (loop (+ i 1) (cons (list len c (char->integer c) (vector-length v) (bytevector-u8-ref bv 3) str) (if (pair? acc) (list (car acc)) '())))))))
        (define (land str)
          (let ((n -1) (c #f) (b #f))
            (guard (e (#t (list n c (vector-ref b 0) str (condition/report-string e))))
              (let loop ((i 0))
                (set! n (string-length str))
                (set! b (make-vector 50 i))
                (set! c (string-ref str i))
                (loop (+ i 1))))))
        (displayln (list (keep 300) (land (string-copy "pq")) (land "")))"#;
    let expected = r#"(0 255 (err bytevector-u8-ref: bad index 4 for #u8(0 127 128 255)) (err bytevector-u8-ref: negative index -1) 127 (err bytevector-u8-ref: expected number, got x) (err bytevector-u8-ref: bad index 140737488355327 for #u8(0 127 128 255)) (err bytevector-u8-ref: negative index -140737488355328))
(a c (err string-ref: bad index 3 for "abc") (err string-ref: negative index -1) b (err string-ref: expected number, got x) (err string-ref: bad index 140737488355327 for "abc"))
((err string-ref: bad index 0 for "") (err string-length: expected string, got abc) 2 λ (err vector-length: expected vector, got "x") 0 (err bytevector-u8-ref: expected bytevector, got "ab"))
((err char->integer: expected char, got 65) 955 1114111 #t #f (err char=?: expected char, got 1) (err char=?: expected char, got "a"))
((4 b d (err string-ref: bad index 4 for "abcd")) (4 λ d (err string-ref: bad index 4 for "aλcd")) (4 b d (err string-ref: bad index 4 for "abcd")))
((3 z 122 100 255 xyz) (2 q 2 pq string-ref: bad index 2 for "pq") (0 #f 0  string-ref: bad index 0 for ""))
"#;
    for jit in ["0", "1"] {
        check_env("inlined-natives", src, &[("TECHNE_JIT", jit), ("TECHNE_JIT_SYNC", "1")], expected);
    }
}

#[test]
fn inlined_calls() {
    // Compiled callers run small functions inline. What the interpreter
    // sees when native code leaves inside one (errors, traces, overflow to
    // bignums, preemption, waits, a moved register stack), captured
    // variables after natives in them collect, and calls after a
    // redefinition, also by such a native, must be as without it. Not
    // under GC stress only: it promotes objects before they could move.
    let src = r#"(define-record-type point (make-point x y) point? (x point-x) (y point-y))
(define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
(define (sq x) (* x x))
(define (second l) (car (cdr l)))
(define (first-char s) (string-ref s 0))
(define (norm p) (+ (abs (point-x p)) (abs (point-y p))))
(define (bump! v i) (vector-set! v i (+ 1 (vector-ref v i))) (vector-ref v (+ i 1)))
(define (use f k x) (let loop ((i 0) (acc '())) (if (< i k) (loop (+ i 1) (cons (f x) acc)) (car acc))))
(define (use-sq k x) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (sq x))) acc)))
(define (use-second k l) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (second l))) acc)))
(define (use-char k s) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (char->integer (first-char s)))) acc)))
(define (use-norm k p) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (norm p))) acc)))
(define (use-bump k v i) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (bump! v i))) acc)))
(define v (make-vector 3 0))
(displayln (list (use-sq 100 3) (use-sq 10 4.5) (use-sq 3 12345678901) (use-second 100 '(1 2 3))
                 (use-char 100 "abc") (use-norm 100 (make-point 3 -4)) (use-bump 10 v 1) v))
(displayln (list (report (lambda () (use-sq 100 'x))) (report (lambda () (use-second 100 '(1))))
                 (report (lambda () (use-char 100 ""))) (report (lambda () (use-norm 100 'p)))
                 (report (lambda () (use-bump 10 v 2))) v))
;; Redefined: the guard fails, the call is made.
(set! sq (lambda (x) (+ x 0.5)))
(set! second (let ((k 2)) (lambda (l) (list-ref l k))))
(displayln (list (use-sq 10 3) (use-second 10 '(1 2 3))))
;; In tasks, preempted inside compiled code with inlined calls.
(define log '())
(define (tick! name i) (when (= 0 (modulo i 1000)) (set! log (cons name log))) i)
(define (count name k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (tick! name i))) acc)))
(define a (spawn (lambda () (count 'a 30000))))
(define b (spawn (lambda () (count 'b 30000))))
(displayln (list (task-join a) (task-join b) (reverse log)))
;; Again, each inlined call leaving native code before it changed
;; anything: the interpreter makes the call, counted once.
(define (big x) (* x 4611686018427387904))
(define (count-big name k)
  (let loop ((i 0) (acc 0))
    (when (= 0 (modulo i 500)) (set! log (cons name log)))
    (if (< i k) (loop (+ i 1) (+ acc (big i))) (exact->inexact acc))))
(set! log '())
(define c (spawn (lambda () (count-big 'c 5000))))
(define d (spawn (lambda () (count-big 'd 5000))))
(displayln (list (task-join c) (task-join d) (reverse log)))
;; Leaving native code after an inlined call changed something: the
;; interpreter goes on in its frame, then in the caller's.
(define (bump-big! v) (vector-set! v 0 (+ 1 (vector-ref v 0))) (* (vector-ref v 0) 4611686018427387904))
(define (use-bump-big k v) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc j (bump-big! v))) (list j acc v))))
(displayln (use-bump-big 10 (vector 0)))
;; Closures inlined whose natives collect: their captured variables are
;; read after the collection moved them.
(define fs
  (let ((x 1) (p (cons 1 2)) (big 140737488355327))
    (list (lambda (n) (collect-garbage) (set! x n) x)
          (lambda () x)
          (lambda () (collect-garbage) (car p))
          (lambda (n) (collect-garbage) (* n big))
          (lambda () (collect-garbage) (car x)))))
(define set-x (list-ref fs 0))
(define (use-set-x n) (+ 1 (set-x n)))
(define get-p (list-ref fs 2))
(define (use-pair) (+ 1 (get-p)))
(define times-big (list-ref fs 3))
(define (use-big n) (list (times-big n)))
(define car-x (list-ref fs 4))
(define (use-car-x) (list 0 (car-x)))
(displayln (list (use-set-x 41) ((list-ref fs 1)) (use-pair) (use-set-x 7) ((list-ref fs 1)) (use-big 3)
                 (report (lambda () (use-car-x)))))
;; A native that collects and redefines the global of the closure running.
(define y 0)
(define (redefine! a b) (set! redefining (lambda (n) -1)) (collect-garbage) #t)
(define (redefining n) (when (= n 29) (member 0 '(0) redefine!)) (set! y (+ y n)) y)
(define (use-redefining n) (+ 1 (redefining n)))
(displayln (let loop ((n 0) (acc '())) (if (< n 32) (loop (+ n 1) (cons (use-redefining n) acc)) (list y (reverse acc)))))
;; Waits in an inlined function, and a native growing the register stack.
(define ch (make-channel))
(define (get) (channel-recv ch))
(define (sum-get k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (get))) acc)))
(define consumer (spawn (lambda () (sum-get 50))))
(spawn (lambda () (for-each (lambda (i) (channel-send ch i)) (iota 50))))
(define (deep n) (if (= n 0) 0 (+ 1 (deep (- n 1)))))
(define depth 0)
(define (cmp a b) (deep depth) (= a b))
(define (mem x l) (member x l cmp))
(define (grow k) (let loop ((i 0) (acc 0)) (if (< i k) (begin (set! depth (* i 200000)) (loop (+ i 1) (+ acc (length (mem 2 '(1 2)))))) acc)))
(displayln (list (task-join consumer) (grow 4)))
;; Uncaught: the trace has the inlined function's frame.
(use-char 10 "")"#;
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("inlined-calls.scm");
    std::fs::write(&file, src).unwrap();
    let reference = run_file_env(&file, None, &[("TECHNE_JIT", "0")]);
    assert!(reference.0.starts_with("(900 202.5 457247362579789703403 200 9700 700 0 #(0 10 0))\n"), "{reference:?}");
    assert!(reference.0.contains("(42 41 2 8 7 (422212465065981) car: expected pair, got 7)\n"), "{reference:?}");
    assert!(reference.1.contains("in first-char (") && reference.1.contains("inlined-calls.scm:5:24)"), "{reference:?}");
    for (stress, jit) in [(None, "1"), (None, "20"), (Some("1"), "1"), (Some("full"), "5")] {
        let got = run_file_env(&file, stress, &[("TECHNE_JIT", jit), ("TECHNE_JIT_SYNC", "1")]);
        assert_eq!(got, reference, "jit={jit} stress={stress:?}");
    }
    // Disabling inlining skips only the IR assertions, not equivalence.
    if std::env::var("TECHNE_JIT_INLINE").as_deref() != Ok("0") {
        // The calls are inlined indeed.
        let (_, ir, _) = run_file_env(&file, None, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_IR", "1")]);
        for caller in [
            "use-sq",
            "use-second",
            "use-char",
            "norm",
            "use-bump",
            "count",
            "count-big",
            "use-bump-big",
            "use-set-x",
            "use-pair",
            "use-big",
            "use-car-x",
            "use-redefining",
            "sum-get",
            "grow",
        ] {
            let line =
                ir.lines().find(|l| l.starts_with(&format!("ir: {caller} "))).unwrap_or_else(|| panic!("{caller} not compiled: {ir}"));
            assert!(!line.contains(" at 0 calls"), "{line}");
        }
    }
}

#[test]
fn inlined_tail_calls() {
    // Compiled callers run small functions inline at tail calls too, the
    // callee taking over the caller's frame. What the interpreter sees
    // when native code leaves inside one (errors before and after it
    // changed something, overflow, waits, preemption, a collection in a
    // closure) and calls after a redefinition must be as without it, and
    // a trace has no frame of the caller's.
    let src = r#"(define-record-type point (make-point x y) point? (x point-x) (y point-y))
(define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
(define (sq x) (* x x))
(define (sq-of p) (sq (point-x p)))
(define (second l) (car (cdr l)))
(define (snd l) (second l))
(define (px p) (point-x p))
(define (bump! v i) (vector-set! v i (+ 1 (vector-ref v i))) (vector-ref v (+ i 1)))
(define (tbump v i) (bump! v i))
(define (sub2 a b) (- a b))
(define (seven) (sub2 10 3))
(define (use f k x) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (f x))) acc)))
(define v (vector 0 1 2))
(displayln (list (use sq-of 100 (make-point 3 4)) (use snd 100 '(1 2 3)) (use px 100 (make-point 5 6))
                 (use (lambda (v) (tbump v 0)) 10 v) v (use sq-of 3 (make-point 12345678901 0)) (seven)))
(displayln (list (report (lambda () (use sq-of 100 'x))) (report (lambda () (use snd 100 '(1))))
                 (report (lambda () (use px 10 'q))) (report (lambda () (use (lambda (v) (tbump v 2)) 10 v))) v))
;; A closure collecting garbage, its captured variable read after.
(define counter (let ((n 0)) (lambda (k) (collect-garbage) (set! n (+ n k)) n)))
(define (next k) (counter k))
(define (count-up k) (let loop ((i 0) (acc '())) (if (< i k) (loop (+ i 1) (cons (next 2) acc)) acc)))
(displayln (count-up 5))
;; Waits in a native called by a function run inline at a tail call.
(define ch (make-channel))
(define (get c) (channel-recv c))
(define (get2 c) (get c))
(define (sum-get k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (get2 ch))) acc)))
(define consumer (spawn (lambda () (sum-get 50))))
(spawn (lambda () (for-each (lambda (i) (channel-send ch i)) (iota 50))))
(displayln (task-join consumer))
;; Preempted in tasks.
(define log '())
(define (tick! name i) (when (= 0 (modulo i 1000)) (set! log (cons name log))) i)
(define (tk name i) (tick! name i))
(define (count name k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (tk name i))) acc)))
(define a (spawn (lambda () (count 'a 30000))))
(define b (spawn (lambda () (count 'b 30000))))
(displayln (list (task-join a) (task-join b) (reverse log)))
;; Redefined: the guard fails, the tail call is made.
(set! sq (lambda (x) (+ x 0.5)))
(set! second (let ((k 2)) (lambda (l) (list-ref l k))))
(displayln (list (use sq-of 10 (make-point 3 4)) (use snd 10 '(1 2 3))))
;; Uncaught after the callee changed something: the trace has its frame,
;; not the caller's.
(use (lambda (v) (tbump v 2)) 10 v)"#;
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("inlined-tail-calls.scm");
    std::fs::write(&file, src).unwrap();
    let reference = run_file_env(&file, None, &[("TECHNE_JIT", "0")]);
    assert!(reference.0.starts_with("(900 200 500 10 #(10 1 2) 457247362579789703403 7)\n"), "{reference:?}");
    assert!(reference.1.contains("in bump! (") && !reference.1.contains("in tbump"), "{reference:?}");
    for (stress, jit) in [(None, "1"), (None, "20"), (Some("1"), "1"), (Some("full"), "5")] {
        let got = run_file_env(&file, stress, &[("TECHNE_JIT", jit), ("TECHNE_JIT_SYNC", "1")]);
        assert_eq!(got, reference, "jit={jit} stress={stress:?}");
    }
    if std::env::var("TECHNE_JIT_INLINE").as_deref() != Ok("0") {
        // The tail calls are inlined indeed.
        let (_, ir, _) = run_file_env(&file, None, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_IR", "1")]);
        // (Those of `next`, `get2` and `tk` in their callers, nested.)
        for caller in ["sq-of", "snd", "px", "count-up", "sum-get", "count"] {
            let line =
                ir.lines().find(|l| l.starts_with(&format!("ir: {caller} "))).unwrap_or_else(|| panic!("{caller} not compiled: {ir}"));
            assert!(line.contains(", 1 tail)"), "{line}");
        }
    }
}

#[test]
fn inlined_nested_calls() {
    // Functions run inline run the small functions they call inline too.
    // What the interpreter sees when native code leaves in one (errors
    // before and after the others changed something, overflow, waits,
    // preemption, collections in closures) and calls after a redefinition
    // of an inner one must be as without it: a trace has the frames of
    // each, but those tail calls replaced.
    let src = r#"(define-record-type point (make-point x y) point? (x point-x) (y point-y))
(define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
(define (sum-xy p) (+ (point-x p) (point-y p)))
(define (use-sum k p) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (sum-xy p))) acc)))
(define (sq x) (* x x))
(define (sq1 x) (+ (sq x) 1))
(define (use-sq1 k x) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (sq1 x))) acc)))
(define (inner v i) (vector-ref v (+ i 1)))
(define (outer v i) (vector-set! v 0 i) (+ 1 (inner v i)))
(define (mid v i) (vector-set! v 0 i) (inner v i))
(define (top3 v i) (+ 2 (outer v i)))
(define (inc! v) (vector-set! v 0 (+ 1 (vector-ref v 0))) (vector-ref v 0))
(define (inc-big! v) (* (inc! v) 4611686018427387904))
(define (use2 f k v i) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (f v i))) acc)))
(define (use-outer k v i) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (outer v i))) acc)))
(define (use-mid k v i) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (mid v i))) acc)))
(define (use-top3 k v i) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (top3 v i))) acc)))
(define (use-inc-big k v) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc j (inc-big! v))) (list j acc v))))
(define v (vector 0 1 2 3))
(displayln (list (use-sum 100 (make-point 3 4)) (use-sq1 100 3) (use-sq1 3 12345678901) (use-sq1 3 2.5)
                 (use-outer 10 v 1) (use-mid 10 v 2) (use-top3 10 v 0) v (use-inc-big 5 (vector 0))))
(displayln (list (report (lambda () (use-sum 100 'p))) (report (lambda () (use-sq1 10 'x)))
                 (report (lambda () (use-outer 10 v 3))) (report (lambda () (use-mid 10 v 3)))
                 (report (lambda () (use-top3 10 v 3))) v))
;; Left inside the inner function after the outer changed something,
;; continuing (overflow): the outer one's registers and closure are read
;; after the inner returns.
(define (inner-big v) (* (vector-ref v 0) 4611686018427387904))
(define outer-c (let ((k 10)) (lambda (v i) (vector-set! v 0 i) (let ((a (* i 3))) (+ a (inner-big v) k)))))
(define (use-outer-c k v) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (outer-c v j))) acc)))
;; A function returning what the functions it tail-calls return.
(define (half x) (/ x 2))
(define (pick x) (if (> x 0) (half x) (sq x)))
(define (use-pick k x) (let loop ((j 0) (acc 0)) (if (< j k) (loop (+ j 1) (+ acc (pick x))) acc)))
(displayln (list (use-outer-c 5 (vector 0)) (use-pick 10 3) (use-pick 10 -3)))
;; Closures collecting garbage, nested, their captured variables read after.
(define counter (let ((n 0)) (lambda (k) (collect-garbage) (set! n (+ n k)) n)))
(define (twice k) (+ (counter k) (counter k)))
(define (use-twice k) (let loop ((i 0) (acc '())) (if (< i k) (loop (+ i 1) (cons (twice i) acc)) acc)))
(displayln (use-twice 5))
;; Waits in a native called by a function inlined in one inlined.
(define ch (make-channel))
(define (get c) (channel-recv c))
(define (get-plus c) (+ 1 (get c)))
(define (sum-get k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (get-plus ch))) acc)))
(define consumer (spawn (lambda () (sum-get 50))))
(spawn (lambda () (for-each (lambda (i) (channel-send ch i)) (iota 50))))
(displayln (task-join consumer))
;; Preempted in tasks.
(define log '())
(define (tick! name i) (when (= 0 (modulo i 1000)) (set! log (cons name log))) i)
(define (tk2 name i) (+ 0 (tick! name i)))
(define (count name k) (let loop ((i 0) (acc 0)) (if (< i k) (loop (+ i 1) (+ acc (tk2 name i))) acc)))
(define a (spawn (lambda () (count 'a 30000))))
(define b (spawn (lambda () (count 'b 30000))))
(displayln (list (task-join a) (task-join b) (reverse log)))
;; Redefined, the inner function: its guard fails, the call is made.
(set! point-x (lambda (p) 100))
(set! inner (lambda (v i) (vector-ref v i)))
(set! sq (let ((k 0.5)) (lambda (x) (+ x k))))
(displayln (list (use-sum 10 (make-point 3 4)) (use-outer 10 v 1) (use-mid 10 v 2) (use-sq1 10 3) v))
;; Uncaught in the innermost after the others changed something: the trace
;; has each inlined function's frame but those tail calls replaced.
(set! inner (lambda (v i) (vector-ref v (+ i 1))))
(use-top3 10 v 3)"#;
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("inlined-nested-calls.scm");
    std::fs::write(&file, src).unwrap();
    let reference = run_file_env(&file, None, &[("TECHNE_JIT", "0")]);
    assert!(reference.0.starts_with("(700 1000 457247362579789703406 21.75 30 30 40 "), "{reference:?}");
    assert!(reference.1.contains("in outer (") && reference.1.contains("in top3 ("), "{reference:?}");
    for (stress, jit) in [(None, "1"), (None, "20"), (Some("1"), "1"), (Some("full"), "5")] {
        let got = run_file_env(&file, stress, &[("TECHNE_JIT", jit), ("TECHNE_JIT_SYNC", "1")]);
        assert_eq!(got, reference, "jit={jit} stress={stress:?}");
    }
    if std::env::var("TECHNE_JIT_INLINE").as_deref() != Ok("0") {
        // The calls are inlined, nested, indeed.
        let (_, ir, _) = run_file_env(&file, None, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_IR", "1")]);
        for (caller, sites) in [
            ("use-sum", "3 calls, 0 tail"),
            ("use-sq1", "2 calls, 0 tail"),
            ("use-outer", "2 calls, 0 tail"),
            ("use-mid", "2 calls, 1 tail"),
            ("use-top3", "3 calls, 0 tail"),
            ("use-inc-big", "2 calls, 0 tail"),
            ("use-outer-c", "2 calls, 0 tail"),
            ("use-pick", "3 calls, 2 tail"),
            ("use-twice", "3 calls, 0 tail"),
            ("sum-get", "2 calls, 0 tail"),
            ("count", "2 calls, 0 tail"),
        ] {
            let line =
                ir.lines().find(|l| l.starts_with(&format!("ir: {caller} "))).unwrap_or_else(|| panic!("{caller} not compiled: {ir}"));
            assert!(line.contains(&format!(" inlined at {sites})")), "{line}");
        }
    }
}

#[test]
fn inline_replay_arguments() {
    // A nested zero-argument entry can clear the outer call's original
    // arguments. Both kinds of entry and tail-frame replacement matter.
    for (nested, body) in [("tail", "(inner)"), ("call", "(+ 1 (inner))")] {
        for (root, call) in [("tail", "(outer x)"), ("call", "(list 'result (outer x))")] {
            let src = format!(
                "(define (inner) missing-variable)
                 (define (outer x) (if (pair? x) {body} 'not-a-pair))
                 (define (caller x) {call})
                 (displayln (guard (e (#t (condition/report-string e))) (caller '(1))))"
            );
            let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join(format!("inline-replay-arguments-{root}-{nested}.scm"));
            std::fs::write(&file, src).unwrap();
            let reference = run_file_env(&file, None, &[("TECHNE_JIT", "0")]);
            assert!(reference.0.contains("unbound variable: missing-variable"), "{reference:?}");
            for stress in [None, Some("1"), Some("full")] {
                for inline in ["0", "1"] {
                    let got = run_file_env(&file, stress, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_INLINE", inline)]);
                    assert_eq!(got, reference, "root={root} nested={nested} stress={stress:?} inline={inline}");
                }
            }
            let (_, ir, _) = run_file_env(
                &file,
                None,
                &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_INLINE", "1"), ("TECHNE_JIT_IR", "1")],
            );
            let line = ir.lines().find(|l| l.starts_with("ir: caller ")).unwrap_or_else(|| panic!("{ir}"));
            assert!(line.contains(" at 2 calls"), "{line}");
        }
    }
}

#[test]
fn inline_replay_fuel() {
    // Overflow after multiple nested calls, also after a completed sibling:
    // replay must not charge those calls a second time and shorten slices.
    for (name, body) in [("depth", "(+ 1 (mid x))"), ("sibling", "(+ (inner 1) (mid x))")] {
        let src = format!(
            "(define (inner x) (* x 10000000000))
             (define (mid x) (+ 1 (inner x)))
             (define (outer x) {body})
             (define log '())
             (define (run name)
               (let loop ((i 0))
                 (when (= 0 (modulo i 500)) (set! log (cons (list name i) log)))
                 (if (< i 10000) (begin (outer 10000000000) (loop (+ i 1))) i)))
             (define a (spawn (lambda () (run 'a))))
             (define b (spawn (lambda () (run 'b))))
             (displayln (list (task-join a) (task-join b)))
             (displayln (reverse log))"
        );
        let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("inline-replay-fuel-{name}.scm"));
        std::fs::write(&file, src).unwrap();
        let reference = run_file_env(&file, None, &[("TECHNE_JIT", "0")]);
        assert!(reference.2 && reference.0.starts_with("(10000 10000)\n"), "{reference:?}");
        for stress in [None, Some("1"), Some("full")] {
            for inline in ["0", "1"] {
                let got = run_file_env(&file, stress, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_INLINE", inline)]);
                assert_eq!(got, reference, "{name} stress={stress:?} inline={inline}");
            }
        }
        let (_, ir, _) =
            run_file_env(&file, None, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_INLINE", "1"), ("TECHNE_JIT_IR", "1")]);
        let line = ir.lines().find(|l| l.starts_with("ir: run ")).unwrap_or_else(|| panic!("{ir}"));
        let sites = if name == "depth" { 3 } else { 4 };
        assert!(line.contains(&format!(" at {sites} calls")), "{line}");
    }
}

#[test]
fn inlined_calls_need_room() {
    // A function running a call inline whose frame reaches past its own
    // checks that the register stack has room for both when entered: deep
    // recursion grows it, and once it cannot grow (the memory limit) the
    // interpreter runs the function, which fails where it would have.
    let src = r#"(define (h a b c) (let ((x (* a b)) (y (* b c)) (z (* a c))) (+ x y z (- x y) (- y z))))
(define (deep n) (if (= n 0) 0 (+ (h 1 1 1) (deep (- n 1)))))
(define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
(displayln (list (deep 100000) (report (lambda () (deep 10000000))) (deep 1000)))"#;
    let dir = std::env::temp_dir().join(format!("techne-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("inlined-room.scm");
    std::fs::write(&file, src).unwrap();
    let limit = ("TECHNE_MEMORY_MB", "160");
    let reference = run_file_env(&file, None, &[("TECHNE_JIT", "0"), limit]);
    assert!(reference.0.starts_with("(300000 out of memory") && reference.0.ends_with(" 3000)\n"), "{reference:?}");
    let env = [("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1"), ("TECHNE_JIT_LOG", "1"), ("TECHNE_JIT_IR", "1"), limit];
    let (out, log, ok) = run_file_env(&file, None, &env);
    assert!(ok && out == reference.0, "{out}\n{log}");
    if std::env::var("TECHNE_JIT_INLINE").as_deref() != Ok("0") {
        assert!(log.contains("ir: deep 11 ops (11 inlined at 1 calls, 0 tail)"), "{log}");
        assert!(log.contains("jit: deep interpreted from now on (no room for its frame)"), "{log}");
    }
}

#[test]
fn inlined_higher_order_calls() {
    // Calls passing lambdas to the prelude's map, filter and co. expand into
    // their definitions, guarded against a redefinition.
    check(
        "inlined-higher-order",
        r#"(define (f k)
          (let ((hits 0) (xs '(1 2 3 4 5 6)))
            (list (map (lambda (x) (* x k)) xs)
                  (filter (lambda (x) (even? x)) xs)
                  (filter-map (lambda (x) (and (odd? x) (* x 10))) xs)
                  (fold (lambda (x acc) (+ x acc)) k xs)
                  (begin (for-each (lambda (x) (when (> x 3) (set! hits (+ hits 1)))) xs) hits)
                  (any (lambda (x) (and (> x 4) x)) xs)
                  (every (lambda (x) (< x 10)) xs)
                  (find (lambda (x) (> x 2)) xs)
                  (append-map (lambda (x) (list x x)) '(1 2))
                  (remove (lambda (x) (= x 3)) xs)
                  (map (lambda (x) (map (lambda (y) (+ x y)) '(10 20))) '(1 2))
                  (map (lambda (x) (define (sq y) (* y y)) (sq x)) '(3 4)))))
        (displayln (f 2))
        (define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
        ;; A lambda of the wrong arity fails as it does when not inlined.
        (displayln (equal? (report (lambda () (map (lambda (x y) x) '(1 2))))
                           (report (lambda () (let ((g (lambda (x y) x))) (map g '(1 2)))))))
        (define (twice l) (map (lambda (x) (* 2 x)) l))
        (define before (twice '(1 2)))
        (define real-map map)
        (set! map (lambda (f l) 'replaced))
        (define after (twice '(1 2)))
        (set! map real-map)
        (displayln (list before after (twice '(3))))"#,
        "((2 4 6 8 10 12) (2 4 6) (10 30 50) 23 3 5 #t 3 (1 1 2 2) (1 2 4 5 6) ((11 21) (12 22)) (9 16))
#t
((2 4) replaced (6))
",
    );
}

#[test]
fn long_lists_of_young_objects() {
    // Lists too long for the nursery are built in the old generation; those
    // holding nursery objects must survive the collections that move them.
    check_env(
        "long-lists",
        "(define (fresh n) (let loop ((i 0) (acc '())) (if (= i n) acc (loop (+ i 1) (cons (string #\\a) acc)))))
        (define (all-a? l) (or (null? l) (and (equal? (car l) \"a\") (all-a? (cdr l)))))
        (define r (reverse (fresh 2000)))
        (define a (append (fresh 2000) '()))
        (define l (apply list (fresh 2000)))
        (define h (make-hash-table))
        (let loop ((i 0)) (when (< i 1000) (hash-table-set! h i (string #\\a)) (loop (+ i 1))))
        (define al (hash-table->alist h))
        (let churn ((n 2000)) (when (> n 0) (make-vector 100 0) (churn (- n 1))))
        (displayln (list (all-a? r) (all-a? a) (all-a? l) (all-a? (map cdr al)) (length al)))",
        &[("TECHNE_NURSERY_KB", "32")],
        "(#t #t #t #t 1000)\n",
    );
}

#[test]
fn task_cancellation() {
    check(
        "cancel",
        r#"(define log '())
        ;; Cancelled while sleeping inside dynamic-wind: the cleanup runs.
        (define t (spawn (lambda () (dynamic-wind (lambda () #f) (lambda () (sleep 100000)) (lambda () (set! log (cons 'cleanup log)))))))
        (sleep 1) (task-cancel t)
        (displayln (list (guard (e (#t (condition/report-string e))) (task-join t)) log))
        ;; A task may catch its cancellation.
        (define t2 (spawn (lambda () (guard (e (#t (list 'caught (condition/report-string e)))) (sleep 100000)))))
        (sleep 1) (task-cancel t2) (displayln (task-join t2))
        ;; Not started yet; cancelling itself; preempted in a loop.
        (define t3 (spawn (lambda () 'never)))
        (task-cancel t3)
        (define t4 (spawn (lambda () (task-cancel (current-task)) 'not-reached)))
        (define t5 (spawn (lambda () (let loop ((i 0)) (loop (+ i 1))))))
        (sleep 1) (task-cancel t5)
        (displayln (map (lambda (t) (guard (e (#t (condition/report-string e))) (task-join t))) (list t3 t4 t5)))
        (task-cancel t) (displayln 'cancelling-a-finished-task-is-fine)
        ;; A task dying of an error also runs its cleanups.
        (define t6 (spawn (lambda () (dynamic-wind (lambda () #f) (lambda () (sleep 1) (car 1)) (lambda () (set! log (cons 'error-cleanup log)))))))
        (displayln (list (guard (e (#t (condition/report-string e))) (task-join t6)) log))"#,
        "(task cancelled (cleanup))\n(caught task cancelled)\n(task cancelled task cancelled task cancelled)\ncancelling-a-finished-task-is-fine\n(car: expected pair, got 1 (error-cleanup cleanup))\n",
    );
}

#[test]
fn documentation() {
    let defs = r#"(define (greet name #:greeting [greeting "hi"]) "Greet NAME." (string-append greeting name))
        (define (just-string) "not a docstring")
        (define-syntax swap! (syntax-rules () "Swap the values of A and B." ((_ a b) (let ((t a)) (set! a b) (set! b t)))))
        (define-record-type point (make-point x y) point? (x point-x set-point-x!))"#;
    check(
        "docstrings",
        &format!("{defs}\n(displayln (list (documentation greet) (documentation just-string) (just-string) (documentation point-x)))"),
        "(Greet NAME. #f not a docstring Return the `x` field of RECORD, a `point` record.)\n",
    );
    let (out, err, ok) = run("help", &format!("{defs}\n(help greet) (help swap!) (help when) (help car) (help no-such-name)"), None);
    assert!(ok, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with(r#"(greet name #:greeting (greeting "hi"))  procedure, "#) && lines[0].ends_with("help.scm:1"), "{out}");
    assert!(lines[3].starts_with("swap!: macro, ") && lines[3].ends_with("help.scm:3"), "{out}");
    assert_eq!(lines[1..3], ["", "Greet NAME."], "{out}");
    assert_eq!(lines[4..8], ["", "Swap the values of A and B.", "(when test body ...)  special form", ""], "{out}");
    assert!(lines[9].starts_with("(car pair)  built-in procedure, ") && lines[9].contains("builtins.rs:"), "{out}");
    assert_eq!(lines[10..12], ["", "Return the first element of PAIR."], "{out}");
    assert_eq!(lines.last(), Some(&"no-such-name: unbound"), "{out}");
    // What the editor's help and checks ask.
    check(
        "descriptions",
        &format!(
            "{defs}\n(displayln (map (lambda (n) (let ((d (binding-description n))) (and d (cdr (assq 'kind d))))) '(greet swap! when car nothing)))
            (displayln (cdr (assq 'params (binding-description 'make-point))))
            (displayln (docstring-problems \"returns X\" 'procedure '(\"x\" \". rest\")))"
        ),
        "(procedure macro special form built-in procedure #f)\n(x y)\n(Start the first line with a capital letter. Make the first line a complete sentence, ending with a period. Use the imperative: \"Return\", not \"returns\". Name the parameter REST in the docstring.)\n",
    );
}

#[test]
fn keywords_and_match() {
    check(
        "keywords",
        "(define (greet name #:greeting [greeting \"hello\"] #:punct [punct \"!\"]) (string-append greeting \", \" name punct))
        (displayln (list (greet \"a\") (greet \"b\" #:greeting \"hi\") (greet \"c\" #:punct \"?\" #:greeting \"yo\")))
        (define (opt a [b 10] [c (* b 2)]) (list a b c)) (displayln (list (opt 1) (opt 1 2) (opt 1 2 3)))
        (define (req #:k k) k) (displayln (guard (e (#t (error-object-message e))) (req)))
        (displayln (guard (e (#t (error-object-message e))) (greet \"x\" #:nope 1)))
        (define (rest-opt a [b 0] . more) (list a b more)) (displayln (list (rest-opt 1) (rest-opt 1 2 3 4)))
        (displayln (apply greet (list \"d\" #:greeting \"hey\")))",
        "(hello, a! hi, b! yo, c?)\n((1 10 20) (1 2 4) (1 2 3))\nreq: missing required keyword argument #:k\ngreet: unknown keyword argument #:nope\n((1 0 ()) (1 2 (3 4)))\nhey, d!\n",
    );
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
    check(
        "generics",
        "(define-record-type point (make-point x y) point? (x point-x) (y point-y))
        (define-generic (describe x)) (define-method (describe x) 'thing) (define-method (describe (n number)) 'number)
        (define-method (describe (i integer)) 'integer) (define-method (describe (l list)) 'list)
        (define-method (describe (p point)) (list 'point (point-x p))) (define-method (describe (r record)) 'record)
        (define-record-type other (make-other) other?)
        (displayln (map describe (list 1 2.5 '(1) '() (make-point 7 0) (make-other) 'sym)))
        (define-generic (area s)) (displayln (guard (e (#t (error-object-irritants e))) (area 5)))
        (displayln (list (procedure? describe) (applicable? area 1) (type-of (make-point 1 1))))",
        "(integer number list list (point 7) record thing)\n(area 5)\n(#t #f point)\n",
    );
    check(
        "restarts",
        "(define (parse-entry s) (restart-case (or (string->number s) (error \"bad number:\" s)) (use-value (v) v) (skip () 'skipped)))
        (displayln (handler-bind ((error-object? (lambda (c) (invoke-restart 'use-value -1)))) (map parse-entry '(\"1\" \"x\" \"3\"))))
        (displayln (with-exception-handler (lambda (c) (invoke-restart 'skip)) (lambda () (map parse-entry '(\"7\" \"y\")))))
        (displayln (list (compute-restarts) (restart-case (+ 1 2) (never () 'no))))
        (displayln (guard (e (#t 'outer)) (handler-bind ((symbol? (lambda (c) 'declined))) (raise 'x))))",
        "(1 -1 3)\n(7 skipped)\n(() 3)\nouter\n",
    );
}

/// Sleeping tasks wake in the order of their deadlines, counted from one
/// start so that how long spawning takes does not matter. Once, without
/// collecting on every allocation (`tasks` does), which only makes the
/// spawning slow. Tasks each waking within 50 ms of their deadlines, 100
/// apart, wake in order; one waking later, the host stalled, makes the run
/// tell nothing.
#[test]
fn sleepers_wake_in_deadline_order() {
    let (out, err, ok) = run(
        "sleepers",
        "(define out (make-channel)) (define t0 (current-jiffy))
         (define (since) (quotient (* 1000 (- (current-jiffy) t0)) (jiffies-per-second)))
         (for-each (lambda (ms) (spawn (lambda () (sleep (max 0 (- ms (since)))) (channel-send out (if (> (since) (+ ms 50)) 'stalled ms))))) '(300 100 200))
         (define woke (list (channel-recv out) (channel-recv out) (channel-recv out)))
         (displayln (if (memq 'stalled woke) 'stalled woke))",
        None,
    );
    assert!(ok && (out == "(100 200 300)\n" || out == "stalled\n"), "{out:?} {err}");
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
        (displayln (guard (e (#t (error-object-message e))) (task-join (spawn (lambda () (sleep 1) (car 5))))))
        (displayln (guard (e ((file-error? e) 'file-error)) (task-join (spawn (lambda () (open-input-file \"/nonexistent/techne\"))))))
        (displayln (task-join (spawn (lambda () (guard (e (#t (list 'caught e))) (sleep 5) (raise 'late))))))
        (define (wait-tail c) (channel-recv c)) (define c2 (make-channel)) (define t (spawn (lambda () (wait-tail c2))))
        (channel-send c2 'tail-ok) (displayln (task-join t))
        (displayln (apply + (map task-join (map (lambda (i) (spawn (lambda () (sleep (modulo i 7)) (* i 2)))) (iota 200)))))
        (define wind-log '())
        (displayln (task-join (spawn (lambda () (dynamic-wind (lambda () (set! wind-log (cons 'in wind-log))) (lambda () (sleep 1) 'slept-in-wind) (lambda () (set! wind-log (cons 'out wind-log))))))))
        (displayln (reverse wind-log))
        (define p (make-parameter 'top))
        (define seen (make-channel 2))
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
        "(a b)\n(a b a b a b a b a b)\n(0 1 4 9 16)\ncar: expected pair, got 5\nfile-error\n(caught late)\ntail-ok\n39800\nslept-in-wind\n(in out)\n((one one) (two top))\ninherited\nab\nescaped-after-sleep\nrestart-ok\ndeadlock\ntask-guard\ntop-guard\n");
    // Durations too long for an `Instant` are catchable errors.
    check(
        "durations",
        "(define (message thunk) (guard (e (#t (condition/report-string e))) (thunk)))
        (displayln (message (lambda () (sleep +inf.0))))
        (displayln (message (lambda () (sleep 1e300))))
        (displayln (message (lambda () (select (timeout +inf.0 'never)))))",
        "sleep: duration out of range: +inf.0\nsleep: duration out of range: 1.0e+300\nselect: duration out of range: +inf.0\n",
    );
}

/// A finished task's slot goes to the next task spawned and a closed,
/// drained channel's to the next channel made; their objects keep working
/// and never reach the new occupant.
#[test]
fn slots_are_reused() {
    check(
        "slots",
        r#"(define t (spawn (lambda () 'first)))
        (define f (spawn (lambda () (raise 'failed))))
        (task-join t) (guard (e (#t e)) (task-join f))
        (define u (spawn (lambda () (sleep 5) 'second)))
        (define v (spawn (lambda () (sleep 5) 'third)))
        (task-cancel t) (task-kill f)
        (displayln (list (task-join t) (guard (e (#t (list 'raised e))) (task-join f)) (task-done? t) (task-done? u)))
        (displayln (list (task-join u) (task-join v)))
        (define me #f)
        (displayln (eq? (task-join (spawn (lambda () (set! me (current-task)) (current-task)))) me))
        (define c (make-channel 1)) (channel-send c 'x) (channel-close c)
        (displayln (list (channel-closed? c) (channel-length c) (channel-recv c)))
        (define d (make-channel 1))
        (displayln (list (channel-closed? c) (channel-length c) (eof-object? (channel-recv c))
                         (guard (e (#t (condition/report-string e))) (channel-send c 'lost))
                         (channel-close c) (channel-closed? d) (channel-length d)))
        (define waiting (make-channel))
        (define reader (spawn (lambda () (channel-recv waiting))))
        (yield) (channel-close waiting)
        (define e (make-channel 1)) (channel-send e 'kept)
        (displayln (list (eof-object? (task-join reader)) (channel-recv e)))"#,
        "(first (raised failed) #t #f)\n(second third)\n#t\n(#t 1 x)\n(#t 0 #t channel-send: channel closed #<void> #f 0)\n(#t kept)\n",
    );
}

#[test]
fn channels() {
    check(
        "channels",
        r#"
        ;; Rendezvous: a send completes when a receiver takes the value.
        (define r (make-channel))
        (define sender (spawn (lambda () (channel-send r 'x) 'sent)))
        (yield) (yield)
        (displayln (list (task-done? sender) (channel-recv r) (task-join sender)))

        ;; A flood into a stalled consumer stays within the buffer.
        (define b (make-channel 8))
        (define most 0)
        (define producer (spawn (lambda () (for-each (lambda (i) (channel-send b i) (set! most (max most (channel-length b)))) (iota 10000)) 'flooded)))
        (sleep 20)
        (define got (let loop ((i 0) (acc 0)) (if (= i 10000) acc (loop (+ i 1) (+ acc (channel-recv b))))))
        (displayln (list (task-join producer) most got))

        ;; Bytes: 100-byte strings into a 1000-byte buffer; one larger message
        ;; still fits an empty buffer.
        (define s (make-channel 100 #:bytes 1000))
        (define most-bytes 0)
        (define bp (spawn (lambda () (for-each (lambda (i) (channel-send s (make-string 100 #\a)) (set! most-bytes (max most-bytes (channel-length s)))) (iota 50)) (channel-send s (make-string 5000 #\b)) 'ok)))
        (sleep 20)
        (define lens (let loop ((i 0) (acc '())) (if (= i 51) acc (loop (+ i 1) (cons (string-length (channel-recv s)) acc)))))
        (displayln (list (task-join bp) most-bytes (car lens) (apply + lens)))

        ;; select: data or timeout, never both, nothing lost or duplicated.
        (define d (make-channel))
        (define sent (spawn (lambda () (for-each (lambda (i) (when (= 0 (modulo i 7)) (sleep 3)) (channel-send d i)) (iota 300)) (channel-close d))))
        (define-values (received timeouts)
          (let loop ((acc '()) (timeouts 0))
            (select (recv d (v) (if (eof-object? v) (values (reverse acc) timeouts) (loop (cons v acc) timeouts)))
                    (timeout 1 (loop acc (+ timeouts 1))))))
        (displayln (list (equal? received (iota 300)) (> timeouts 0)))

        ;; Two selects offering to two channels: each value arrives once.
        (define c1 (make-channel)) (define c2 (make-channel))
        (define (offerer tag) (spawn (lambda () (for-each (lambda (i) (select (send c1 (cons tag i)) (send c2 (cons tag i)))) (iota 100)))))
        (define o1 (offerer 'a)) (define o2 (offerer 'b))
        (define all (let loop ((n 0) (acc '())) (if (= n 200) acc (loop (+ n 1) (cons (select (recv c1 (v) v) (recv c2 (v) v)) acc)))))
        (displayln (list (length all) (length (delete-duplicates all))))

        ;; Cancelling a waiting sender or select withdraws its offers.
        (define w (make-channel))
        (define blocked (spawn (lambda () (channel-send w 'cancelled-send))))
        (define chooser (spawn (lambda () (select (send w 'cancelled-select) (timeout 100000 #f)))))
        (yield) (task-cancel blocked) (task-cancel chooser)
        (displayln (select (recv w (v) v) (timeout 10 'nothing)))
        ;; So does a top-level send abandoned by a deadlock.
        (displayln (guard (e (#t 'deadlock)) (channel-send w 'abandoned)))
        (displayln (select (recv w (v) v) (timeout 10 'nothing)))

        ;; Close: buffered values first, then eof; sends fail.
        (define c (make-channel 2)) (channel-send c 1) (channel-close c)
        (displayln (list (channel-recv c) (eof-object? (channel-recv c)) (channel-closed? c)
                         (guard (e (#t (condition/report-string e))) (channel-send c 2))))
        (define waiting (make-channel))
        (define late (spawn (lambda () (guard (e (#t 'send-failed)) (channel-send waiting 'x)))))
        (yield) (channel-close waiting)
        (displayln (task-join late))
    "#,
        "(#f x sent)\n(flooded 8 49995000)\n(ok 10 5000 10000)\n(#t #t)\n(200 200)\nnothing\ndeadlock\nnothing\n(1 #t #t channel-send: channel closed)\nsend-failed\n",
    );
}

/// Float arithmetic in compiled code, where native code knows registers
/// floats (`jit::flonums`): a known float meeting a fixnum, a bignum, a
/// ratio, a complex number, a non-number; NaNs and infinities; loop heads
/// the interpreter leaves something else in. As the interpreter computes.
#[test]
fn float_loops_in_compiled_code() {
    let source = r#"
(define (sum-to n step) (let loop ((i 0) (acc 0.0)) (if (= i n) acc (loop (+ i 1) (+ acc step)))))
(displayln (list (sum-to 10 0.5) (sum-to 10 1) (sum-to 4 (expt 10 20))))
(define (mix x) (let loop ((z 1.5) (i 0)) (if (= i 6) z (loop (* z (if (= i 3) x 1)) (+ i 1)))))
(displayln (list (mix 2) (mix (expt 10 30)) (mix 1/3) (mix (make-rectangular 0 1))))
(define (grow z n) (let loop ((z z) (i 0)) (if (= i n) z (loop (+ z 0.5) (+ i 1)))))
(displayln (list (grow 0.0 4) (grow 1 4) (grow (make-rectangular 1 1) 4)))
(define (nan-loop n) (let loop ((x 0.0) (i 0)) (if (= i n) (list x (nan? x) (eqv? x x)) (loop (* (+ x +inf.0) 0.0) (+ i 1)))))
(displayln (nan-loop 3))
(define (count-below lim) (let loop ((x 0.0) (k 0)) (if (< x lim) (loop (+ x 0.25) (+ k 1)) k)))
(displayln (list (count-below 2) (count-below 2.1) (count-below 3/2)))
(displayln (guard (e (#t (condition/report-string e))) (let loop ((x 1.0) (i 0)) (if (= i 3) x (loop (+ x (if (= i 1) 'oops 1)) (+ i 1))))))
"#;
    let expected = "(5.0 10.0 400000000000000000000.0)\n(3.0 1.5000000000000002e+30 0.5 0.0+1.5i)\n(2.0 3.0 3.0+i)\n(+nan.0 #t #t)\n(8 9 6)\n+: expected number, got oops\n";
    check_env("float_loops_interp", source, &[("TECHNE_JIT", "0")], expected);
    check_env("float_loops_jit", source, &[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1")], expected);
}

/// Loop heads compiled assuming the kinds the interpreter saw there
/// (`Job::observed`), entered later with others: a float or a bignum where
/// a fixnum was seen, a fixnum or a ratio where a float was. As the
/// interpreter computes.
#[test]
fn assumed_kinds_in_compiled_code() {
    let source = r#"
(define (f0 p0) (+ (let loop ((i 0) (acc p0)) (if (< i 4) (loop (+ i 1) acc) acc)) (/ p0 4)))
(let loop ((k 0)) (when (< k 50) (f0 7) (loop (+ k 1))))
(displayln (list (f0 7) (f0 0.5) (f0 (expt 10 20)) (f0 7)))
(define (scale v k) (let loop ((i 0) (s 0.0)) (if (= i 4) s (loop (+ i 1) (+ s (* k (vector-ref v i)))))))
(define v (vector 1.0 2.0 3.0 4.0))
(let loop ((k 0)) (when (< k 50) (scale v 0.5) (loop (+ k 1))))
(displayln (list (scale v 0.5) (scale v 2) (scale (vector 1 2 3 4) 1/2) (scale v 0.5)))
(define (count-from lo hi) (let loop ((i lo) (n 0)) (if (< i hi) (loop (+ i 1) (+ n 1)) n)))
(let loop ((k 0)) (when (< k 50) (count-from 0 10) (loop (+ k 1))))
(displayln (list (count-from 0 10) (count-from (expt 2 62) (+ (expt 2 62) 5)) (count-from 0.5 3) (count-from 0 10)))
;; A loop whose assumed fixnum comes from a call, long enough for its
;; back-edge to end slices before the assumption is checked.
(define procs (vector (lambda (x) (+ x 1))))
(define (count n) (let loop ((i 0) (acc 0)) (if (< i n) (loop (+ i 1) ((vector-ref procs 0) acc)) acc)))
(displayln (list (count 10) (count 300000)))
"#;
    let expected = "(35/4 0.625 125000000000000000000 35/4)\n(5.0 20.0 5.0 5.0)\n(10 5 3 10)\n(10 300000)\n";
    check_env("assumed_kinds_interp", source, &[("TECHNE_JIT", "0")], expected);
    check_env("assumed_kinds_jit", source, &[("TECHNE_JIT", "5"), ("TECHNE_JIT_SYNC", "1")], expected);
}

#[test]
fn assumed_arguments_in_compiled_code() {
    // Compiled for the kinds of arguments its calls had: other calls (and
    // self tail calls passing others) are interpreted, and later ones of
    // the kinds assumed run compiled again.
    let source = r#"
(define (warm f) (let loop ((k 0)) (when (< k 50) (f) (loop (+ k 1)))))
(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
(warm (lambda () (fib 10)))
(displayln (list (fib 10) (fib 10.5) (fib 9/2) (fib 10)))
(define (count-down n acc) (if (< n 1) acc (count-down (- n 1) (+ acc 1))))
(warm (lambda () (count-down 10 0)))
(displayln (list (count-down 10 0) (count-down 2.5 0) (count-down 3 (expt 10 20)) (count-down 3 0.5) (count-down 10 0)))
(define (sum-to n) (if (< n 1) 0 (+ n (sum-to (- n 1)))))
(warm (lambda () (sum-to 10)))
(displayln (list (sum-to 10) (sum-to 3.5) (sum-to 10)))
(define (walk l acc) (if (null? l) acc (walk (cdr l) (+ acc (car l)))))
(warm (lambda () (walk '(1 2 3) 0)))
(displayln (list (walk '(1 2 3) 0) (walk '(1 2.5 3 4) 0) (walk '(1 2 3) 0)))
"#;
    let expected = "(55 99.5 11/2 55)\n(10 2 100000000000000000003 3.5 10)\n(55 7.5 55)\n(6 10.5 6)\n";
    check_env("assumed_arguments_interp", source, &[("TECHNE_JIT", "0")], expected);
    check_env("assumed_arguments_jit", source, &[("TECHNE_JIT", "5"), ("TECHNE_JIT_SYNC", "1")], expected);
}

#[test]
fn vectors_and_globals_kept_in_compiled_loops() {
    // Compiled loops keep vectors' lengths and the globals they read
    // vectors through or call: what compiled callees change, they read
    // anew.
    let source = r#"
(define (warm f) (let loop ((k 0)) (when (< k 50) (f) (loop (+ k 1)))))
(define at -1)
(define v (vector 1 2 3))
(define (grow-at i) (when (= i at) (set! v (vector 10 20 30 40 50))))
(define (gone-at i) (when (= i at) (set! v 'gone)))
(define (sum-v n hook) (let loop ((i 0) (s 0)) (if (< i n) (begin (hook i) (loop (+ i 1) (+ s (vector-ref v i)))) s)))
(warm (lambda () (sum-v 3 grow-at) (sum-v 3 gone-at)))
(set! at 1)
(displayln (list (sum-v 5 grow-at) (guard (e (#t 'not-a-vector)) (sum-v 5 gone-at))))
(define g (make-vector 4 0))
(define (set-all! x) (let loop ((i 0)) (when (< i 4) (vector-set! g i x) (loop (+ i 1)))))
(warm (lambda () (set-all! 1)))
(define before g)
(set! g '#(1 2 3 4))
(displayln (list before (guard (e (#t 'immutable)) (set-all! 5)) g))
(define (fill! w x) (let loop ((i 0)) (when (< i (vector-length w)) (vector-set! w i x) (loop (+ i 1)))))
(define m (make-vector 3 0))
(warm (lambda () (fill! m 7)))
(displayln (list m (guard (e (#t 'immutable)) (fill! '#(1 2 3) 9))))
(define (sum-alloc w) (let loop ((i 0) (s 0) (junk '())) (if (< i (vector-length w)) (loop (+ i 1) (+ s (vector-ref w i)) (cons (make-vector 50 i) junk)) s)))
(define big (make-vector 1000 1))
(warm (lambda () (sum-alloc big)))
(displayln (list (sum-alloc big) (sum-alloc (vector 1 2))))
(set! at -1)
(define (pick x) x)
(define (redefine-at i) (when (= i at) (set! pick (lambda (x) (* 100 x)))))
(define (run-pick n hook) (let loop ((i 0) (s 0)) (if (< i n) (begin (hook i) (loop (+ i 1) (+ s (pick i)))) s)))
(warm (lambda () (run-pick 4 redefine-at)))
(set! at 1)
(displayln (list (run-pick 4 redefine-at) (run-pick 4 redefine-at)))
"#;
    let expected = "(141 not-a-vector)\n(#(1 1 1 1) immutable #(1 2 3 4))\n(#(7 7 7) immutable)\n(1000 3)\n(600 600)\n";
    check_env("kept_vectors_interp", source, &[("TECHNE_JIT", "0")], expected);
    check_env("kept_vectors_jit", source, &[("TECHNE_JIT", "5"), ("TECHNE_JIT_SYNC", "1")], expected);
}
