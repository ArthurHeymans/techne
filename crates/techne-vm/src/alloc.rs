//! What each world holds, counted where memory is allocated (PLAN.md,
//! Stage 1, language step 8.3): the global allocator charges every
//! allocation to the account of the world allocating it, and credits it
//! when it is freed, on whatever thread.
//!
//! Each block carries an 8-byte trailer after what was asked for: its
//! account's slot and the slot's generation. Accounts live in a static
//! table, so a block freed after its world is gone finds a newer
//! generation there and changes nothing (a slot's generation and count
//! are one word, changed together). A thread counts what it allocates
//! and frees for its current account in a thread-local delta, added to the
//! account when the thread changes accounts or the account is read; a
//! block of another account is credited to it at once, atomically.
//! Allocations outside any account (slot 0) are not counted.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

/// The allocator that counts (installed for every program using the VM).
pub struct Counting;

#[global_allocator]
static COUNTING: Counting = Counting;

/// Accounts at once, at most.
const SLOTS: usize = 1 << 16;

/// Each slot's generation (16 bits, above) and count of bytes (48 bits,
/// signed: a thread may credit before another's charge is added).
#[allow(clippy::declare_interior_mutable_const)]
const EMPTY: AtomicU64 = AtomicU64::new(0);
static TABLE: [AtomicU64; SLOTS] = [EMPTY; SLOTS];

const COUNT: u64 = (1 << 48) - 1;

fn generation_of(word: u64) -> u16 {
    (word >> 48) as u16
}

fn count_of(word: u64) -> isize {
    ((word << 16) as i64 >> 16) as isize
}

fn word(generation: u16, count: isize) -> u64 {
    (generation as u64) << 48 | (count as u64 & COUNT)
}

/// Slots given back, and the next never used (0 is no account).
static FREE: Mutex<(Vec<u32>, u32)> = Mutex::new((Vec::new(), 1));

/// A block's account: its slot, and the slot's generation above.
type Tag = u64;

const fn tag(slot: u32, generation: u16) -> Tag {
    slot as u64 | (generation as u64) << 32
}

thread_local! {
    /// The account this thread charges, and what it counted for it since
    /// it was last added there. A constant-initialized `Cell` of a type
    /// without drop: using it neither allocates nor registers a destructor,
    /// and it stays usable while the thread's other locals are torn down.
    static CURRENT: Cell<(Tag, isize)> = const { Cell::new((0, 0)) };
}

/// This thread's account.
#[inline(always)]
fn current() -> Tag {
    CURRENT.with(|c| c.get().0)
}

const TRAILER: usize = std::mem::size_of::<Tag>();

/// Where the trailer of a block of `layout` is, and the layout with it;
/// `None` past what a layout can describe.
#[inline(always)]
fn extended(layout: Layout) -> Option<(usize, Layout)> {
    // No overflow: a layout's size is at most `isize::MAX`.
    let at = layout.size().next_multiple_of(std::mem::align_of::<u32>());
    let align = layout.align().max(std::mem::align_of::<u32>());
    // What `Layout` requires of the size, its alignment being valid already.
    (at <= isize::MAX as usize - TRAILER - (align - 1)).then(|| (at, unsafe { Layout::from_size_align_unchecked(at + TRAILER, align) }))
}

/// `extended` of a layout that was extended before (a live block's).
#[inline(always)]
fn extended_live(layout: Layout) -> (usize, Layout) {
    // Its block was allocated with it.
    unsafe { extended(layout).unwrap_unchecked() }
}

/// Count `bytes` for this thread's account; its tag.
#[inline(always)]
fn charge(bytes: usize) -> Tag {
    CURRENT.with(|c| {
        let (tag, delta) = c.get();
        c.set((tag, delta + bytes as isize));
        tag
    })
}

/// Count `bytes` (negative: credit) for the account `tag`, unless it is
/// none or gone.
#[inline(always)]
fn recharge(tag: Tag, bytes: isize) {
    let here = CURRENT.with(|c| {
        let (current, delta) = c.get();
        let here = current == tag;
        if here {
            c.set((current, delta + bytes));
        }
        here
    });
    if !here {
        add(tag, bytes);
    }
}

/// Add this thread's delta to its account.
fn flush() {
    let (current, delta) = CURRENT.with(|c| {
        let (current, delta) = c.get();
        c.set((current, 0));
        (current, delta)
    });
    add(current, delta);
}

/// Make `tag` this thread's account (after `flush`); the one before.
fn switch(tag: Tag) -> Tag {
    CURRENT.with(|c| c.replace((tag, 0)).0)
}

/// Add `bytes` to the account `tag` in the table, if it is still there.
fn add(tag: Tag, bytes: isize) {
    let (slot, generation) = (tag as u32, (tag >> 32) as u16);
    if slot == 0 || bytes == 0 {
        return;
    }
    let _ = TABLE[slot as usize]
        .try_update(Relaxed, Relaxed, |w| (generation_of(w) == generation).then(|| word(generation, count_of(w) + bytes)));
}

unsafe impl GlobalAlloc for Counting {
    #[inline(always)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((at, total)) = extended(layout) else { return std::ptr::null_mut() };
        let p = unsafe { System.alloc(total) };
        if !p.is_null() {
            unsafe { p.add(at).cast::<Tag>().write_unaligned(charge(total.size())) };
        }
        p
    }

    #[inline(always)]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let Some((at, total)) = extended(layout) else { return std::ptr::null_mut() };
        let p = unsafe { System.alloc_zeroed(total) };
        if !p.is_null() {
            unsafe { p.add(at).cast::<Tag>().write_unaligned(charge(total.size())) };
        }
        p
    }

    #[inline(always)]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let (at, total) = extended_live(layout);
        recharge(unsafe { p.add(at).cast::<Tag>().read_unaligned() }, -(total.size() as isize));
        unsafe { System.dealloc(p, total) };
    }

    #[inline(always)]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let (at, total) = extended_live(layout);
        // `realloc`'s contract: `new_size` rounded to the alignment fits.
        let Some((new_at, new_total)) = extended(unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) }) else {
            return std::ptr::null_mut();
        };
        // The block stays with its account.
        let tag = unsafe { p.add(at).cast::<Tag>().read_unaligned() };
        let q = unsafe { System.realloc(p, total, new_total.size()) };
        if !q.is_null() {
            unsafe { q.add(new_at).cast::<Tag>().write_unaligned(tag) };
            recharge(tag, new_total.size() as isize - total.size() as isize);
        }
        q
    }
}

/// What a world's memory is charged to. Dropping it stops counting: what
/// it still has allocated is credited to nothing when freed.
pub struct Account {
    tag: Tag,
}

impl Account {
    /// A new account; `None` if every slot is taken.
    pub fn new() -> Option<Account> {
        let slot = {
            let mut free = FREE.lock().unwrap_or_else(|e| e.into_inner());
            match free.0.pop() {
                Some(slot) => slot,
                None if (free.1 as usize) < SLOTS => {
                    free.1 += 1;
                    free.1 - 1
                }
                None => return None,
            }
        };
        // Its count starts anew; its generation was moved on when the slot
        // was given back.
        let generation = generation_of(TABLE[slot as usize].load(Relaxed));
        TABLE[slot as usize].store(word(generation, 0), Relaxed);
        Some(Account { tag: tag(slot, generation) })
    }

    /// Bytes allocated and not freed, trailers included (what other threads
    /// counted for it in their current turn not yet).
    pub fn held(&self) -> usize {
        if current() == self.tag {
            flush();
        }
        count_of(TABLE[self.tag as u32 as usize].load(Relaxed)).max(0) as usize
    }

    /// Charge this thread's allocations to the account until the guard
    /// returned is dropped (which restores the account charged before).
    pub fn enter(&self) -> Entered {
        self.reference().enter()
    }

    /// The account, to charge from another thread (its work for the
    /// world): once the account is dropped, what it charges counts for
    /// nothing.
    pub fn reference(&self) -> AccountRef {
        AccountRef(self.tag)
    }
}

/// An account to charge (`Account::reference`).
#[derive(Clone, Copy)]
pub struct AccountRef(Tag);

impl AccountRef {
    /// `Account::enter`.
    pub fn enter(self) -> Entered {
        flush();
        Entered { previous: switch(self.0), entered: self.0, _thread: std::marker::PhantomData }
    }

    /// Whether this thread charges the account now.
    pub fn entered(self) -> bool {
        current() == self.0
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        if current() == self.tag {
            switch(0);
        }
        let slot = self.tag as u32;
        // Late frees of its blocks find another generation. A slot is used
        // by as many accounts as it has generations, then never again: its
        // first generation would come back.
        let generation = (self.tag >> 32) as u16 + 1;
        TABLE[slot as usize].store(word(generation, 0), Relaxed);
        if generation < u16::MAX {
            FREE.lock().unwrap_or_else(|e| e.into_inner()).0.push(slot);
        }
    }
}

/// While it lives, this thread's allocations go to an account
/// (`Account::enter`). It stays on the thread that entered (`!Send`), and
/// guards are dropped in the reverse order of entering. A guard restoring
/// an account dropped meanwhile charges nothing: its generation is gone.
pub struct Entered {
    previous: Tag,
    entered: Tag,
    _thread: std::marker::PhantomData<*const ()>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        // The account may have been dropped meanwhile (it then let go).
        debug_assert!(matches!(current(), t if t == self.entered || t == 0), "accounts left in the order entered");
        flush();
        switch(self.previous);
    }
}
