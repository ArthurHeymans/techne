//! A line-oriented REPL: reads until the input forms are balanced, evaluates
//! them in the user module and prints non-void results.
//!
//! Errors that have restarts available enter a small debugger at the raise
//! point: choose a restart by number (followed by argument expressions, e.g.
//! `1 42`) or `0` to abort.
use std::io::{BufRead, Write};

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

pub fn run(vm: &mut Vm) {
    vm.register_fn_vm("%repl-debugger", debugger);
    let handler = vm.get_global("%repl-debugger").expect("debugger registered");
    vm.push_handler(handler);
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
        if let Err(e) = reader::read(&buffer)
            && (e.contains("EOF") || e.contains("end of input"))
        {
            continue;
        }
        let source = std::mem::take(&mut buffer);
        match vm.eval_source(&source) {
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
}
