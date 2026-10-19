//! techne-vm: `techne run` and `techne repl` on their own (see `cli`).

fn main() -> std::process::ExitCode {
    // A world past its memory ceiling ends, not the process.
    techne_vm::alloc::contain_out_of_memory();
    techne_vm::cli::main(std::env::args().skip(1).collect())
}
