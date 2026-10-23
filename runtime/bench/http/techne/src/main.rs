//! techne-vm as an HTTP server: `server.scm`, one task per connection,
//! over techne-net's TCP procedures.
//!
//! `THREADS` VMs run on threads of their own, each with its own listener on
//! the port (`#:reuse-port`, the kernel spreads connections). `IO` picks
//! where the sockets' readiness is polled:
//!
//! - `inline` (default): each VM thread has a current-thread tokio runtime,
//!   which it drives with `techne_net::run_tasks` (`run_tasks_for` when
//!   woken, as a host event loop does). One thread per VM, no cross-thread
//!   wakeups.
//! - `thread`: techne-net's shared runtime polls every VM's sockets on one
//!   I/O thread; VMs run `run_tasks` and park until a future's waker
//!   unparks them, as `techne-process` does.
//!
//! Usage: techne-http-bench server.scm PORT

use techne_vm::vm::{USER_MODULE, Vm};

fn start(vm: &mut Vm, script: &str, port: u16) {
    vm.eval_file(std::path::Path::new(script)).unwrap_or_else(|e| panic!("{e}"));
    let main = vm.eval_in(USER_MODULE, "<main>", &format!("(lambda () (serve {port}))")).unwrap_or_else(|e| panic!("{e}"));
    vm.spawn(&main);
}

fn run_inline(script: &str, port: u16) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut vm = Vm::new();
    techne_net::install_on(&mut vm, rt.handle().clone()).unwrap_or_else(|e| panic!("{e}"));
    start(&mut vm, script, port);
    rt.block_on(techne_net::run_tasks(&mut vm)).unwrap_or_else(|e| panic!("{e}"));
}

fn run_threaded(script: &str, port: u16) {
    let mut vm = Vm::new();
    techne_net::install(&mut vm).unwrap_or_else(|e| panic!("{e}"));
    start(&mut vm, script, port);
    vm.run_tasks().unwrap_or_else(|e| panic!("{e}"));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let script = args.get(1).expect("usage: techne-http-bench server.scm PORT").clone();
    let port: u16 = args.get(2).map_or(8080, |p| p.parse().expect("port"));
    let threads: usize = std::env::var("THREADS").map_or(1, |v| v.parse().expect("THREADS"));
    let inline = std::env::var("IO").map_or(true, |m| m != "thread");
    let vms: Vec<_> = (0..threads)
        .map(|_| {
            let script = script.clone();
            std::thread::spawn(move || if inline { run_inline(&script, port) } else { run_threaded(&script, port) })
        })
        .collect();
    vms.into_iter().for_each(|t| t.join().unwrap());
}
