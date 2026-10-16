//! techne-vm: `techne run` and `techne repl` on their own (see `cli`).

fn main() -> std::process::ExitCode {
    techne_vm::cli::main(std::env::args().skip(1).collect())
}
