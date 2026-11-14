//! Frontends in another process (EDITOR.md, section 6, "Transport"): a
//! frontend attaches over the editor's socket (`server`) as one in the
//! runtime's process does over a channel, with a session of its own, and
//! is sent the same outputs. `techne window --attach` opens a window on
//! the editor running, as `emacsclient --create-frame` opens a frame.
//!
//! The connection carries the presentation protocol (`present`)
//! serialized, framed as the socket's other requests are. Most of a
//! snapshot is sent as it is; what is not:
//!
//! - a view's text is sent as what changed since the text that view was
//!   last sent (the bytes the two share at the start and at the end left
//!   out), so typing in a large file sends a few bytes, not the file;
//! - an image's pixels are sent when it is shown and was not shown in the
//!   snapshot sent before, then the image by its id: both ends keep the
//!   pixels of the images the last snapshot shows, no more;
//! - a snapshot waiting to be sent behind a newer one is dropped (the newer
//!   one shows all it does), unless it asks the frontend for something (a
//!   pane's `request`): the frontend draws what is current, however slow
//!   it is, and what waits for it stays bounded; a frontend that lets
//!   `MAX_QUEUED` outputs wait anyway is disconnected;
//! - instants are the frontend's own: it numbers its keys and clicks, and
//!   a snapshot says up to which one it answers, for the frontend to say
//!   when the runtime is busy and to measure latency.
//!
//! C-g works as it does in the runtime's process: the server's side of the
//! attachment sends it on as it comes, ahead of what the runtime is doing.
//! A frontend whose connection ends is detached; one whose runtime
//! crashed is attached to the runtime started instead, as the others are,
//! and is told so (`Down::Restarted`): what it sent the old one goes
//! unanswered, and its captures for the old one are void. A frontend
//! chooses where it is attached with a `Link`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use techne_text::ropey::Rope;

use crate::{
    host::{BUSY_AFTER, Event, Host, Shared},
    images::{Image, ImageData},
    present::{CapturedFrame, Input, Output, Snapshot},
    runtime::{File, journal_for},
    server::{OpenError, Request},
};

/// Largest frame of an attachment: an image's pixels and a captured frame
/// cross it whole. The peer runs as the same user, as the socket's
/// directory is private: this bounds mistakes, not an adversary.
const MAX_FRAME: usize = 256 << 20;

/// The most outputs waiting to be sent to a frontend: past them, it is not
/// reading what it is sent, and is disconnected rather than have the
/// editor keep them. Snapshots replacing one another, few wait.
const MAX_QUEUED: usize = 4096;

/// What a frontend sends.
#[derive(Serialize, Deserialize)]
enum Up {
    Input(Input),
    /// A capture's result, for the runtime of `generation`: the number of
    /// `Restarted` the frontend was told before it.
    Captured {
        generation: u64,
        id: usize,
        result: Result<CapturedFrame, String>,
    },
}

/// What a frontend is sent.
#[derive(Serialize, Deserialize)]
enum Down {
    /// A snapshot, its panes' texts left out: `texts` brings those of the
    /// views it names up to date, the others are as last sent. `images`
    /// are the pixels of the images it shows that the snapshot sent before
    /// did not. It answers the frontend's keys and clicks up to number
    /// `answered` (counting from 1, in the order sent), if any.
    Snapshot { snapshot: Box<Snapshot>, texts: Vec<(u64, Text)>, images: Vec<ImageData>, answered: Option<u64> },
    /// Any other output.
    Output(Output),
    /// The runtime crashed and another took over: views and images are sent
    /// anew, and the frontend's inputs sent before are not answered.
    Restarted,
    /// The frontend is not attached, or no longer: why. Nothing follows.
    Failed(String),
}

/// A view's text, from the text it was last sent.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
enum Text {
    /// All of it: the view was not sent before.
    Whole(String),
    /// `removed` bytes at byte `at` replaced by `inserted`.
    Splice { at: usize, removed: usize, inserted: String },
}

fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let bytes = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::other(format!("a frame of {} bytes is past the {MAX_FRAME}-byte limit", bytes.len())));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// The next message, or `None` at a clean end of stream.
fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> std::io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other(format!("a frame of {len} bytes is past the {MAX_FRAME}-byte limit")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    rmp_serde::from_slice(&buf).map(Some).map_err(std::io::Error::other)
}

// The editor's side

/// The editor's side of an attachment, shared by its two threads: the one
/// reading the frontend's inputs and the one sending it outputs.
struct Served {
    host: Mutex<Option<Host>>,
    /// The numbers of the frontend's keys and clicks the runtime was sent
    /// and has not answered, oldest first.
    unanswered: Mutex<VecDeque<u64>>,
    /// How many times the runtime was restarted since the frontend
    /// attached: a capture for an earlier one is void.
    restarts: AtomicU64,
}

/// An output waiting to be sent.
enum Item {
    /// A snapshot, and how many keys and clicks it and those it replaced
    /// answer.
    Snapshot(Box<Snapshot>, usize),
    Other(Event),
}

/// The outputs waiting to be sent to a frontend, and whether more come.
#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    /// The runtime detached the frontend, or it is being disconnected.
    closed: bool,
}

type Waiting = Arc<(Mutex<Queue>, Condvar)>;

/// Closes the queue when the runtime drops the frontend's delivery, as it
/// does detaching it.
struct CloseOnDrop(Waiting);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.0.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.0.1.notify_all();
    }
}

/// Queue `event` for the frontend, from the runtime's thread: never blocks
/// on the frontend. A snapshot replaces one waiting last that asks nothing
/// of the frontend. Past `MAX_QUEUED`, the frontend is disconnected.
fn enqueue(waiting: &Waiting, connection: &UnixStream, event: Event) {
    let mut queue = waiting.0.lock().unwrap_or_else(|e| e.into_inner());
    if queue.closed {
        return;
    }
    match event {
        Event::Output(Output::Snapshot(s)) => {
            let answers = s.answers.len();
            match queue.items.back_mut() {
                Some(Item::Snapshot(last, n)) if last.panes.iter().all(|p| p.request.is_none()) => {
                    *last = s;
                    *n += answers;
                }
                _ => queue.items.push_back(Item::Snapshot(s, answers)),
            }
        }
        // The host keeps it, for a restart.
        Event::Output(Output::Session(_)) => return,
        other => queue.items.push_back(Item::Other(other)),
    }
    if queue.items.len() > MAX_QUEUED {
        queue.items.clear();
        queue.closed = true;
        let _ = connection.shutdown(std::net::Shutdown::Both);
    }
    waiting.1.notify_one();
}

/// Attach the frontend at the other end of `stream`, which asked to
/// (`Request::Attach`), to the runtime of `shared`, and serve it until it
/// detaches or its connection ends.
pub(crate) fn serve(shared: &Arc<Mutex<Shared>>, mut stream: UnixStream, profile: String, path: Option<PathBuf>) {
    let file = match path.map(|path| journal_for(&path).map(|journal| File { path, journal })).transpose() {
        Ok(file) => file,
        Err(e) => {
            let _ = write_frame(&mut stream, &Down::Failed(format!("journal: {e}")));
            return;
        }
    };
    let (Ok(out), Ok(connection)) = (stream.try_clone(), stream.try_clone()) else { return };
    let waiting = Waiting::default();
    let closing = CloseOnDrop(waiting.clone());
    let deliver = Arc::new(move |e| enqueue(&closing.0, &connection, e));
    let host = crate::host::attach(shared, file, profile, deliver);
    let served = Arc::new(Served { host: Mutex::new(Some(host)), unanswered: Mutex::default(), restarts: AtomicU64::new(0) });
    let theirs = served.clone();
    let writer = std::thread::Builder::new().name("attachment out".into()).spawn(move || {
        let mut out = out;
        send_events(&waiting, &mut out, &theirs);
        // However it stopped, the connection ends, and with it the reading.
        let _ = out.shutdown(std::net::Shutdown::Both);
    });
    // Until the frontend closes the connection, or it fails.
    let mut numbered = 0;
    while let Ok(Some(up)) = read_frame::<Up>(&mut stream) {
        match up {
            Up::Input(input) => {
                let counted = matches!(input, Input::Key { .. } | Input::Click { .. });
                numbered += counted as u64;
                // Taken before the runtime has it, so that its answer finds it.
                let mut unanswered = served.unanswered.lock().expect("the attachment");
                let mut host = served.host.lock().expect("the attachment");
                let Some(host) = host.as_mut() else { break };
                if host.sent(input) && counted {
                    unanswered.push_back(numbered);
                }
            }
            Up::Captured { generation, id, result } => {
                if generation == served.restarts.load(Ordering::Acquire)
                    && let Some(host) = served.host.lock().expect("the attachment").as_ref()
                {
                    host.captured(id, result);
                }
            }
        }
    }
    if let Some(host) = served.host.lock().expect("the attachment").take() {
        host.close();
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    if let Ok(w) = writer {
        let _ = w.join();
    }
}

/// Send the frontend what waits for it as it comes, until the queue is
/// closed and empty, the frontend cannot be written to, or it is told it
/// is no longer attached.
fn send_events(waiting: &Waiting, out: &mut UnixStream, served: &Served) {
    let mut sent = Sent::default();
    loop {
        let item = {
            let mut queue = waiting.0.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(item) = queue.items.pop_front() {
                    break item;
                }
                if queue.closed {
                    return;
                }
                queue = waiting.1.wait(queue).unwrap_or_else(|e| e.into_inner());
            }
        };
        let down = match item {
            Item::Snapshot(s, answers) => {
                let mut unanswered = served.unanswered.lock().expect("the attachment");
                let answered = (0..answers).map_while(|_| unanswered.pop_front()).last();
                drop(unanswered);
                sent.down(s, answered)
            }
            Item::Other(Event::Output(o)) => Down::Output(o),
            Item::Other(Event::Failed(e)) => Down::Failed(e),
            // The runtime crashed: this frontend restarts it, unless another
            // did, and is attached to the new one.
            Item::Other(Event::Ended) => {
                let restarted = served.host.lock().expect("the attachment").as_mut().is_some_and(Host::restart);
                if restarted {
                    // Its views and images are another runtime's: sent anew.
                    sent = Sent::default();
                    served.unanswered.lock().expect("the attachment").clear();
                    served.restarts.fetch_add(1, Ordering::AcqRel);
                    Down::Restarted
                } else {
                    Down::Failed("the runtime stopped".into())
                }
            }
        };
        let last = matches!(down, Down::Failed(_));
        if let Err(e) = write_frame(out, &down) {
            // Too large, say: the frontend is told, if it can be.
            let _ = write_frame(out, &Down::Failed(format!("an output could not be sent: {e}")));
            return;
        }
        if last {
            return;
        }
    }
}

/// What a frontend was sent: each view's text, at its revision, and the
/// images the last snapshot showed.
#[derive(Default)]
struct Sent {
    texts: HashMap<u64, (u64, Rope)>,
    images: HashSet<u64>,
}

impl Sent {
    /// `snapshot` to send, answering up to `answered`.
    fn down(&mut self, mut snapshot: Box<Snapshot>, answered: Option<u64>) -> Down {
        let mut texts = Vec::new();
        let mut images = Vec::new();
        let mut shown = HashSet::new();
        for pane in &snapshot.panes {
            // Two panes may show one view: one update.
            if texts.iter().any(|(view, _)| *view == pane.view) {
                continue;
            }
            match self.texts.get(&pane.view) {
                Some((revision, _)) if *revision == pane.revision => {}
                Some((_, old)) => texts.push((pane.view, splice(old, &pane.text))),
                None => texts.push((pane.view, Text::Whole(pane.text.to_string()))),
            }
        }
        for b in snapshot.panes.iter().flat_map(|p| &p.blocks) {
            let id = b.image.id();
            if shown.insert(id) && !self.images.contains(&id) {
                let d = &b.image.0;
                images.push(ImageData { id, width: d.width, height: d.height, rgba: d.rgba.clone() });
            }
        }
        // The views shown now; a view shown again later is sent whole.
        self.texts.retain(|view, _| snapshot.panes.iter().any(|p| p.view == *view));
        for pane in &snapshot.panes {
            self.texts.insert(pane.view, (pane.revision, pane.text.clone()));
        }
        // The frontend keeps the pixels of the images shown, as here.
        self.images = shown;
        snapshot.answers.clear();
        Down::Snapshot { snapshot, texts, images, answered }
    }
}

/// How `new` differs from `old`: one splice, between the bytes they share
/// at the start and those they share at the end, at character boundaries.
fn splice(old: &Rope, new: &Rope) -> Text {
    let (old_len, new_len) = (old.len_bytes(), new.len_bytes());
    // Both texts agree before `at`, so a character crossing it in one
    // crosses it in the other: its first byte says how long it is.
    let mut at = common_prefix(old, new);
    while at > 0 && !boundary(new, at) {
        at -= 1;
    }
    let mut end = common_suffix(old, new).min(old_len.min(new_len) - at);
    while end > 0 && !boundary(new, new_len - end) {
        end -= 1;
    }
    let inserted = new.byte_slice(at..new_len - end).to_string();
    Text::Splice { at, removed: old_len - end - at, inserted }
}

/// Whether a character starts at byte `i` of `text`, or it ends there.
fn boundary(text: &Rope, i: usize) -> bool {
    i == text.len_bytes() || text.byte(i) & 0xC0 != 0x80
}

/// How many bytes two texts have in common at their start.
fn common_prefix(a: &Rope, b: &Rope) -> usize {
    common(a.chunks(), b.chunks(), |x, k| &x[..k], |x, k| &x[k..], |x, y| x.iter().zip(y).position(|(p, q)| p != q))
}

/// How many bytes two texts have in common at their end.
fn common_suffix(a: &Rope, b: &Rope) -> usize {
    fn backwards(r: &Rope) -> techne_text::ropey::iter::Chunks<'_> {
        r.chunks_at_byte(r.len_bytes()).0.reversed()
    }
    common(
        backwards(a),
        backwards(b),
        |x, k| &x[x.len() - k..],
        |x, k| &x[..x.len() - k],
        |x, y| x.iter().rev().zip(y.iter().rev()).position(|(p, q)| p != q),
    )
}

/// How many bytes the chunks `a` and `b` give agree, from the side `take`
/// takes `k` of a chunk from (`drop` leaves the rest); `differ` says how
/// far two such runs agree when they do not entirely.
fn common<'a>(
    mut a: impl Iterator<Item = &'a str>,
    mut b: impl Iterator<Item = &'a str>,
    take: impl Fn(&'a [u8], usize) -> &'a [u8],
    drop: impl Fn(&'a [u8], usize) -> &'a [u8],
    differ: impl Fn(&[u8], &[u8]) -> Option<usize>,
) -> usize {
    let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
    let mut n = 0;
    loop {
        if x.is_empty() {
            match a.next() {
                Some(c) => x = c.as_bytes(),
                None => return n,
            }
            continue;
        }
        if y.is_empty() {
            match b.next() {
                Some(c) => y = c.as_bytes(),
                None => return n,
            }
            continue;
        }
        let k = x.len().min(y.len());
        let (p, q) = (take(x, k), take(y, k));
        // Chunks the two texts share are the same memory.
        if std::ptr::eq(p, q) || p == q {
            n += k;
            (x, y) = (drop(x, k), drop(y, k));
        } else {
            return n + differ(p, q).unwrap_or(k);
        }
    }
}

/// Apply `text` to `rope`, the view's text as it was last sent.
fn apply(rope: &mut Rope, text: Text) -> Result<(), String> {
    match text {
        Text::Whole(s) => *rope = Rope::from_str(&s),
        Text::Splice { at, removed, inserted } => {
            let end = at.checked_add(removed).filter(|&e| e <= rope.len_bytes() && boundary(rope, at) && boundary(rope, e));
            let Some(end) = end else { return Err(format!("a splice of {removed} bytes at {at} is not in the text")) };
            let (from, to) = (rope.byte_to_char(at), rope.byte_to_char(end));
            rope.remove(from..to);
            rope.insert(from, &inserted);
        }
    }
    Ok(())
}

// The frontend's side

/// An attachment to an editor in another process, through its socket: what
/// a frontend does with a `Host`, it does with this.
pub struct Remote {
    stream: UnixStream,
    /// The keys and clicks sent, numbered from 1.
    numbered: u64,
    /// Those sent and not answered yet, by number, with when they were
    /// made, oldest first.
    waiting: Arc<Mutex<VecDeque<(u64, Instant)>>>,
    /// How many times the editor said its runtime was restarted.
    generation: Arc<AtomicU64>,
}

impl Remote {
    /// Attach to the editor listening on `socket`: a frontend reading keys
    /// by `profile`, showing the file at `path` or else what the session
    /// used last shows. `deliver` gets the runtime's outputs, as `Host`'s
    /// does, from a thread of its own: `Event::Ended` when the editor
    /// restarted its runtime (the frontend is attached to the new one:
    /// `Link::restart` says so), `Event::Failed` when it is no longer
    /// attached.
    pub fn attach(
        socket: &Path,
        profile: &str,
        path: Option<&Path>,
        deliver: impl Fn(Event) + Send + 'static,
    ) -> Result<Remote, OpenError> {
        let mut stream = UnixStream::connect(socket).map_err(|_| OpenError::NotRunning)?;
        let failed = |e: std::io::Error| OpenError::Failed(e.to_string());
        let path = path.map(std::path::absolute).transpose().map_err(failed)?;
        crate::server::write_request(&mut stream, &Request::Attach { profile: profile.into(), file: path }).map_err(failed)?;
        let waiting = Arc::<Mutex<VecDeque<(u64, Instant)>>>::default();
        let generation = Arc::new(AtomicU64::new(0));
        let input = stream.try_clone().map_err(failed)?;
        let (w, g) = (waiting.clone(), generation.clone());
        std::thread::Builder::new()
            .name("attachment in".into())
            .spawn(move || {
                let why = receive(input, &w, &g, &deliver).err().unwrap_or_else(|| "the editor went away".into());
                deliver(Event::Failed(why));
            })
            .map_err(failed)?;
        Ok(Remote { stream, numbered: 0, waiting, generation })
    }

    /// Send an input. C-g interrupts what the runtime does for this
    /// frontend as soon as it arrives, as it does for one in its process.
    pub fn send(&mut self, input: Input) {
        if let Input::Key { at, .. } | Input::Click { at, .. } = &input {
            self.numbered += 1;
            self.waiting.lock().expect("the waiting").push_back((self.numbered, *at));
        }
        let _ = write_frame(&mut self.stream, &Up::Input(input));
    }

    /// When the runtime is busy, as far as this frontend can tell: as
    /// `Host::busy_from`.
    pub fn busy_from(&self) -> Option<Instant> {
        self.waiting.lock().expect("the waiting").front().map(|(_, at)| *at + BUSY_AFTER)
    }

    /// Complete a capture (`Output::Capture`) of the runtime attached to
    /// now: one of a runtime restarted since is dropped by the editor.
    pub fn captured(&mut self, id: usize, result: Result<CapturedFrame, String>) {
        let generation = self.generation.load(Ordering::Acquire);
        let _ = write_frame(&mut self.stream, &Up::Captured { generation, id, result });
    }

    /// Detach: the connection ends.
    pub fn close(self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Deliver what comes on `input` as outputs, until the connection ends
/// (Ok) or a message cannot be read or applied (why).
fn receive(
    mut input: UnixStream,
    waiting: &Mutex<VecDeque<(u64, Instant)>>,
    generation: &AtomicU64,
    deliver: &impl Fn(Event),
) -> Result<(), String> {
    let mut texts: HashMap<u64, Rope> = HashMap::new();
    let mut images: HashMap<u64, Image> = HashMap::new();
    loop {
        let down = match read_frame::<Down>(&mut input) {
            Ok(Some(down)) => down,
            // The editor's process ended, with what it had not read.
            Ok(None) => return Ok(()),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        match down {
            Down::Snapshot { mut snapshot, texts: updates, images: new, answered } => {
                for (view, text) in updates {
                    match text {
                        Text::Whole(s) => drop(texts.insert(view, Rope::from_str(&s))),
                        splice => apply(texts.get_mut(&view).ok_or_else(|| format!("a splice of view {view}, never sent"))?, splice)?,
                    }
                }
                texts.retain(|view, _| snapshot.panes.iter().any(|p| p.view == *view));
                images.extend(new.into_iter().map(|d| (d.id, Image(Arc::new(d)))));
                for pane in &mut snapshot.panes {
                    pane.text = texts.get(&pane.view).ok_or_else(|| format!("no text for view {}", pane.view))?.clone();
                    for b in &mut pane.blocks {
                        b.image = images.get(&b.image.id()).ok_or_else(|| format!("no pixels for image {}", b.image.id()))?.clone();
                    }
                }
                // The pixels of the images shown, as the editor keeps track.
                images.retain(|id, _| snapshot.panes.iter().flat_map(|p| &p.blocks).any(|b| b.image.id() == *id));
                if let Some(answered) = answered {
                    let mut waiting = waiting.lock().expect("the waiting");
                    while waiting.front().is_some_and(|(n, _)| *n <= answered) {
                        snapshot.answers.extend(waiting.pop_front().map(|(_, at)| at));
                    }
                }
                deliver(Event::Output(Output::Snapshot(snapshot)));
            }
            Down::Output(o) => deliver(Event::Output(o)),
            Down::Restarted => {
                texts.clear();
                images.clear();
                waiting.lock().expect("the waiting").clear();
                generation.fetch_add(1, Ordering::AcqRel);
                deliver(Event::Ended);
            }
            Down::Failed(e) => return Err(e),
        }
    }
}

/// Where a frontend is attached: to a runtime it started in its own
/// process (`Host`), or to an editor in another (`Remote`).
pub enum Link {
    Local(Host),
    Remote(Remote),
}

impl Link {
    pub fn send(&mut self, input: Input) {
        match self {
            Link::Local(h) => h.send(input),
            Link::Remote(r) => r.send(input),
        }
    }

    pub fn busy_from(&self) -> Option<Instant> {
        match self {
            Link::Local(h) => h.busy_from(),
            Link::Remote(r) => r.busy_from(),
        }
    }

    /// After `Event::Ended`, as `Host::restart`: whether the frontend is
    /// attached to a new runtime. The editor of another process restarted
    /// its runtime itself before saying it ended, and it is.
    pub fn restart(&mut self) -> bool {
        match self {
            Link::Local(h) => h.restart(),
            Link::Remote(_) => true,
        }
    }

    pub fn captured(&mut self, id: usize, result: Result<CapturedFrame, String>) {
        match self {
            Link::Local(h) => h.captured(id, result),
            Link::Remote(r) => r.captured(id, result),
        }
    }

    /// Detach. A runtime of this process serves the frontends still
    /// attached to it, from other processes, until they detach too.
    pub fn close(self) {
        match self {
            Link::Local(h) => h.close_outliving(),
            Link::Remote(r) => r.close(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The splice from `a` to `b`, checked to make `b` of `a`.
    fn check(a: &str, b: &str) -> Text {
        let (old, new) = (Rope::from_str(a), Rope::from_str(b));
        let text = splice(&old, &new);
        let shown = format!("{text:?}");
        let mut rope = old.clone();
        apply(&mut rope, text).unwrap();
        assert_eq!(rope.to_string(), b, "{a:?} to {b:?} by {shown}");
        splice(&old, &new)
    }

    #[test]
    fn a_splice_is_what_changed() {
        assert_eq!(check("hello world", "hello brave world"), Text::Splice { at: 6, removed: 0, inserted: "brave ".into() });
        assert_eq!(check("abc", "abc"), Text::Splice { at: 3, removed: 0, inserted: String::new() });
        assert_eq!(check("abc", ""), Text::Splice { at: 0, removed: 3, inserted: String::new() });
        assert_eq!(check("", "abc"), Text::Splice { at: 0, removed: 0, inserted: "abc".into() });
        // Repeats: the suffix never runs into the prefix.
        assert_eq!(check("aaa", "aaaa"), Text::Splice { at: 3, removed: 0, inserted: "a".into() });
        assert_eq!(check("abab", "ab"), Text::Splice { at: 2, removed: 2, inserted: String::new() });
    }

    #[test]
    fn a_splice_keeps_to_character_boundaries() {
        // é is C3 A9 and è C3 A8: they share a first byte, not a character.
        assert_eq!(check("é", "è"), Text::Splice { at: 0, removed: 2, inserted: "è".into() });
        // And Ã© ends as é does.
        check("xé", "xÃ©");
        check("日本語", "日本人語");
        check("🙂a", "🙃a");
        check("a🙂", "b🙂");
    }

    #[test]
    fn a_splice_across_chunks() {
        // Ropes of many chunks, sharing most of them.
        let big: String = (0..20_000).map(|i| format!("line {i} é\n")).collect();
        let old = Rope::from_str(&big);
        let mut new = old.clone();
        let middle = new.len_chars() / 2;
        new.insert(middle, "inserted");
        new.remove(10..20);
        let text = splice(&old, &new);
        let mut rope = old.clone();
        apply(&mut rope, text).unwrap();
        assert_eq!(rope, new);
    }

    #[test]
    fn a_splice_outside_the_text_is_refused() {
        let mut rope = Rope::from_str("é");
        assert!(apply(&mut rope, Text::Splice { at: 1, removed: 0, inserted: "x".into() }).is_err());
        assert!(apply(&mut rope, Text::Splice { at: 0, removed: 3, inserted: String::new() }).is_err());
    }
}
