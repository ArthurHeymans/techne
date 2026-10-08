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
        (displayln (list (calls 100) (map (lambda (x y) (cons x y)) '(1 2) '(a b))))"#;
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
        "(a b)\n(a b a b a b a b a b)\n(0 1 4 9 16)\n(10 20 30)\ncar: expected pair, got 5\n(caught late)\ntail-ok\n39800\nslept-in-wind\n(in out)\n((one one) (two top))\ninherited\nab\nescaped-after-sleep\nrestart-ok\ndeadlock\ntask-guard\ntop-guard\n");
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
