//! Where `Budget` storage comes from: small blocks from the system
//! allocator, large ones (the nursery, old-generation blocks, register
//! stacks) mapped straight from the kernel.
//!
//! A world's large buffers are zero-filled and mostly untouched when made:
//! mapped pages cost nothing until used, and go back to the kernel when
//! freed. Through the system allocator they would not reliably: glibc
//! raises its mapping threshold after freeing a large block, so later large
//! blocks come from its heap, where zeroing touches every page: a new
//! world held about 1.3 MiB more that way.

use std::{
    alloc::{AllocError, Allocator, Layout, System},
    ptr::NonNull,
};

/// Blocks at least this large are mapped.
const LARGE: usize = 64 << 10;
const PAGE: usize = 4096;

#[derive(Clone, Copy, Default)]
pub(crate) struct Pages;

fn mapped(layout: Layout) -> bool {
    cfg!(all(target_os = "linux", not(miri))) && layout.size() >= LARGE && layout.align() <= PAGE
}

fn pages(size: usize) -> usize {
    size.next_multiple_of(PAGE)
}

fn slice(p: NonNull<u8>, len: usize) -> NonNull<[u8]> {
    NonNull::slice_from_raw_parts(p, len)
}

#[cfg(all(target_os = "linux", not(miri)))]
mod os {
    use std::ptr::NonNull;

    pub(super) fn map(len: usize) -> Option<NonNull<u8>> {
        // SAFETY: a fresh private anonymous mapping.
        let p = unsafe {
            libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0)
        };
        (p != libc::MAP_FAILED).then(|| NonNull::new(p.cast()).expect("mmap returned null"))
    }

    /// # Safety
    /// `p..p + len` is a mapping made by `map` (or `remap`) and unused.
    pub(super) unsafe fn unmap(p: NonNull<u8>, len: usize) {
        // SAFETY: forwarded from the caller.
        unsafe { libc::munmap(p.as_ptr().cast(), len) };
    }

    /// # Safety
    /// `p..p + old` is a mapping made by `map` (or `remap`).
    pub(super) unsafe fn remap(p: NonNull<u8>, old: usize, new: usize) -> Option<NonNull<u8>> {
        // SAFETY: forwarded from the caller; the kernel moves or resizes
        // the mapping, zero-filling any growth.
        let q = unsafe { libc::mremap(p.as_ptr().cast(), old, new, libc::MREMAP_MAYMOVE) };
        (q != libc::MAP_FAILED).then(|| NonNull::new(q.cast()).expect("mremap returned null"))
    }
}

#[cfg(not(all(target_os = "linux", not(miri))))]
mod os {
    use std::ptr::NonNull;

    pub(super) fn map(_: usize) -> Option<NonNull<u8>> {
        unreachable!("only Linux maps pages")
    }
    pub(super) unsafe fn unmap(_: NonNull<u8>, _: usize) {
        unreachable!("only Linux maps pages")
    }
    pub(super) unsafe fn remap(_: NonNull<u8>, _: usize, _: usize) -> Option<NonNull<u8>> {
        unreachable!("only Linux maps pages")
    }
}

// SAFETY: a block is either the system allocator's (small) or a mapping
// of whole pages (large); which follows from its layout's size and
// alignment alone, and every layout the caller may pass for a block lies
// on the same side as the one it was made with (a mapped block is returned
// exactly as large as requested, so any fitting layout is large too).
unsafe impl Allocator for Pages {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if !mapped(layout) {
            return System.allocate(layout);
        }
        self.allocate_zeroed(layout)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if !mapped(layout) {
            return System.allocate_zeroed(layout);
        }
        os::map(pages(layout.size())).map(|p| slice(p, layout.size())).ok_or(AllocError)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if mapped(layout) {
            // SAFETY: a mapped block of this size (see the impl's comment).
            unsafe { os::unmap(ptr, pages(layout.size())) }
        } else {
            // SAFETY: the system allocator's block with this layout.
            unsafe { System.deallocate(ptr, layout) }
        }
    }

    unsafe fn grow(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: forwarded from the caller; mapped growth is zero-filled.
        unsafe { self.grow_zeroed(ptr, old, new) }
    }

    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY (all arms): the caller supplies a live block made with
        // `old`, and `new` at least as large.
        unsafe {
            match (mapped(old), mapped(new)) {
                (true, true) => {
                    let p = os::remap(ptr, pages(old.size()), pages(new.size())).ok_or(AllocError)?;
                    // The kernel zeroes pages it adds, not the rest of the
                    // last page kept: a block shrunk before may have left
                    // bytes there.
                    let kept = pages(old.size()).min(new.size());
                    p.as_ptr().add(old.size()).write_bytes(0, kept - old.size());
                    Ok(slice(p, new.size()))
                }
                (false, false) => System.grow_zeroed(ptr, old, new),
                _ => self.moved(ptr, old, new),
            }
        }
    }

    unsafe fn shrink(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY (all arms): the caller supplies a live block made with
        // `old`, and `new` no larger.
        unsafe {
            match (mapped(old), mapped(new)) {
                (true, true) => os::remap(ptr, pages(old.size()), pages(new.size())).map(|p| slice(p, new.size())).ok_or(AllocError),
                (false, false) => System.shrink(ptr, old, new),
                _ => self.moved(ptr, old, new),
            }
        }
    }
}

impl Pages {
    /// A block changing sides: a new one, the contents copied, the old freed.
    ///
    /// # Safety
    /// As for `Allocator::grow`/`shrink`.
    unsafe fn moved(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let fresh = self.allocate_zeroed(new)?;
        // SAFETY: both blocks are live and distinct.
        unsafe {
            std::ptr::copy_nonoverlapping(ptr.as_ptr(), fresh.cast::<u8>().as_ptr(), old.size().min(new.size()));
            self.deallocate(ptr, old);
        }
        Ok(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_cross_between_sides_with_their_contents() {
        let mut v: Vec<u64, Pages> = Vec::new_in(Pages);
        for i in 0..100_000u64 {
            v.push(i);
        }
        assert!(v.iter().copied().eq(0..100_000));
        v.truncate(10);
        v.shrink_to_fit();
        assert!(v.iter().copied().eq(0..10));
        v.extend(10..50_000);
        assert!(v.iter().copied().eq(0..50_000));
    }

    #[test]
    fn regrown_bytes_are_zero() {
        let (big, bigger) = (Layout::from_size_align(LARGE + 2, 8).unwrap(), Layout::from_size_align(LARGE + 1, 8).unwrap());
        // SAFETY: each call is given the block and layout the previous made.
        unsafe {
            let p = Pages.allocate_zeroed(big).unwrap().cast::<u8>();
            p.as_ptr().add(LARGE + 1).write(0xa5);
            let p = Pages.shrink(p, big, bigger).unwrap().cast::<u8>();
            let p = Pages.grow_zeroed(p, bigger, big).unwrap().cast::<u8>();
            assert_eq!(p.as_ptr().add(LARGE + 1).read(), 0);
            Pages.deallocate(p, big);
        }
    }
}
