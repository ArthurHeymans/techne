//! `memset`, `memcpy` and `memmove` for small sizes. Hermit's
//! (compiler-builtins', weak) issue up to three `rep stos`/`rep movs` per
//! call, each costing tens of cycles to start: the interpreter clears and
//! slides a few registers per call, and spent most of its time there.
//! Accesses here are inline assembly so that LLVM does not turn the loops
//! back into calls to these very functions.

use std::{arch::asm, ffi::c_void};

/// Above this, a `rep` instruction (fast on CPUs with ERMSB) is worth its
/// start.
const SMALL: usize = 256;

/// # Safety
/// `p..p + 8` is writable.
#[inline(always)]
unsafe fn store(p: *mut u8, v: u64) {
    // SAFETY: forwarded from the caller; x86 stores need no alignment.
    unsafe { asm!("mov qword ptr [{p}], {v}", p = in(reg) p, v = in(reg) v, options(nostack, preserves_flags)) };
}

/// # Safety
/// `p..p + 8` is readable.
#[inline(always)]
unsafe fn load(p: *const u8) -> u64 {
    let v;
    // SAFETY: forwarded from the caller; x86 loads need no alignment.
    unsafe { asm!("mov {v}, qword ptr [{p}]", p = in(reg) p, v = out(reg) v, options(nostack, preserves_flags, readonly)) };
    v
}

/// # Safety
/// `p` is writable.
#[inline(always)]
unsafe fn store_byte(p: *mut u8, v: u8) {
    // SAFETY: forwarded from the caller.
    unsafe { asm!("mov byte ptr [{p}], {v}", p = in(reg) p, v = in(reg_byte) v, options(nostack, preserves_flags)) };
}

/// # Safety
/// `p` is readable.
#[inline(always)]
unsafe fn load_byte(p: *const u8) -> u8 {
    let v;
    // SAFETY: forwarded from the caller.
    unsafe { asm!("mov {v}, byte ptr [{p}]", p = in(reg) p, v = out(reg_byte) v, options(nostack, preserves_flags, readonly)) };
    v
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut c_void, c: i32, n: usize) -> *mut c_void {
    let d = dest.cast::<u8>();
    // SAFETY: `d..d + n` is writable (memset's contract).
    unsafe {
        if n >= SMALL {
            asm!("rep stosb", inout("rcx") n => _, inout("rdi") d => _, in("al") c as u8, options(nostack, preserves_flags));
            return dest;
        }
        let word = u64::from_ne_bytes([c as u8; 8]);
        let words = n / 8;
        (0..words).for_each(|i| store(d.add(8 * i), word));
        (8 * words..n).for_each(|i| store_byte(d.add(i), c as u8));
    }
    dest
}

/// Copy forward, word by word: right for overlaps with `d` below `s`.
///
/// # Safety
/// `s..s + n` is readable and `d..d + n` writable.
#[inline(always)]
unsafe fn forward(d: *mut u8, s: *const u8, n: usize) {
    // SAFETY: forwarded from the caller.
    unsafe {
        if n >= SMALL {
            asm!("rep movsb", inout("rcx") n => _, inout("rdi") d => _, inout("rsi") s => _, options(nostack, preserves_flags));
            return;
        }
        let words = n / 8;
        (0..words).for_each(|i| store(d.add(8 * i), load(s.add(8 * i))));
        (8 * words..n).for_each(|i| store_byte(d.add(i), load_byte(s.add(i))));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    // SAFETY: memcpy's contract: disjoint, readable and writable.
    unsafe { forward(dest.cast(), src.cast(), n) };
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let (d, s) = (dest.cast::<u8>(), src.cast::<u8>());
    // SAFETY: memmove's contract: readable and writable, maybe overlapping.
    unsafe {
        if d.addr().wrapping_sub(s.addr()) >= n {
            // Below the source, or past its end: forward is safe.
            forward(d, s, n);
        } else {
            // Into the source's end: backward, the tail bytes first.
            let words = n / 8;
            (8 * words..n).rev().for_each(|i| store_byte(d.add(i), load_byte(s.add(i))));
            (0..words).rev().for_each(|i| store(d.add(8 * i), load(s.add(8 * i))));
        }
    }
    dest
}
