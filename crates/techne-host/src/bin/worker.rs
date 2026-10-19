//! A worker for the tests: `techne-worker --fd N [--unsandboxed]`, as
//! `techne cloud worker` is; with TECHNE_TEST_SPIN set, deployments'
//! worlds have `(%spin)`, a native that never returns nor looks for stops;
//! with TECHNE_TEST_LIE set, it says it is ready, then answers a request
//! it was never sent, as a compromised worker might.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fd = args.iter().position(|a| a == "--fd").and_then(|i| args.get(i + 1)).and_then(|f| f.parse().ok());
    let Some(fd) = fd else {
        eprintln!("usage: techne-worker --fd N [--unsandboxed]");
        return std::process::ExitCode::from(2);
    };
    if std::env::var_os("TECHNE_TEST_LIE").is_some() {
        return lie(fd);
    }
    if std::env::var_os("TECHNE_TEST_SPIN").is_some() {
        techne_host::set_world_setup(|vm| {
            vm.register_fn("%spin", || -> i64 {
                loop {
                    std::hint::spin_loop();
                }
            })
        });
    }
    techne_host::worker::main(fd, !args.iter().any(|a| a == "--unsandboxed"))
}

fn lie(fd: i32) -> std::process::ExitCode {
    use std::os::fd::FromRawFd;
    use techne_host::wire::{FromWorker, write_frame};
    // SAFETY: the descriptor the supervisor gave.
    let mut socket = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    let _ = std::io::Read::read_exact(&mut socket, &mut [0]);
    socket.set_nonblocking(true).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let mut s = tokio::net::UnixStream::from_std(socket).unwrap();
        write_frame(&mut s, &FromWorker::Ready.to_data()).await.unwrap();
        // After a request has had time to be sent.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let lie = FromWorker::Response { rid: 999, response: techne_vm::data::Data::Bool(true), streams: false };
        let _ = write_frame(&mut s, &lie.to_data()).await;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    std::process::ExitCode::SUCCESS
}
