//! JSON-RPC's framing over a stream of bytes (`lisp/editor/rpc.scm`): the
//! `Content-Length` header and the body it counts.
//!
//! A framer is fed the bytes a peer writes, however they arrive: it keeps
//! what is not a whole frame yet, reads each header once (looking for the
//! delimiter only where it was not searched already), and hands back the
//! body of each frame that is whole. The header is strict: MIME's field
//! names are case-insensitive, a length is digits, two lengths must agree,
//! and a header or a body past a bound is the peer's failure, not
//! something to wait for more of. Bodies are strings; what they mean is
//! `string->json`'s business.

use std::cell::RefCell;

/// A header may not run past this.
const MAX_HEADER: usize = 8 << 10;

/// Nor a body past this.
const MAX_BODY: usize = 32 << 20;

/// What ends a header.
const DELIMITER: &[u8] = b"\r\n\r\n";

/// The bytes of frames arriving, and where the reading has got to.
#[derive(Default)]
pub struct Framer {
    /// Bytes read and not yet a whole frame, oldest first.
    inbox: Vec<u8>,
    /// The header's end: how many bytes of `inbox` it takes, and the body
    /// length it gives, once read.
    header: Option<(usize, usize)>,
    /// How much of the inbox was searched for the header's delimiter.
    scanned: usize,
    /// The id the next request takes (`take_id`).
    next_id: i64,
}

impl Framer {
    /// A request id no other request of this connection has: taken in
    /// one native call, so two tasks asking at once (tasks are
    /// preempted anywhere in Lisp) never get the same one.
    pub fn take_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    /// Take `bytes` as they arrive.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.inbox.extend_from_slice(bytes);
    }

    /// The next whole frame's body, or none while it is not whole.
    pub fn next_frame(&mut self) -> Result<Option<String>, String> {
        let (end, len) = match self.header {
            Some(read) => read,
            None => {
                // A delimiter spanning the last search's end is the only
                // one to look behind for.
                let from = self.scanned.saturating_sub(3);
                match find(&self.inbox[from..], DELIMITER) {
                    Some(at) => {
                        let end = from + at;
                        // However the header arrived: whole in one chunk
                        // is no licence to be longer.
                        if end > MAX_HEADER {
                            return Err("rpc: a frame header runs past the limit".into());
                        }
                        let len = length(&self.inbox[..end])?;
                        self.header = Some((end, len));
                        (end, len)
                    }
                    None => {
                        let searchable = self.inbox.len().min(MAX_HEADER + 4);
                        self.scanned = searchable;
                        if self.inbox.len() > MAX_HEADER {
                            return Err("rpc: a frame header runs past the limit".into());
                        }
                        return Ok(None);
                    }
                }
            }
        };
        let body = end + 4;
        if self.inbox.len() < body + len {
            return Ok(None);
        }
        let text = String::from_utf8(self.inbox[body..body + len].to_vec()).map_err(|e| format!("rpc: a frame's body is not text: {e}"))?;
        self.inbox.drain(..body + len);
        self.header = None;
        // What follows is the next frame's, not searched yet.
        self.scanned = 0;
        Ok(Some(text))
    }

    /// Forget what is buffered: the peer is gone, or spoke out of frame.
    pub fn clear(&mut self) {
        self.inbox.clear();
        self.header = None;
        self.scanned = 0;
    }
}

/// Where `needle` is in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The `Content-Length` a header gives, or why it does not give one.
fn length(header: &[u8]) -> Result<usize, String> {
    let header = std::str::from_utf8(header).map_err(|_| "rpc: a frame header is not text".to_string())?;
    let mut found = None;
    for line in header.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| "rpc: a malformed frame header".to_string())?;
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err("rpc: a malformed frame header".into());
        }
        let n: usize = value.parse().map_err(|_| "rpc: a frame's length is too large".to_string())?;
        if found.is_some_and(|f| f != n) {
            return Err("rpc: two frame lengths that differ".into());
        }
        found = Some(n);
    }
    let n = found.ok_or_else(|| "rpc: a frame header with no length".to_string())?;
    if n > MAX_BODY {
        return Err("rpc: a frame runs past the size limit".into());
    }
    Ok(n)
}

/// Define the framer's natives in `vm`.
pub fn install(vm: &mut techne_vm::vm::Vm) {
    vm.name_foreign_type::<RefCell<Framer>>("rpc-framer");
    techne_vm::procedures! { vm;
        /// Return a new framer, for a peer's bytes.
        "(make-rpc-framer)" => || techne_vm::api::Foreign::new(RefCell::new(Framer::default()));
        /// Give FRAMER the bytes CHUNK, a bytevector or a string, as they
        /// arrive from the peer.
        "(rpc-framer-feed! framer chunk)" => |f: techne_vm::api::Foreign<RefCell<Framer>>, chunk: techne_vm::api::Bytes| {
            f.0.borrow_mut().feed(&chunk.0);
        };
        /// Return the body of the next whole frame FRAMER holds, as a string,
        /// or #f while it holds none. A header past the bounds, malformed or
        /// of another length than its body, raises an error: the connection's
        /// to end.
        "(rpc-framer-next! framer)" => |f: techne_vm::api::Foreign<RefCell<Framer>>| -> Result<Option<String>, String> {
            f.0.borrow_mut().next_frame()
        };
        /// Return a request id for FRAMER's connection that no other request
        /// of it has, from 1 up.
        "(rpc-framer-take-id! framer)" => |f: techne_vm::api::Foreign<RefCell<Framer>>| f.0.borrow_mut().take_id();
        /// Forget what FRAMER holds: the peer is gone.
        "(rpc-framer-clear! framer)" => |f: techne_vm::api::Foreign<RefCell<Framer>>| f.0.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_arriving_in_pieces() {
        let mut f = Framer::default();
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let two = frame("{\"a\":1}") + &frame("{\"b\":2}");
        // One byte at a time.
        let mut bodies = Vec::new();
        for b in two.as_bytes() {
            f.feed(&[*b]);
            while let Some(body) = f.next_frame().unwrap() {
                bodies.push(body);
            }
        }
        assert_eq!(bodies, ["{\"a\":1}", "{\"b\":2}"]);
        // And a frame with a body of its own length only.
        f.feed(b"Content-Length: 2\r\n\r\n{}extra");
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("{}"));
        // Frames arriving together: each found in what the one before
        // left.
        let mut f = Framer::default();
        f.feed((frame("[1]") + &frame("[22]") + &frame("[333]")).as_bytes());
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("[1]"));
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("[22]"));
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("[333]"));
        assert_eq!(f.next_frame().unwrap(), None);
        // And a frame's header split across the end of a chunk after one.
        let two = frame("[1]") + &frame("[2]");
        let cut = frame("[1]").len() + 10;
        f.feed(&two.as_bytes()[..cut]);
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("[1]"));
        assert_eq!(f.next_frame().unwrap(), None);
        f.feed(&two.as_bytes()[cut..]);
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("[2]"));
    }

    #[test]
    fn headers_are_read_strictly() {
        let mut f = Framer::default();
        f.feed(b"content-LENGTH: 2\r\n\r\n[]");
        assert_eq!(f.next_frame().unwrap().as_deref(), Some("[]"));
        for bad in [
            &b"Content-Length: -5\r\n\r\n"[..],
            b"Content-Length: 5, please\r\n\r\n",
            b"Content-Length: 4\r\nContent-Length: 5\r\n\r\n",
            b"No-Length: 4\r\n\r\n",
        ] {
            let mut f = Framer::default();
            f.feed(bad);
            assert!(f.next_frame().is_err(), "{}", String::from_utf8_lossy(bad));
        }
        let mut f = Framer::default();
        f.feed(&vec![b'x'; MAX_HEADER + 1]);
        assert!(f.next_frame().is_err(), "a header past its bound");
        // Also when it arrives whole, its end with it.
        let mut f = Framer::default();
        f.feed(format!("Content-Length: 2\r\nX: {}\r\n\r\n{{}}", "y".repeat(MAX_HEADER)).as_bytes());
        assert!(f.next_frame().is_err(), "a whole header past its bound");
    }

    #[test]
    fn a_body_past_its_bound_is_refused_before_it_arrives() {
        let mut f = Framer::default();
        f.feed(format!("Content-Length: {}\r\n\r\n", MAX_BODY + 1).as_bytes());
        assert!(f.next_frame().is_err());
    }
}
