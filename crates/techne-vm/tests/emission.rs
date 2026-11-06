//! Native emission must not depend on the process's hash seed.
//!
//! The frontend writes blocks, guard checks and exits in map iteration order.
//! With a randomized hasher, two runs of one binary compiled the same
//! function to different block orders and, when the order changed block
//! layout, to different machine code (observed: `partition` of the qsort
//! workload, two layouts in six runs). This test compiles one function in
//! several fresh processes and requires identical disassembly once 64-bit
//! immediate addresses are masked.
//!
//! `TECHNE_JIT_ASM` prints the finalized function, so this covers the backend
//! too; a frontend-only check would not catch an order that changes layout.
use std::{fs, process::Command};

/// Eight runs: a two-way layout lottery survives one run with probability
/// 2^-7, and a run is a few tens of milliseconds.
const RUNS: usize = 8;

const SOURCE: &str = r#"
;; A loop, an inlined helper with four guarded vector accesses, and recursive
;; calls: the shape whose block order the process-seeded map lottery changed.
(define n 300)
(define v (make-vector n 0))
(let seed ((i 0) (x 42))
  (when (< i n) (vector-set! v i x) (seed (+ i 1) (modulo (+ (* x 75) 74) 65537))))
(define (swap! i j) (let ((t (vector-ref v i))) (vector-set! v i (vector-ref v j)) (vector-set! v j t)))
(define (partition lo hi)
  (let ((pivot (vector-ref v hi)))
    (let loop ((i lo) (j lo))
      (if (< j hi)
          (if (< (vector-ref v j) pivot)
              (begin (swap! i j) (loop (+ i 1) (+ j 1)))
              (loop i (+ j 1)))
          (begin (swap! i hi) i)))))
(define (qs lo hi) (when (< lo hi) (let ((p (partition lo hi))) (qs lo (- p 1)) (qs (+ p 1) hi))))
(qs 0 (- n 1))
(display (vector-ref v 0)) (newline)
"#;

/// The `partition` disassembly with `movabsq`'s 64-bit immediate masked.
///
/// Code, constant-pool and helper addresses are materialized by `movabsq`, and
/// they are the tokens that differ between two runs of one binary. Everything
/// else is kept: `movl $0xffffffff` and other 32-bit immediates, stack
/// displacements, `;`-separated branch pairs such as `jnz label2; j label1`,
/// and Cranelift's unwinding directives. A `movabsq` immediate can also be a
/// double's bit pattern (a NaN mask, say), which is masked with the addresses;
/// block and instruction order remain fully checked.
fn disassembly(path: &std::path::Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
        .arg(path)
        .env("TECHNE_JIT", "1000")
        .env("TECHNE_JIT_SYNC", "1")
        .env("TECHNE_JIT_ASM", "partition")
        .env_remove("TECHNE_JIT_IR")
        .env_remove("TECHNE_JIT_LOG")
        .env_remove("TECHNE_JIT_STATS")
        .env_remove("TECHNE_GC_STRESS")
        .env_remove("TECHNE_GC_STATS")
        .output()
        .expect("run VM");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "partition failed: {stderr}");
    let body: String = stderr
        .lines()
        .skip_while(|line| !line.starts_with("asm: partition"))
        .skip(1)
        // Block labels sit at column 0 and an indented line is not a boundary;
        // another function's dump or a log line is.
        .take_while(|line| {
            !(line.starts_with("asm: ")
                || line.starts_with("jit")
                || line.starts_with("thread")
                || line.starts_with("error")
                || line.starts_with("note"))
        })
        .map(|line| format!("{line}\n"))
        .collect();
    let masked: String = body
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("movabsq") {
                let mut out = String::with_capacity(line.len());
                let mut rest = line;
                while let Some(at) = rest.find("$0x") {
                    let tail = &rest[at + 3..];
                    let end = tail.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(tail.len());
                    out.push_str(&rest[..at]);
                    out.push_str(if end >= 8 { "$ADDR" } else { &rest[at..at + 3 + end] });
                    rest = &tail[end..];
                }
                out.push_str(rest);
                out.push('\n');
                out
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    let instructions = masked.lines().filter(|line| line.starts_with("  ") && !line.contains("unwind")).count();
    assert!(instructions > 100, "no usable `partition` disassembly ({instructions} instructions):\n{stderr}");
    masked
}

#[test]
fn native_emission_is_identical_across_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("emission.scm");
    fs::write(&path, SOURCE).unwrap();
    let first = disassembly(&path);
    for run in 1..RUNS {
        let later = disassembly(&path);
        assert_eq!(first, later, "run {run} emitted a different layout");
    }
}
