//! The REPL: reads until the input forms are complete, evaluates them in the
//! user module and prints non-void results. On a terminal it offers line
//! editing, history (`~/.techne_history`) and completion of global names.
//!
//! Errors that have restarts available enter a small debugger at the raise
//! point: choose a restart by number (followed by argument expressions, e.g.
//! `1 42`) or `0` to abort.
use std::{
    cell::RefCell,
    io::{BufRead, IsTerminal, Write},
    rc::Rc,
};

use rustyline::{
    Context, Editor, Helper,
    completion::{Completer, Pair},
    highlight::Highlighter,
    hint::Hinter,
    history::DefaultHistory,
    validate::{ValidationContext, ValidationResult, Validator},
};

use crate::{
    builtins::{condition_message, list_values, repr},
    reader,
    value::Value,
    vm::{Error, Vm},
};

fn read_line() -> Option<String> {
    let mut line = String::new();
    (std::io::stdin().lock().read_line(&mut line).unwrap_or(0) > 0).then_some(line)
}

/// Handler run at the raise point: offers the active restarts.
fn debugger(vm: &mut Vm, condition: Value) -> Result<Value, Error> {
    let condition = vm.root(condition);
    let restarts = vm.call_global("compute-restarts", &[])?;
    let restarts = list_values(restarts).unwrap_or_default();
    if restarts.is_empty() {
        return Err(vm.raise_error(condition.get()));
    }
    let restarts: Vec<_> = restarts.into_iter().map(|r| vm.root(r)).collect();
    vm.flush();
    eprintln!("error: {}", condition_message(vm, condition.get()));
    eprintln!("restarts:");
    for (i, r) in restarts.iter().enumerate() {
        let name = vm.call_global("restart-name", &[r.get()])?;
        eprintln!("  [{}] {}", i + 1, repr(name));
    }
    eprintln!("  [0] abort");
    loop {
        eprint!("debug> ");
        let Some(line) = read_line() else { return Err(vm.raise_error(condition.get())) };
        let line = line.trim();
        let (choice, args) = line.split_once(' ').unwrap_or((line, ""));
        match choice.parse::<usize>() {
            Ok(0) => return Err(vm.raise_error(condition.get())),
            Ok(k) if k <= restarts.len() => {
                let mut values = vec![restarts[k - 1].get()];
                for form in reader::read(args).map_err(Error::new)? {
                    let v = vm.eval_sexp(&form)?;
                    values.push(v);
                }
                let values: Vec<_> = values.into_iter().map(|v| vm.root(v)).collect();
                let args: Vec<Value> = values.iter().map(|r| r.get()).collect();
                // Escapes to the restart-case; does not return normally.
                return vm.call_global("invoke-restart", &args);
            }
            _ => eprintln!("choose 0-{}", restarts.len()),
        }
    }
}

fn incomplete(source: &str) -> bool {
    matches!(reader::read(source), Err(e) if e.contains("EOF") || e.contains("end of input"))
}

fn eval_print(vm: &mut Vm, source: &str) {
    match vm.eval_source(source) {
        Ok(v) => {
            vm.flush();
            if v != Value::VOID {
                println!("{}", repr(v));
            }
        }
        Err(e) => {
            vm.flush();
            eprintln!("{e}");
        }
    }
}

struct LispHelper {
    names: Rc<RefCell<Vec<Rc<str>>>>,
}

fn identifier_char(c: char) -> bool {
    !c.is_whitespace() && !"()[]{}'\"`,;".contains(c)
}

impl Completer for LispHelper {
    type Candidate = Pair;
    fn complete(&self, line: &str, pos: usize, _: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        let start = line[..pos].rfind(|c: char| !identifier_char(c)).map_or(0, |i| i + 1);
        let word = &line[start..pos];
        if word.is_empty() {
            return Ok((pos, vec![]));
        }
        let names = self.names.borrow();
        let matches = names
            .iter()
            .filter(|n| n.starts_with(word))
            .map(|n| Pair { display: n.to_string(), replacement: n.to_string() })
            .collect();
        Ok((start, matches))
    }
}

impl Hinter for LispHelper {
    type Hint = String;
}
impl Highlighter for LispHelper {}
impl Validator for LispHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        Ok(if incomplete(ctx.input()) { ValidationResult::Incomplete } else { ValidationResult::Valid(None) })
    }
}
impl Helper for LispHelper {}

fn run_editor(vm: &mut Vm) -> rustyline::Result<()> {
    let names = Rc::new(RefCell::new(vm.global_names()));
    let mut editor: Editor<LispHelper, DefaultHistory> = Editor::new()?;
    editor.set_helper(Some(LispHelper { names: names.clone() }));
    let history = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".techne_history"));
    if let Some(h) = &history {
        let _ = editor.load_history(h);
    }
    loop {
        match editor.readline("λ> ") {
            Ok(source) => {
                if source.trim().is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(source.as_str());
                eval_print(vm, &source);
                *names.borrow_mut() = vm.global_names();
            }
            Err(rustyline::error::ReadlineError::Interrupted) => continue,
            Err(_) => break,
        }
    }
    if let Some(h) = &history {
        let _ = editor.save_history(h);
    }
    Ok(())
}

pub fn run(vm: &mut Vm) {
    vm.register_fn_vm("%repl-debugger", debugger);
    let handler = vm.get_global("%repl-debugger").expect("debugger registered");
    vm.push_handler(handler);
    if std::io::stdin().is_terminal() && run_editor(vm).is_ok() {
        return;
    }
    let mut buffer = String::new();
    loop {
        print!("{}", if buffer.is_empty() { "λ> " } else { ".. " });
        let _ = std::io::stdout().flush();
        let Some(line) = read_line() else {
            println!();
            return;
        };
        buffer.push_str(&line);
        // Incomplete input: keep reading.
        if incomplete(&buffer) {
            continue;
        }
        let source = std::mem::take(&mut buffer);
        eval_print(vm, &source);
    }
}
