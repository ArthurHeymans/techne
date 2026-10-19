//! Bodies that stream, of requests and responses: chunks through a bounded
//! queue from a [`Writer`] to a [`Reader`], then the end, or a failure
//! after which nothing more comes. A writer waits while the queue is full,
//! so a body moves no faster than its reader takes it; one whose reader is
//! gone is told so.
//!
//! A request crosses from world to world as an exchange ([`Exchange`]):
//! its head as data, its body if it streams, and its response, head and
//! body likewise. A world reaches an exchange through the natives
//! [`install`] defines.

use std::{cell::RefCell, rc::Rc};

use techne_vm::{
    api::{Bytes, Foreign, Value},
    data::Data,
    vm::{Error, Vm},
};
use tokio::sync::{mpsc, oneshot};

/// The most bytes of a chunk: a write of more is cut into chunks.
pub const CHUNK: usize = 64 << 10;

/// Chunks a body's queue holds.
const QUEUE: usize = 4;

/// How a body ended: wholly, or failed for why.
type End = Rc<RefCell<Option<Result<(), String>>>>;

/// What writes a body.
pub struct Writer {
    tx: mpsc::Sender<Vec<u8>>,
    end: End,
}

/// What reads a body.
pub struct Reader {
    rx: mpsc::Receiver<Vec<u8>>,
    end: End,
}

/// A body's writer and reader.
pub fn channel() -> (Writer, Reader) {
    let (tx, rx) = mpsc::channel(QUEUE);
    let end = End::default();
    (Writer { tx, end: end.clone() }, Reader { rx, end })
}

impl Writer {
    /// Write `bytes`, waiting while the queue is full: false if the reader
    /// is gone.
    pub async fn write(&self, bytes: &[u8]) -> bool {
        for chunk in bytes.chunks(CHUNK) {
            if self.tx.send(chunk.to_vec()).await.is_err() {
                return false;
            }
        }
        true
    }

    /// End the body, with `result`: wholly, or failed for why. A writer
    /// dropped without ending its body fails it.
    pub fn end(self, result: Result<(), String>) {
        *self.end.borrow_mut() = Some(result);
    }

    /// Whether the reader is gone.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

impl Reader {
    /// The body's next chunk; `None` at its end, and after; or why it
    /// failed.
    pub async fn read(&mut self) -> Result<Option<Vec<u8>>, String> {
        match self.rx.recv().await {
            Some(chunk) => Ok(Some(chunk)),
            None => match self.end.borrow().clone() {
                Some(r) => r.map(|()| None),
                None => Err("the body was cut short".into()),
            },
        }
    }
}

/// A request and its response.
pub struct Response {
    /// The response as data, its body empty if it streams.
    pub head: Data,
    /// Its body, if it streams.
    pub body: Option<Reader>,
}

/// A request crossing from one world to another, as each world sees it:
/// what it reads (the request's body where it is answered, the response's
/// where it was asked), what it writes, and the response, to wait for or
/// to give.
#[derive(Default)]
pub struct Exchange {
    pub(crate) reader: RefCell<Option<Reader>>,
    pub(crate) writer: RefCell<Option<Writer>>,
    pub(crate) reply: RefCell<Option<oneshot::Sender<Response>>>,
    pub(crate) response: RefCell<Option<oneshot::Receiver<Response>>>,
    /// Whether the response streams, once it is given or received.
    pub(crate) streams: std::cell::Cell<bool>,
}

impl Exchange {
    /// Whether the side waiting for the response, or reading its body,
    /// is gone.
    pub(crate) fn abandoned(&self) -> bool {
        match (&*self.reply.borrow(), &*self.writer.borrow()) {
            (Some(reply), _) => reply.is_closed(),
            (None, Some(w)) => w.is_closed(),
            (None, None) => false,
        }
    }

    /// Answer with `response`, if not answered yet; else fail the body
    /// being written with `why`.
    pub(crate) fn answer(&self, response: Data, why: &str) {
        if let Some(reply) = self.reply.borrow_mut().take() {
            let _ = reply.send(Response { head: response, body: None });
        } else if let Some(w) = self.writer.borrow_mut().take() {
            w.end(Err(why.into()));
        }
    }
}

/// Define in `vm` what reaches an exchange from Lisp, an exchange a
/// foreign object (`(%host-body-read x)` and the rest below).
pub fn install(vm: &mut Vm) {
    // The next chunk of what X reads, a bytevector, or #f at its end.
    vm.register_async("%host-body-read", 1, |vm, args| {
        let x = args.get::<Foreign<Exchange>>(vm, 0);
        async move {
            let x = x?.0;
            let Some(mut reader) = x.reader.borrow_mut().take() else {
                return Err(Error::new("no body to read, or it is being read"));
            };
            let chunk = reader.read().await;
            *x.reader.borrow_mut() = Some(reader);
            chunk.map(|c| c.map(Bytes)).map_err(Error::new)
        }
    });
    // Write DATA, a string or a bytevector, to what X writes: #f if its
    // reader is gone.
    vm.register_async("%host-body-write", 2, |vm, args| {
        let x = args.get::<Foreign<Exchange>>(vm, 0);
        let data = args.get::<Bytes>(vm, 1);
        async move {
            let (x, data) = (x?.0, data?.0);
            // A sender of its own: the writer stays with the exchange,
            // to be ended there.
            let tx = x.writer.borrow().as_ref().map(|w| w.tx.clone()).ok_or_else(|| Error::new("no body to write"))?;
            for chunk in data.chunks(CHUNK) {
                if tx.send(chunk.to_vec()).await.is_err() {
                    return Ok(false);
                }
            }
            Ok::<_, Error>(true)
        }
    });
    techne_vm::procedures! { vm;
        // End what X writes; with WHY, failed.
        "(%host-body-end x why)" => |x: Foreign<Exchange>, why: Option<String>| -> Result<(), Error> {
            if let Some(w) = x.0.writer.borrow_mut().take() {
                w.end(why.map_or(Ok(()), Err));
            }
            Ok(())
        };
        // Whether the response X received streams.
        "(%host-body-streams? x)" => |x: Foreign<Exchange>| -> Result<bool, Error> {
            Ok(x.0.streams.get())
        };
        // Answer X with RESPONSE, its body streaming from what X writes.
        #[vm]
        "(%host-respond x response)" => |vm: &mut Vm, x: Foreign<Exchange>, response: Value| -> Result<(), Error> {
            let reply = x.0.reply.borrow_mut().take().ok_or_else(|| Error::new("answered already"))?;
            let head = vm.to_data(&response, crate::REQUEST_BYTES)?;
            let (writer, reader) = channel();
            *x.0.writer.borrow_mut() = Some(writer);
            x.0.streams.set(true);
            let _ = reply.send(Response { head, body: Some(reader) });
            Ok(())
        };
    }
    // The response X waits for, its body empty if it streams.
    vm.register_async("%host-response", 1, |vm, args| {
        let x = args.get::<Foreign<Exchange>>(vm, 0);
        async move {
            let x = x?.0;
            let rx = x.response.borrow_mut().take().ok_or_else(|| Error::new("no response to wait for"))?;
            let response = rx.await.map_err(|_| Error::new("host-fetch: the host stopped"))?;
            x.streams.set(response.body.is_some());
            *x.reader.borrow_mut() = response.body;
            Ok::<_, Error>(DataValue(response.head))
        }
    });
}

/// Data, as a value of the world it is given to.
struct DataValue(Data);

impl techne_vm::api::IntoValue for DataValue {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        vm.from_data(&self.0)
    }
}

/// `host-fetch` in a front world, through `%host-exchange`: a request
/// whose body is a procedure (`lisp/http`'s, read as it is called) has it
/// read by a task of its own and written to the exchange, as far as the
/// deployment reads it; a response whose body streams is given a
/// procedure writing it.
const FRONT: &str = r#"
(define (host-fetch deployment request)
  (let* ((body (dict-ref request 'body #f))
         (streams (procedure? body))
         (x (%host-exchange deployment (if streams (dict-set request 'body (bytevector)) request) streams)))
    (when streams
      (spawn (lambda ()
               (guard (e (#t (%host-body-end x "the request's body could not be read")))
                 (let loop ()
                   (let ((b (body)))
                     (cond ((eof-object? b) (%host-body-end x #f))
                           ((%host-body-write x b) (loop)))))))))
    (let ((response (%host-response x)))
      (if (%host-body-streams? x)
          (dict-set response 'body
                    (lambda (write)
                      (let loop ()
                        (let ((b (%host-body-read x)))
                          (when b (write b) (loop))))))
          response))))
"#;

/// Define `host-fetch` in `front` ([`crate::Host::install`]), asking
/// `fetch` for each response.
pub(crate) fn install_front(front: &mut Vm, fetch: impl Fn(&str, Data, Option<Reader>) -> oneshot::Receiver<Response> + 'static) {
    install(front);
    front.register_fn_vm(
        "%host-exchange",
        move |vm: &mut Vm, deployment: String, request: Value, streams: bool| -> Result<Foreign<Exchange>, Error> {
            let request = vm.to_data(&request, crate::REQUEST_BYTES)?;
            let x = Exchange::default();
            let body = streams.then(|| {
                let (writer, reader) = channel();
                *x.writer.borrow_mut() = Some(writer);
                reader
            });
            *x.response.borrow_mut() = Some(fetch(&deployment, request, body));
            Ok(Foreign::new(x))
        },
    );
    front.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-host>", FRONT).expect("host-fetch compiles");
}

/// A body as it crosses a socket: a chunk, or its end, failed if with why.
pub(crate) enum Piece {
    Bytes(Vec<u8>),
    End(Option<String>),
}

/// Write the pieces arriving on `pieces` to `writer`, calling `credit`
/// for each chunk written, through the body's end: false if the reader
/// went away first. Pieces that stop without an end cut the body short.
pub(crate) async fn relay(mut pieces: mpsc::Receiver<Piece>, writer: Writer, credit: impl Fn()) -> bool {
    while let Some(piece) = pieces.recv().await {
        match piece {
            Piece::Bytes(b) => {
                if !writer.write(&b).await {
                    return false;
                }
                credit();
            }
            Piece::End(error) => {
                writer.end(error.map_or(Ok(()), Err));
                return true;
            }
        }
    }
    true
}

/// Send what `reader` reads with `send`, a chunk for each of `credits`,
/// and its end without one, until `send` fails or the credits are closed.
pub(crate) async fn pump(mut reader: Reader, credits: Rc<tokio::sync::Semaphore>, send: impl Fn(Piece) -> bool) {
    loop {
        let piece = match reader.read().await {
            Ok(Some(b)) => match credits.acquire().await {
                Ok(permit) => {
                    permit.forget();
                    Piece::Bytes(b)
                }
                Err(_) => return,
            },
            Ok(None) => Piece::End(None),
            Err(e) => Piece::End(Some(e)),
        };
        let end = matches!(piece, Piece::End(_));
        if !send(piece) || end {
            return;
        }
    }
}
