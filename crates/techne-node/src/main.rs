//! techne-node: `techne node` on its own (see `cli`).

fn main() -> std::process::ExitCode {
    techne_node::cli::main(&["techne-node"], std::env::args().skip(1).collect())
}
