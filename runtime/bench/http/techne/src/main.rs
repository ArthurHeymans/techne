//! techne-vm as an HTTP server. Rust gives Lisp four async TCP natives
//! (`tcp-listen`, `tcp-accept`, `tcp-read`, `tcp-write`, `tcp-close`); the
//! server itself, one task per connection, is `server.scm`.
//!
//! `THREADS` VMs run on threads of their own, each with its own listener on
//! the port (SO_REUSEPORT, the kernel spreads connections). `IO` picks where
//! the sockets' readiness is polled:
//!
//! - `inline` (default): each VM thread drives a current-thread tokio runtime
//!   and calls `run_tasks_for` when it is blocked, as a host event loop does
//!   (`set_wake_notifier`). One thread per VM, no cross-thread wakeups.
//! - `thread`: one shared multi-thread tokio runtime (`IO_WORKERS` workers,
//!   default 1) polls the sockets; VMs run `run_tasks` and park until a
//!   future's waker unparks them, as `techne-process` does.
//!
//! Usage: techne-http-bench server.scm PORT

use std::{
    cell::RefCell,
    io::ErrorKind,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use techne_vm::{
    api::{Bytes, Foreign},
    tasks::Progress,
    value::Value,
    vm::{USER_MODULE, Vm},
};
use tokio::net::{TcpListener, TcpStream};

struct Listener(TcpListener);
struct Conn(RefCell<Option<Rc<TcpStream>>>);

impl Conn {
    fn stream(&self) -> Result<Rc<TcpStream>, String> {
        self.0.borrow().clone().ok_or_else(|| "connection closed".to_string())
    }
}

fn listen(port: u16) -> std::io::Result<TcpListener> {
    use socket2::{Domain, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    s.set_reuse_address(true)?;
    s.set_reuse_port(true)?;
    s.bind(&std::net::SocketAddr::from(([127, 0, 0, 1], port)).into())?;
    s.listen(4096)?;
    s.set_nonblocking(true)?;
    TcpListener::from_std(s.into())
}

thread_local! {
    /// What reads land in before they become strings.
    static SCRATCH: RefCell<Vec<u8>> = RefCell::new(vec![0; 16 * 1024]);
}

fn conn(vm: &mut Vm, v: Value) -> Result<Rc<TcpStream>, String> {
    let c: Foreign<Conn> = vm.get(v).map_err(|e| e.to_string())?;
    c.stream()
}

fn install(vm: &mut Vm) {
    vm.register_fn("tcp-listen", |port: i64| -> Result<Foreign<Listener>, String> {
        listen(port as u16).map(|l| Foreign::new(Listener(l))).map_err(|e| format!("tcp-listen: {e}"))
    });
    vm.register_fn("tcp-close", |c: Foreign<Conn>| {
        c.0.0.borrow_mut().take();
    });
    vm.register_async("tcp-accept", 1, |vm: &mut Vm, args: &[Value]| {
        let l: Result<Foreign<Listener>, String> = vm.get(args[0]).map_err(|e| e.to_string());
        async move {
            let l = l?;
            let (s, _) = l.0.0.accept().await.map_err(|e| format!("tcp-accept: {e}"))?;
            s.set_nodelay(true).map_err(|e| e.to_string())?;
            Ok::<_, String>(Foreign::new(Conn(RefCell::new(Some(Rc::new(s))))))
        }
    });
    // The next bytes received, as a string, or #f at end of stream.
    vm.register_async("tcp-read", 1, |vm: &mut Vm, args: &[Value]| {
        let s = conn(vm, args[0]);
        async move {
            let s = s?;
            loop {
                let read = SCRATCH.with_borrow_mut(|buf| {
                    s.try_read(buf).map(|n| String::from_utf8_lossy(&buf[..n]).into_owned())
                });
                match read {
                    Ok(text) if text.is_empty() => return Ok(None),
                    Ok(text) => return Ok(Some(text)),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => s.readable().await.map_err(|e| e.to_string())?,
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => return Ok(None),
                    Err(e) => return Err(format!("tcp-read: {e}")),
                }
            }
        }
    });
    vm.register_async("tcp-write", 2, |vm: &mut Vm, args: &[Value]| {
        let s = conn(vm, args[0]);
        let data: Result<Bytes, String> = vm.get(args[1]).map_err(|e| e.to_string());
        async move {
            let (s, Bytes(data)) = (s?, data?);
            let mut at = 0;
            while at < data.len() {
                match s.try_write(&data[at..]) {
                    Ok(n) => at += n,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => s.writable().await.map_err(|e| e.to_string())?,
                    Err(e) => return Err(format!("tcp-write: {e}")),
                }
            }
            Ok::<_, String>(())
        }
    });
}

fn start(vm: &mut Vm, script: &str, port: u16) {
    install(vm);
    vm.eval_file(std::path::Path::new(script)).unwrap_or_else(|e| panic!("{e}"));
    let main = vm.eval_in(USER_MODULE, "<main>", &format!("(lambda () (serve {port}))")).unwrap_or_else(|e| panic!("{e}"));
    vm.spawn(main);
}

fn run_inline(script: &str, port: u16) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut vm = Vm::new();
    let wake = Arc::new(tokio::sync::Notify::new());
    let notify = wake.clone();
    vm.set_wake_notifier(move || notify.notify_one());
    rt.block_on(async {
        start(&mut vm, script, port);
        loop {
            match vm.run_tasks_for(Duration::from_millis(2)) {
                Progress::Finished => break,
                Progress::OutOfTime => tokio::task::yield_now().await,
                Progress::Blocked => match vm.next_timer() {
                    Some(t) => {
                        tokio::select! {
                            _ = wake.notified() => {}
                            _ = tokio::time::sleep_until(t.into()) => {}
                        }
                    }
                    None => wake.notified().await,
                },
            }
        }
    });
}

fn run_threaded(script: &str, port: u16, rt: &tokio::runtime::Runtime) {
    let _io = rt.enter();
    let mut vm = Vm::new();
    start(&mut vm, script, port);
    vm.run_tasks().unwrap_or_else(|e| panic!("{e}"));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let script = args.get(1).expect("usage: techne-http-bench server.scm PORT").clone();
    let port: u16 = args.get(2).map_or(8080, |p| p.parse().expect("port"));
    let var = |name: &str, default: usize| std::env::var(name).ok().map_or(default, |v| v.parse().expect(name));
    let threads = var("THREADS", 1);
    let inline = std::env::var("IO").map_or(true, |m| m != "thread");
    let shared = (!inline).then(|| {
        Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(var("IO_WORKERS", 1))
                .enable_all()
                .build()
                .unwrap(),
        )
    });
    let vms: Vec<_> = (0..threads)
        .map(|_| {
            let (script, shared) = (script.clone(), shared.clone());
            std::thread::spawn(move || match shared {
                None => run_inline(&script, port),
                Some(rt) => run_threaded(&script, port, &rt),
            })
        })
        .collect();
    vms.into_iter().for_each(|t| t.join().unwrap());
}
