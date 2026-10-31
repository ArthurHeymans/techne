//! JSON-RPC over a process (`lisp/editor/rpc.scm`) against a peer in
//! Python: requests and their answers, notifications both ways, the
//! peer's own requests, a request nobody answers, and the peer's death.

use std::path::Path;

use techne_vm::vm::Vm;

/// Answers `echo` with its params, never answers `silent`, asks the
/// client `client/question` when told `ask` and notifies what it
/// answered, says `bye` and exits on `die`, writes a bad frame on
/// `garble`, and notifies every `$/cancelRequest` back as `cancelled`.
const PEER: &str = r#"
import sys, json

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line.strip() == b'': break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(value):
    body = json.dumps(value).encode()
    # A byte at a time: the client's framing must not care.
    for b in b'content-length: %d\r\n\r\n' % len(body) + body:
        sys.stdout.buffer.write(bytes([b]))
    sys.stdout.buffer.flush()

sys.stderr.write('x' * 200000)
sys.stderr.flush()
while True:
    msg = read()
    if msg is None: break
    method = msg.get('method')
    if method == 'echo':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': msg['params']})
    elif method == 'fail':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'error': {'code': 1, 'message': 'it failed'}})
    elif method == 'silent':
        pass
    elif method == '$/cancelRequest':
        send({'jsonrpc': '2.0', 'method': 'cancelled', 'params': msg['params']})
    elif method == 'ask':
        send({'jsonrpc': '2.0', 'id': 'q1', 'method': 'client/question', 'params': {'n': 41}})
        send({'jsonrpc': '2.0', 'id': 'q2', 'method': 'client/unknown', 'params': {}})
        send({'jsonrpc': '2.0', 'id': 'q3', 'method': 'client/broken', 'params': {}})
        for _ in range(3):
            answer = read()
            send({'jsonrpc': '2.0', 'method': 'answered', 'params': answer})
    elif method == 'poke':
        send({'jsonrpc': '2.0', 'method': 'bye-soon', 'params': {}})
    elif method == 'garble':
        sys.stdout.buffer.write(b'Content-Length: nope\r\n\r\n')
        sys.stdout.buffer.flush()
    elif method == 'die':
        send({'jsonrpc': '2.0', 'method': 'bye', 'params': {}})
        sys.exit(0)
"#;

fn vm(dir: &Path) -> Vm {
    let peer = dir.join("peer.py");
    std::fs::write(&peer, PEER).unwrap();
    let mut vm = Vm::new();
    techne_editor::install(&mut vm);
    techne_node::install(&mut vm).unwrap();
    let rpc = techne_editor::runtime::lisp_dir().join("rpc.scm");
    vm.eval_source(&format!("(require {:?})", rpc.display().to_string())).unwrap();
    vm.eval_source(&format!(
        r#"(define notes '())
           (define dead #f)
           (define (notes-of m) (filter-map (lambda (n) (and (equal? (car n) m) (cadr n))) (reverse notes)))
           (define (await-note m)
             (let loop ((n 0))
               (cond ((pair? (notes-of m)) (car (notes-of m)))
                     ((> n 1000) #f)
                     (else (sleep 10) (loop (+ n 1))))))
           (define (in-task thunk) (task-join (spawn thunk)))
           (define conn #f)
           (define asked-back #f)
           (define (note! m p)
             (set! notes (cons (list m p) notes))
             ;; A handler asking the peer something, from where the
             ;; notification came: it must not wait on itself.
             (when (equal? m "bye-soon")
               (set! asked-back (dict-ref (rpc-request! conn "echo" (dict "back" 1) #:timeout 5000) "back"))))
           (set! conn (rpc-connect "python3" (list {peer:?})
                          #:on-note note!
                          #:on-ask (lambda (m p)
                                     (cond ((equal? m "client/question") (dict "answer" (+ 1 (dict-ref p "n"))))
                                           ((equal? m "client/broken") (error "broken handler"))
                                           (else rpc-unsupported)))
                          #:on-dead (lambda () (set! dead #t))))"#,
        peer = peer.display().to_string()
    ))
    .unwrap();
    vm
}

fn eval(vm: &mut Vm, source: &str) -> String {
    let v = vm.eval_source(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    vm.repr(&v)
}

#[test]
fn requests_notifications_and_the_peers_requests() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = vm(dir.path());
    // Answers find their requests, in whatever order they are asked.
    assert_eq!(eval(&mut vm, r#"(in-task (lambda () (dict-ref (rpc-request! conn "echo" (dict "a" "ünï")) "a")))"#), r#""ünï""#);
    assert_eq!(
        eval(
            &mut vm,
            r#"(let ((ts (map (lambda (i)
                                (spawn (lambda ()
                                         ;; Each spends its slice differently first, so
                                         ;; they are preempted at different places.
                                         (let spin ((k (* i 7919))) (unless (= k 0) (spin (- k 1))))
                                         (dict-ref (rpc-request! conn "echo" (dict "i" i) #:timeout 5000) "i"))))
                              '(1 2 3 4 5 6 7 8))))
                 (map task-join ts))"#
        ),
        "(1 2 3 4 5 6 7 8)"
    );
    // An error answer is an error, the request's own.
    assert_eq!(
        eval(&mut vm, r#"(in-task (lambda () (guard (e (#t (error-object-message e))) (rpc-request! conn "fail" (dict)))))"#),
        r#""it failed""#
    );
    // The peer's requests: answered, refused as unknown, failed.
    eval(&mut vm, r#"(rpc-notify! conn "ask" (dict))"#);
    assert_eq!(
        eval(
            &mut vm,
            r#"(in-task (lambda ()
                 (let loop ((n 0)) (when (and (< (length (notes-of "answered")) 3) (< n 1000)) (sleep 10) (loop (+ n 1))))
                 (map (lambda (a) (list (dict-ref a "id")
                                        (let ((r (dict-ref a "result" #f))) (and r (dict-ref r "answer")))
                                        (let ((e (dict-ref a "error" #f))) (and e (dict-ref e "code")))))
                      (notes-of "answered"))))"#
        ),
        r#"(("q1" 42 #f) ("q2" #f -32601) ("q3" #f -32603))"#
    );
    // A notification's handler asks the peer, and is answered.
    eval(&mut vm, r#"(rpc-notify! conn "poke" (dict))"#);
    assert_eq!(
        eval(
            &mut vm,
            r#"(in-task (lambda () (let loop ((n 0)) (unless (or asked-back (> n 500)) (sleep 10) (loop (+ n 1)))) asked-back))"#
        ),
        "1"
    );
    // No answer: the request fails after its timeout, the peer is told,
    // and nobody waits any more.
    assert_eq!(
        eval(
            &mut vm,
            r#"(in-task (lambda () (guard (e (#t (error-object-message e))) (rpc-request! conn "silent" (dict) #:timeout 100))))"#
        ),
        r#""no answer to""#
    );
    assert_eq!(eval(&mut vm, r#"(in-task (lambda () (dict-ref (await-note "cancelled") "id")))"#), "12");
    assert_eq!(eval(&mut vm, "(rpc-waiting conn)"), "0");
    // The peer's death fails who waits, and is said once.
    assert_eq!(
        eval(
            &mut vm,
            r#"(in-task (lambda ()
                 (let ((waiting (spawn (lambda () (guard (e (#t (error-object-message e))) (rpc-request! conn "silent" (dict)))))))
                   (sleep 50)
                   (rpc-notify! conn "die" (dict))
                   (list (task-join waiting) (and (await-note "bye") #t)))))"#
        ),
        r#"("the connection ended" #t)"#
    );
    assert_eq!(
        eval(
            &mut vm,
            r#"(in-task (lambda () (let loop ((n 0)) (unless (or dead (> n 500)) (sleep 10) (loop (+ n 1)))) (list dead (rpc-closed? conn))))"#
        ),
        "(#t #t)"
    );
    assert_eq!(
        eval(&mut vm, r#"(guard (e (#t (error-object-message e))) (rpc-notify! conn "echo" (dict)))"#),
        r#""the connection is closed""#
    );
}

#[test]
fn a_peer_out_of_frame_is_ended() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = vm(dir.path());
    assert_eq!(
        eval(
            &mut vm,
            r#"(in-task (lambda ()
                 (rpc-notify! conn "garble" (dict))
                 (let loop ((n 0)) (unless (or dead (> n 500)) (sleep 10) (loop (+ n 1))))
                 (list dead (rpc-closed? conn))))"#
        ),
        "(#t #t)"
    );
}
