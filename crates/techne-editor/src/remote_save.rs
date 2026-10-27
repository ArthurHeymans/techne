//! Remote writes outlive the task waiting for them: the node has already
//! received the request. Keep ownership of each pending save until its
//! answer is taken, so an abandoned overwrite cannot race a newer save.

use std::{
    cell::RefCell,
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll, Waker},
};

use techne_node::RequestError;
use techne_text::{Document, PendingSave};

type Reply = Result<Result<[u8; 32], Option<[u8; 32]>>, RequestError>;
type Outcome = Result<(), String>;

#[derive(Clone, Default)]
pub(crate) struct Saves(Rc<RefCell<Registry>>);

#[derive(Default)]
struct Registry {
    entries: HashMap<String, Rc<RefCell<Save>>>,
    generations: HashMap<String, u64>,
}

fn location(doc: &Rc<RefCell<Document>>) -> Result<String, String> {
    match doc.borrow().place() {
        Some(techne_text::Place::Remote(name)) => Ok(name.clone()),
        _ => Err("not a remote document".into()),
    }
}

struct Save {
    document: Weak<RefCell<Document>>,
    waiting: bool,
    state: State,
}

enum State {
    Writing { reply: Pin<Box<dyn Future<Output = Reply>>>, save: Option<PendingSave> },
    Finished(Outcome),
    // Connection loss is not proof that the remote side effect stopped.
    Unknown(String),
}

/// Dropping the caller's wait abandons neither the write nor its slot.
/// Only abandoned replies may be polled by a later save or reload: polling
/// a live caller's reply with a noop waker would steal its notification.
pub(crate) struct Waiter {
    saves: Saves,
    _document: Rc<RefCell<Document>>,
    location: String,
    save: Rc<RefCell<Save>>,
}

impl Future for Waiter {
    type Output = Outcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Outcome> {
        let result = self.save.borrow_mut().poll(cx);
        if result.is_ready() {
            self.saves.remove(&self.location, &self.save);
        }
        result
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        self.save.borrow_mut().waiting = false;
    }
}

impl Save {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Outcome> {
        let (reply, save) = match &mut self.state {
            State::Writing { reply, save } => (reply, save),
            State::Finished(result) => return Poll::Ready(result.clone()),
            State::Unknown(reason) => return Poll::Ready(Err(reason.clone())),
        };
        let written = match reply.as_mut().poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(written)) => written,
            Poll::Ready(Err(RequestError::Failed(reason))) => {
                self.state = State::Finished(Err(reason.clone()));
                return Poll::Ready(Err(reason));
            }
            Poll::Ready(Err(RequestError::Unknown(reason))) => {
                let reason = format!(
                    "remote save outcome unknown ({reason}); further saves and reloads of this location are blocked without remote write fencing"
                );
                self.state = State::Unknown(reason.clone());
                return Poll::Ready(Err(reason));
            }
        };
        let result = match written {
            Ok(_) => match self.document.upgrade() {
                Some(doc) => doc
                    .borrow_mut()
                    .end_save(save.take().expect("a pending save"))
                    .map_err(|e| format!("saved, but the journal did not start again: {e}")),
                // The journal's Saving record lets a later open recognize
                // the acknowledged write; there is no live document to settle.
                None => Ok(()),
            },
            Err(_) => Err(techne_text::SaveError::ChangedOnDisk.to_string()),
        };
        self.state = State::Finished(result.clone());
        Poll::Ready(result)
    }
}

impl Saves {
    fn remove(&self, location: &str, save: &Rc<RefCell<Save>>) {
        let mut saves = self.0.borrow_mut();
        if !matches!(save.borrow().state, State::Unknown(_)) && saves.entries.get(location).is_some_and(|current| Rc::ptr_eq(current, save))
        {
            saves.entries.remove(location);
        }
    }

    /// Changes even if a completed save wrote identical bytes. A reload
    /// must not accept bytes read before such an overlapping save.
    pub(crate) fn generation(&self, doc: &Rc<RefCell<Document>>) -> Result<u64, String> {
        Ok(self.generation_location(&location(doc)?))
    }

    /// Take a previous save's answer, if ready, before saving or reloading.
    /// A definitive failure releases the slot; an unknown outcome keeps
    /// the location quarantined, including across reconnection/reopening.
    pub(crate) fn settle(&self, doc: &Rc<RefCell<Document>>) -> Outcome {
        self.settle_location(&location(doc)?)
    }

    fn generation_location(&self, location: &str) -> u64 {
        self.0.borrow().generations.get(location).copied().unwrap_or(0)
    }

    fn settle_location(&self, location: &str) -> Outcome {
        let Some(save) = self.0.borrow().entries.get(location).cloned() else { return Ok(()) };
        if let State::Unknown(reason) = &save.borrow().state {
            return Err(reason.clone());
        }
        if save.borrow().waiting {
            return Err("a save of this document is under way".into());
        }
        let result = save.borrow_mut().poll(&mut Context::from_waker(Waker::noop()));
        match result {
            Poll::Pending => Err("a save of this document is under way".into()),
            Poll::Ready(result) => {
                self.remove(location, &save);
                result
            }
        }
    }

    /// Opening can replace a journal just as reloading can. Check before
    /// issuing a read and again before using its bytes, including saves
    /// which finished (and released their slot) while the read waited.
    pub(crate) async fn open<F: Future<Output = Result<Option<Vec<u8>>, String>>>(
        &self,
        documents: &crate::Documents,
        name: &str,
        read: impl FnOnce() -> F,
        journal: impl FnOnce() -> Result<PathBuf, String>,
    ) -> Result<Rc<RefCell<Document>>, String> {
        let key = PathBuf::from(name);
        if let Some(doc) = documents.borrow().get(&key).and_then(Weak::upgrade) {
            return Ok(doc);
        }
        self.settle_location(name)?;
        let generation = self.generation_location(name);
        let bytes = read().await.map_err(|e| format!("{name}: {e}"))?;
        // Another task may have opened it while this one read. Returning
        // its document does not rebuild a journal or consume stale bytes.
        if let Some(doc) = documents.borrow().get(&key).and_then(Weak::upgrade) {
            return Ok(doc);
        }
        self.settle_location(name)?;
        if self.generation_location(name) != generation {
            return Err("the file was saved while it was read; open it again".into());
        }
        let (doc, _) = Document::open_remote(name, bytes, &journal()?).map_err(|e| format!("{name}: {e}"))?;
        let doc = Rc::new(RefCell::new(doc));
        documents.borrow_mut().insert(key, Rc::downgrade(&doc));
        Ok(doc)
    }

    pub(crate) fn start<F: Future<Output = Reply> + 'static>(
        &self,
        doc: &Rc<RefCell<Document>>,
        write: impl FnOnce(&PendingSave) -> F,
    ) -> Result<Waiter, String> {
        self.settle(doc)?;
        let location = location(doc)?;
        let pending = doc.borrow_mut().begin_save().map_err(|e| e.to_string())?;
        let reply = Box::pin(write(&pending));
        let save = Rc::new(RefCell::new(Save {
            document: Rc::downgrade(doc),
            waiting: true,
            state: State::Writing { reply, save: Some(pending) },
        }));
        let mut saves = self.0.borrow_mut();
        *saves.generations.entry(location.clone()).or_default() += 1;
        saves.entries.insert(location.clone(), save.clone());
        Ok(Waiter { saves: self.clone(), _document: doc.clone(), location, save })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn document(dir: &std::path::Path) -> Rc<RefCell<Document>> {
        let (doc, _) = Document::open_remote("/ssh:test:/file", Some(b"old".to_vec()), &dir.join("journal")).unwrap();
        let doc = Rc::new(RefCell::new(doc));
        let tx = doc.borrow().edit(&"user".into(), [(0..3, "saved")]).unwrap();
        doc.borrow_mut().apply(tx).unwrap();
        doc
    }

    fn start(saves: &Saves, doc: &Rc<RefCell<Document>>) -> (oneshot::Sender<Reply>, impl Future<Output = Outcome> + use<>) {
        let (tx, rx) = oneshot::channel();
        let wait = saves.start(doc, |_| async move { rx.await.map_err(|_| RequestError::Unknown("connection lost".into()))? }).unwrap();
        (tx, wait)
    }

    #[test]
    fn cancellation_keeps_the_write_owned_until_its_answer_is_known() {
        let dir = tempfile::tempdir().unwrap();
        let doc = document(dir.path());
        let saves = Saves::default();
        let (reply, wait) = start(&saves, &doc);
        drop(wait);
        assert!(saves.settle(&doc).unwrap_err().contains("under way"));
        assert!(saves.start(&doc, |_| std::future::ready(Ok(Ok([0; 32])))).is_err());
        // Edits may continue, but settling must mark only the saved revision.
        let tx = doc.borrow().edit(&"user".into(), [(5..5, " later")]).unwrap();
        doc.borrow_mut().apply(tx).unwrap();
        reply.send(Ok(Ok([0; 32]))).unwrap();
        saves.settle(&doc).unwrap();
        assert!(doc.borrow().is_dirty());
        let (reply, wait) = start(&saves, &doc);
        drop(wait);
        reply.send(Ok(Ok([0; 32]))).unwrap();
        saves.settle(&doc).unwrap();
        assert!(!doc.borrow().is_dirty());
        assert_eq!(doc.borrow().text().to_string(), "saved later");
    }

    #[test]
    fn a_second_caller_does_not_steal_the_live_waiters_wake() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::task::Wake;

        #[derive(Default)]
        struct Woke(AtomicBool);
        impl Wake for Woke {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let doc = document(dir.path());
        let saves = Saves::default();
        let (reply, wait) = start(&saves, &doc);
        let mut wait = Box::pin(wait);
        let woke = Arc::new(Woke::default());
        let waker = Waker::from(woke.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        assert!(saves.settle(&doc).unwrap_err().contains("under way"));
        reply.send(Ok(Ok([0; 32]))).unwrap();
        assert!(woke.0.load(Ordering::SeqCst));
        assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
        assert!(!doc.borrow().is_dirty());
        saves.settle(&doc).unwrap();
    }

    #[test]
    fn definitive_failures_release_the_slot_and_remain_dirty() {
        for failure in [Ok(Err(None)), Err(RequestError::Failed("write: permission denied".into()))] {
            let dir = tempfile::tempdir().unwrap();
            let doc = document(dir.path());
            let saves = Saves::default();
            let (reply, wait) = start(&saves, &doc);
            drop(wait);
            reply.send(failure).unwrap();
            assert!(saves.settle(&doc).is_err());
            assert!(doc.borrow().is_dirty());
            saves.settle(&doc).unwrap();
            let (_, wait) = start(&saves, &doc);
            drop(wait);
        }
    }

    #[test]
    fn reopening_does_not_read_or_replace_a_pending_or_unknown_saves_journal() {
        for unknown in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("journal");
            let doc = document(dir.path());
            let documents = crate::Documents::default();
            documents.borrow_mut().insert(PathBuf::from("/ssh:test:/file"), Rc::downgrade(&doc));
            let saves = Saves::default();
            let (reply, wait) = start(&saves, &doc);
            drop(wait);
            let reply = if unknown {
                reply.send(Err(RequestError::Unknown("connection lost".into()))).unwrap();
                None
            } else {
                Some(reply)
            };
            drop(doc);
            let before = std::fs::read(&journal).unwrap();
            let read = || -> std::future::Ready<Result<Option<Vec<u8>>, String>> { panic!("a blocked open must not issue a read") };
            let opened = techne_process::runtime().block_on(saves.open(&documents, "/ssh:test:/file", read, || Ok(journal.clone())));
            let error = opened.expect_err("the open must be refused");
            assert!(error.contains(if unknown { "outcome unknown" } else { "under way" }));
            assert_eq!(std::fs::read(&journal).unwrap(), before, "the Saving record must survive");
            if let Some(reply) = reply {
                // A definitive acknowledgement permits the same journal to
                // be recovered against the bytes that the old write saved.
                reply.send(Ok(Ok([0; 32]))).unwrap();
                let opened = techne_process::runtime()
                    .block_on(saves.open(
                        &documents,
                        "/ssh:test:/file",
                        || std::future::ready(Ok(Some(b"saved".to_vec()))),
                        || Ok(journal.clone()),
                    ))
                    .unwrap();
                assert_eq!(opened.borrow().text().to_string(), "saved");
                assert!(!opened.borrow().is_dirty());
            }
        }
    }

    #[test]
    fn an_open_refuses_bytes_read_before_a_completed_save() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal");
        let documents = crate::Documents::default();
        let saves = Saves::default();
        let saved_journal = RefCell::new(Vec::new());
        let read = || {
            // Another task opens, saves and drops the document while this
            // read waits. Its finished slot alone cannot detect the race.
            let doc = document(dir.path());
            documents.borrow_mut().insert(PathBuf::from("/ssh:test:/file"), Rc::downgrade(&doc));
            let (reply, wait) = start(&saves, &doc);
            drop(wait);
            reply.send(Ok(Ok([0; 32]))).unwrap();
            saves.settle(&doc).unwrap();
            *saved_journal.borrow_mut() = std::fs::read(&journal).unwrap();
            std::future::ready(Ok(Some(b"old".to_vec())))
        };
        let opened = techne_process::runtime().block_on(saves.open(&documents, "/ssh:test:/file", read, || Ok(journal.clone())));
        assert!(opened.expect_err("stale bytes must be refused").contains("saved while"));
        assert_eq!(std::fs::read(&journal).unwrap(), *saved_journal.borrow());
    }

    #[test]
    fn transport_loss_quarantines_a_write_that_can_still_land() {
        let dir = tempfile::tempdir().unwrap();
        let doc = document(dir.path());
        let saves = Saves::default();
        let (reply, wait) = start(&saves, &doc);
        drop(wait);
        // Losing the reply does not complete the simulated remote effect.
        let disk = dir.path().join("remote-file");
        let finish_old_write = || std::fs::write(&disk, "late old write").unwrap();
        reply.send(Err(RequestError::Unknown("connection lost".into()))).unwrap();
        assert!(saves.settle(&doc).unwrap_err().contains("outcome unknown"));
        assert!(doc.borrow().is_dirty());
        drop(doc);
        let other = tempfile::tempdir().unwrap();
        let reopened = document(other.path());
        assert!(
            saves
                .start(&reopened, |_| {
                    std::fs::write(&disk, "new write").unwrap();
                    std::future::ready(Ok(Ok([0; 32])))
                })
                .is_err()
        );
        assert!(!disk.exists(), "no replacement write was admitted");
        finish_old_write();
        assert_eq!(std::fs::read_to_string(&disk).unwrap(), "late old write");
        assert!(saves.settle(&reopened).unwrap_err().contains("outcome unknown"));
    }

    #[test]
    fn an_identical_completed_save_invalidates_an_earlier_reload() {
        let dir = tempfile::tempdir().unwrap();
        let doc = document(dir.path());
        let saves = Saves::default();
        let (reply, wait) = start(&saves, &doc);
        drop(wait);
        reply.send(Ok(Ok([0; 32]))).unwrap();
        saves.settle(&doc).unwrap();
        let before = (doc.borrow().revision(), doc.borrow().disk_hash(), saves.generation(&doc).unwrap());
        let (reply, wait) = start(&saves, &doc);
        drop(wait);
        reply.send(Ok(Ok([0; 32]))).unwrap();
        saves.settle(&doc).unwrap();
        let after = (doc.borrow().revision(), doc.borrow().disk_hash(), saves.generation(&doc).unwrap());
        assert_eq!((before.0, before.1), (after.0, after.1));
        assert_ne!(before, after, "a reload must reject its pre-save bytes even though the text and hash did not change");
    }
}
