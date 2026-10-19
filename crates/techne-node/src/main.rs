//! techne-node: `techne node` on its own (see `cli`).

fn main() -> std::process::ExitCode {
    // A world past its memory ceiling ends, not the process.
    techne_vm::alloc::contain_out_of_memory();
    techne_node::cli::main(&["techne-node"], std::env::args().skip(1).collect())
}
