//! techne-lsp: `techne lsp` on its own, for editors set up to run it.

fn main() -> std::process::ExitCode {
    // A world past its memory ceiling ends, not the process.
    techne_vm::alloc::contain_out_of_memory();
    techne_lsp::main()
}
