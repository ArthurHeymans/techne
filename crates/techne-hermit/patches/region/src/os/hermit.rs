//! Hermit's memory management system calls (the kernel's `mman` feature).
//! Not POSIX: no placement, no query of mappings.

use crate::{Error, Protection, Region, Result};
use std::io;

unsafe extern "C" {
  fn sys_mmap(size: usize, prot_flags: u32, ret: &mut *mut u8) -> i32;
  fn sys_munmap(ptr: *mut u8, size: usize) -> i32;
  fn sys_mprotect(ptr: *mut u8, size: usize, prot_flags: u32) -> i32;
}

fn check(r: i32) -> Result<()> {
  if r == 0 {
    Ok(())
  } else {
    Err(Error::SystemCall(io::Error::from_raw_os_error(r.abs())))
  }
}

/// Region's READ/WRITE/EXECUTE bits are Hermit's.
fn native(protection: Protection) -> u32 {
  protection.bits() as u32
}

pub fn page_size() -> usize {
  4096
}

pub unsafe fn alloc(base: *const (), size: usize, protection: Protection) -> Result<*const ()> {
  if !base.is_null() {
    return Err(Error::SystemCall(io::ErrorKind::Unsupported.into()));
  }
  let mut p = std::ptr::null_mut();
  check(sys_mmap(size, native(protection), &mut p))?;
  Ok(p as *const ())
}

pub unsafe fn free(base: *const (), size: usize) -> Result<()> {
  check(sys_munmap(base as *mut u8, size))
}

pub unsafe fn protect(base: *const (), size: usize, protection: Protection) -> Result<()> {
  check(sys_mprotect(base as *mut u8, size, native(protection)))
}

pub fn lock(_: *const (), _: usize) -> Result<()> {
  Ok(())
}

pub fn unlock(_: *const (), _: usize) -> Result<()> {
  Ok(())
}

pub struct QueryIter;

impl QueryIter {
  pub fn new(_: *const (), _: usize) -> Result<Self> {
    Err(Error::SystemCall(io::ErrorKind::Unsupported.into()))
  }

  pub fn upper_bound(&self) -> usize {
    0
  }
}

impl Iterator for QueryIter {
  type Item = Result<Region>;

  fn next(&mut self) -> Option<Self::Item> {
    None
  }
}
