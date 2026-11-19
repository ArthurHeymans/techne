//! Load the Scheme terminal benchmark with real editor document primitives,
//! but without a frontend, a PTY, or an attached editor session.
use std::{io::Write, path::PathBuf};
use techne_vm::{api::Value, vm::Vm};

const CASES: [&str; 4] = ["plain", "sgr", "tui", "checker"];

fn eval(vm: &mut Vm, source: &str) -> std::io::Result<Value> {
    vm.eval_source(source).map_err(|e| std::io::Error::other(e.to_string()))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--help") {
        println!("terminal-bench [BYTES [ITERATIONS [TSV]]] | --fixtures DIR [BYTES]");
        return Ok(());
    }
    let fixtures = first.as_deref() == Some("--fixtures");
    let dir = if fixtures { Some(PathBuf::from(args.next().ok_or("expected a fixture directory")?)) } else { None };
    let bytes = (if fixtures { args.next() } else { first }).map(|n| n.parse::<usize>()).transpose()?.unwrap_or(262_144);
    let iterations = if fixtures { 5 } else { args.next().map(|n| n.parse::<usize>()).transpose()?.unwrap_or(5) };
    let out = if fixtures { None } else { args.next() };
    if bytes == 0 || iterations == 0 || args.next().is_some() {
        return Err("expected positive BYTES and ITERATIONS, and at most one output path".into());
    }

    let mut vm = Vm::new();
    techne_editor::install(&mut vm);
    let source = techne_vm::library::lisp_dir().join("bench/terminal.scm");
    eval(&mut vm, &format!("(require {:?})", source.display().to_string()))?;
    if let Some(dir) = dir {
        for name in CASES {
            let path = dir.join(name);
            eval(
                &mut vm,
                &format!(
                    "(call-with-output-file {:?} (lambda (p) (display (terminal-bench-data '{name} {bytes}) p)))",
                    path.display().to_string()
                ),
            )?;
        }
        return Ok(());
    }
    let mut output: Box<dyn Write> =
        if let Some(path) = out { Box::new(std::fs::File::create(path)?) } else { Box::new(std::io::stdout().lock()) };
    eprintln!("Techne terminal: {bytes} bytes, {iterations} iterations, 80x24");
    for name in CASES {
        eval(&mut vm, &format!("(define bench-data (terminal-bench-data '{name} {bytes}))"))?;
        let step = "(terminal-bench-iteration bench-data)";
        eval(&mut vm, step)?;
        let mut times = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            vm.collect();
            let val = eval(&mut vm, step)?;
            times.push(vm.repr(&val).parse::<f64>()?);
        }
        times.sort_by(f64::total_cmp);
        // Match Ebb: for an even count select the upper middle sample.
        let ms = times[times.len() / 2];
        writeln!(output, "techne/{name}\t{ms:.3}\t{:.3}", bytes as f64 / 1_048_576.0 / (ms / 1000.0))?;
    }
    Ok(())
}
