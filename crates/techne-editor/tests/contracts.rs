//! Commands with contracts (doc/operations.md, step 3): run from keys,
//! their arguments read; invoked with them as data, by the user or an
//! agent; validated alike, sensitive ones kept out of what is recorded.

use std::time::{Duration, Instant};

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

fn runtime(text: &str) -> Runtime {
    Runtime::with_document(Document::new(text), "emacs").unwrap()
}

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn eval(rt: &mut Runtime, source: &str) -> String {
    rt.eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

/// What evaluating SOURCE raises, as `error-text` writes it.
fn refused(rt: &mut Runtime, source: &str) -> String {
    eval(rt, &format!("(guard (e (#t (error-text e))) {source} 'ran)"))
}

fn until(rt: &mut Runtime, source: &str, done: impl Fn(&str) -> bool) -> String {
    let start = Instant::now();
    loop {
        let v = eval(rt, source);
        if done(&v) {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out: {source} is {v}");
        rt.run_tasks(Duration::from_millis(10)).unwrap();
    }
}

const S: &str = "(current-session)";
const LAST: &str = "(car (kept-operation-records))";

#[test]
fn from_keys_and_as_data_alike() {
    let mut rt = runtime("abc");
    // From keys: the option, then its value, read; C-u for global.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "set-option");
    keys(&mut rt, "RET");
    type_text(&mut rt, "read-only");
    keys(&mut rt, "RET");
    type_text(&mut rt, "#t");
    keys(&mut rt, "RET");
    assert_eq!(eval(&mut rt, &format!("(option (current-buffer {S}) 'read-only)")), "#t");
    assert_eq!(eval(&mut rt, &format!("(map car (record-arguments {LAST}))")), "(name value global buffer)");
    // As data: the same validation, the same body.
    eval(&mut rt, &format!("(invoke {S} 'set-option (dict 'name 'read-only 'value #f 'buffer (current-buffer {S})))"));
    assert_eq!(eval(&mut rt, &format!("(option (current-buffer {S}) 'read-only)")), "#f");
    // As JSON has it: strings for symbols, keys too.
    eval(&mut rt, &format!("(invoke {S} 'set-option (dict \"name\" \"read-only\" \"value\" #t \"buffer\" \"*scratch*\"))"));
    assert_eq!(eval(&mut rt, &format!("(option (current-buffer {S}) 'read-only)")), "#t");
    // An invalid value is refused, saying why, and changes nothing.
    // Neither a buffer nor global: refused, not set everywhere.
    let why = refused(&mut rt, &format!("(invoke {S} 'set-option (dict 'name 'read-only 'value #f))"));
    assert!(why.contains("Give the buffer"), "{why}");
    let why = refused(&mut rt, &format!("(invoke {S} 'set-option (dict 'name 'read-only 'value 3))"));
    assert!(why.contains("Invalid arguments for set-option") && why.contains("value: expected a boolean, got 3"), "{why}");
    let why = refused(&mut rt, &format!("(invoke {S} 'set-option (dict 'name 'no-such-option 'value 3 'colour 1))"));
    assert!(why.contains("name: expected one of") && why.contains("colour: not an argument"), "{why}");
    assert_eq!(eval(&mut rt, &format!("(option (current-buffer {S}) 'read-only)")), "#t");
    // From keys too: the explanation is the message.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "set-option");
    keys(&mut rt, "RET");
    type_text(&mut rt, "read-only");
    keys(&mut rt, "RET");
    type_text(&mut rt, "3");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().echo.contains("expected a boolean"), "{}", rt.snapshot().echo);
    // Check only: validated and admitted, not run.
    assert_eq!(eval(&mut rt, &format!("(invoke {S} 'set-option (dict 'name 'read-only 'value #f) #:mode 'check)")), "#t");
    assert_eq!(eval(&mut rt, &format!("(option (current-buffer {S}) 'read-only)")), "#t");
}

#[test]
fn agents_invoke_only_what_is_exposed() {
    let mut rt = runtime("abc");
    let why = refused(&mut rt, &format!("(invoke {S} 'find-file (dict 'file \"/tmp\") #:principal \"agent:test\")"));
    assert!(why.contains("not exposed to agents"), "{why}");
    let why = refused(&mut rt, &format!("(invoke {S} 'forward-char (dict))"));
    assert!(why.contains("no contract"), "{why}");
    eval(&mut rt, &format!("(invoke {S} 'set-option (dict 'name 'read-only 'value #t 'global #t) #:principal \"agent:test\")"));
    assert_eq!(eval(&mut rt, &format!("(record-principal {LAST})")), "\"agent:test\"");
    assert_eq!(eval(&mut rt, &format!("(record-effects {LAST})")), "(settings)");
    // Its view says who invoked it, with what, declaring what.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "operations");
    keys(&mut rt, "RET M-< RET");
    let shown = eval(&mut rt, &format!("(document-string (session-document {S}))"));
    assert!(shown.contains("agent:test") && shown.contains("name read-only") && shown.contains("settings"), "{shown}");
    eval(&mut rt, "(unset-option! 'read-only)");
}

#[test]
fn sensitive_arguments_stay_out_of_what_is_recorded_and_written() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-command (log-in s user password) \"Fail to log in.\"
           #:arguments '((user string) (password string #:sensitive))
           (error \"bad password for\" user password))",
    );
    // Valid, and failing: the error raised, the record, its condition.
    let why = refused(&mut rt, &format!("(invoke {S} 'log-in (dict 'user \"me\" 'password \"hunter2\"))"));
    assert!(why.contains("bad password for") && !why.contains("hunter2"), "{why}");
    let record = eval(&mut rt, &format!("(list (record-arguments {LAST}) (record-condition {LAST}))"));
    assert!(record.contains("<redacted>") && !record.contains("hunter2"), "{record}");
    // Invalid: the schema's explanation names it, not its value.
    let why = refused(&mut rt, &format!("(invoke {S} 'log-in (dict 'user \"me\" 'password 'swordfish))"));
    assert!(why.contains("password: expected a string") && !why.contains("swordfish"), "{why}");
}

#[test]
fn effects_declared_and_effects_had() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-command (sneaky s text) \"Say it changes nothing, and change the text.\"
           #:arguments '((text string))
           #:effects '()
           (insert-text! s text \"new\"))
         (define-command (honest s text) \"Insert TEXT.\"
           #:arguments '((text string))
           #:effects '(edit)
           #:available (lambda (s) (if (option (current-buffer s) 'read-only) \"the buffer is read-only\" #t))
           (insert-text! s text \"new\"))",
    );
    eval(&mut rt, &format!("(invoke {S} 'sneaky (dict 'text \"x\"))"));
    assert!(eval(&mut rt, &format!("(record-violation {LAST})")).contains("does not declare: edit"));
    assert!(eval(&mut rt, &format!("(take-violations! {S})")).contains("sneaky: it had effects it does not declare: edit"));
    eval(&mut rt, &format!("(invoke {S} 'honest (dict 'text \"y\"))"));
    assert_eq!(eval(&mut rt, &format!("(record-violation {LAST})")), "#f");
    // Not available: refused, and said in M-x.
    eval(&mut rt, "(set-option! 'read-only #t)");
    let why = refused(&mut rt, &format!("(invoke {S} 'honest (dict 'text \"z\"))"));
    assert!(why.contains("honest is not available: the buffer is read-only"), "{why}");
    keys(&mut rt, "M-x");
    type_text(&mut rt, "honest");
    let s = rt.snapshot();
    let rows: Vec<String> = s.minibuffer.as_ref().unwrap().rows.iter().map(|r| format!("{r:?}")).collect();
    assert!(rows.iter().any(|r| r.contains("unavailable: the buffer is read-only")), "{rows:?}");
    keys(&mut rt, "C-g");
    eval(&mut rt, "(unset-option! 'read-only)");
}

#[test]
fn a_shell_command_on_a_region_declares_what_its_arguments_make_it_do() {
    let mut rt = runtime("hello\n");
    let region = "(dict 'buffer \"*scratch*\" 'from 0 'to 5)";
    eval(&mut rt, &format!("(invoke {S} 'shell-command-on-region (dict 'command \"tr a-z A-Z\" 'region {region}))"));
    assert_eq!(eval(&mut rt, "(record-effects (car (traced-operation-records)))"), "(external)");
    // Ended before the next, which is then the last kept.
    until(&mut rt, "(length (pending-operation-records))", |v| v == "0");
    eval(&mut rt, &format!("(invoke {S} 'shell-command-on-region (dict 'command \"tr a-z A-Z\" 'region {region} 'replace #t))"));
    until(&mut rt, &format!("(document-string (session-document {S}))"), |v| v == "\"HELLO\\n\"");
    // Kept once it ends, its output in.
    until(&mut rt, "(length (pending-operation-records))", |v| v == "0");
    assert_eq!(eval(&mut rt, &format!("(record-effects {LAST})")), "(external edit)");
    // The region as data, as the record keeps it.
    assert!(eval(&mut rt, &format!("(record-arguments {LAST})")).contains("\"buffer\" \"*scratch*\""));
}

/// The contracts agents are offered, as JSON: kept as a fixture, which
/// TECHNE_UPDATE_FIXTURES=1 writes again.
#[test]
fn exposed_contracts_as_json_schema() {
    let mut rt = runtime("");
    let json = eval(&mut rt, "(json->string (list->vector (exposed-command-schemas)))");
    let json: String = serde_json::from_str(&json).unwrap();
    let pretty = format!("{}\n", serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&json).unwrap()).unwrap());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exposed-commands.json");
    if std::env::var_os("TECHNE_UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, &pretty).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), pretty, "refresh with TECHNE_UPDATE_FIXTURES=1");
}

#[test]
fn what_is_recorded_is_bounded_data_and_never_a_secret() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-command (keep s text places at) \"Keep TEXT, PLACES and AT.\"
           #:arguments '((text string) (places (list-of location)) (at buffer #:sensitive))
           #:result 'string
           #:expose '(agents)
           (if (string=? text \"leak\") at 3))",
    );
    // A megabyte of text, kept as its first characters and its length;
    // locations within a list as data, not live documents.
    eval(
        &mut rt,
        &format!(
            "(invoke {S} 'keep (dict 'text (make-string 1000000 #\\x) 'places (list (location (session-document {S}) 1)) 'at \"*scratch*\"))"
        ),
    );
    let kept = eval(&mut rt, &format!("(record-arguments {LAST})"));
    assert!(kept.len() < 2000 && kept.contains("1000000 characters"), "{}", &kept[..kept.len().min(300)]);
    assert!(kept.contains("\"position\" 1") && !kept.contains("#<"), "{kept}");
    assert!(eval(&mut rt, "(cadr (operation-logs-bytes))").parse::<usize>().unwrap() < 10_000);
    // A result not as declared fails the operation; a secret in it stays out.
    let why =
        refused(&mut rt, &format!("(invoke {S} 'keep (dict 'text \"leak\" 'places '() 'at \"*scratch*\") #:principal \"agent:test\")"));
    assert!(why.contains("not what its contract says") && !why.contains("scratch"), "{why}");
    assert!(eval(&mut rt, &format!("(record-condition {LAST})")).contains("not what its contract says"));
    let why = refused(&mut rt, &format!("(invoke {S} 'keep (dict 'text \"x\" 'places '() 'at \"*scratch*\") #:principal \"agent:test\")"));
    assert!(why.contains("expected a string"), "{why}");
    // A sensitive reference naming nothing: refused without saying it.
    let why = refused(&mut rt, &format!("(invoke {S} 'keep (dict 'text \"x\" 'places '() 'at \"secret-buffer\"))"));
    assert!(why.contains("at: expected a buffer") && !why.contains("secret-buffer"), "{why}");
}

#[test]
fn defaults_are_checked_and_references_keep_their_revision() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-command (count-to s n) \"Count to N.\" #:arguments '((n integer #:default \"oops\")) n)
         (define-command (mark s at) \"Say where AT is.\" #:arguments '((at location)) (location-position at))",
    );
    let why = refused(&mut rt, &format!("(invoke {S} 'count-to (dict))"));
    assert!(why.contains("n: expected an integer"), "{why}");
    // A location as data, read back after the text changed: mapped.
    eval(&mut rt, &format!("(invoke {S} 'mark (dict 'at (location (session-document {S}) 2)))"));
    let data = eval(&mut rt, &format!("(cdr (assq 'at (record-arguments {LAST})))"));
    assert!(data.contains("\"revision\" 0") && data.contains("\"position\" 2"), "{data}");
    eval(&mut rt, &format!("(insert-text! {S} \">>\" \"new\")"));
    assert_eq!(eval(&mut rt, &format!("(invoke {S} 'mark (dict 'at (cdr (assq 'at (record-arguments {LAST})))))")), "4");
}

#[test]
fn reading_conforms_each_argument_and_only_true_admits() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define seen #f)
         (define-command (pick s kind thing) \"Pick a KIND, then a THING of it.\"
           #:arguments `((kind (one-of fruit tool)) (thing ,(lambda (args) (set! seen (dict-ref args 'kind)) 'string)))
           #:read '((kind prompt) (thing prompt))
           (list kind thing))
         (define-command (never s) \"Never available.\" #:arguments '() #:available (lambda (s) #f) 'ran)",
    );
    keys(&mut rt, "M-x");
    type_text(&mut rt, "pick");
    keys(&mut rt, "RET");
    type_text(&mut rt, "fruit");
    keys(&mut rt, "RET");
    type_text(&mut rt, "apple");
    keys(&mut rt, "RET");
    assert_eq!(eval(&mut rt, "seen"), "fruit", "what comes after gets the symbol, not the string typed");
    // An invalid answer stops the reading, saying why.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "pick");
    keys(&mut rt, "RET");
    type_text(&mut rt, "fish");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().echo.contains("kind: expected one of"), "{}", rt.snapshot().echo);
    assert!(rt.snapshot().minibuffer.is_none());
    let why = refused(&mut rt, &format!("(invoke {S} 'never (dict))"));
    assert!(why.contains("never is not available: not here"), "{why}");
}

#[test]
fn settings_and_processes_are_effects_had() {
    let mut rt = runtime("hello\n");
    eval(
        &mut rt,
        "(define-command (quietly s) \"Change a setting, declaring nothing.\"
           #:arguments '() #:effects '()
           (set-option! 'read-only #f))",
    );
    eval(&mut rt, &format!("(invoke {S} 'quietly (dict))"));
    assert!(eval(&mut rt, &format!("(record-violation {LAST})")).contains("settings"));
    // A shell command holds its operation until its output is in: the
    // region it replaces is its change.
    eval(
        &mut rt,
        &format!(
            "(invoke {S} 'shell-command-on-region (dict 'command \"tr a-z A-Z\" 'region (dict 'buffer \"*scratch*\" 'from 0 'to 5) 'replace #t))"
        ),
    );
    let serial = eval(&mut rt, "(operation-serial (record-operation (car (traced-operation-records))))");
    until(&mut rt, &format!("(document-string (session-document {S}))"), |v| v == "\"HELLO\\n\"");
    let d = format!("(session-document {S})");
    assert_eq!(eval(&mut rt, &format!("(cadr (document-operation {d} (document-revision {d})))")), serial);
    until(&mut rt, "(length (pending-operation-records))", |v| v == "0");
    let effects = eval(&mut rt, &format!("(list (record-effects {LAST}) (record-violation {LAST}))"));
    assert_eq!(effects, "((external edit) #f)");
}
