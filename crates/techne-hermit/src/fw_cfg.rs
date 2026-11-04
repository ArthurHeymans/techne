//! QEMU's firmware configuration device, over its legacy I/O ports: how
//! the host hands the guest a file (`-fw_cfg name=opt/...,file=...`)
//! faster than the serial console types it in. Hermit runs the program in
//! ring 0, so the ports are ours to use.

use std::arch::asm;

const SELECTOR: u16 = 0x510;
const DATA: u16 = 0x511;
const FILE_DIR: u16 = 0x19;

fn select(key: u16) {
    // SAFETY: the fw_cfg selector port; selecting has no other effect.
    unsafe { asm!("out dx, ax", in("dx") SELECTOR, in("ax") key, options(nomem, nostack, preserves_flags)) };
}

/// `rep insb`, which KVM serves a page per exit rather than a byte.
fn read(buf: &mut [u8]) {
    // SAFETY: the fw_cfg data port, which reads the selected item on, into
    // `buf`.
    unsafe {
        asm!("rep insb", inout("rcx") buf.len() => _, inout("rdi") buf.as_mut_ptr() => _, in("dx") DATA, options(nostack, preserves_flags))
    };
}

fn be32(buf: &mut [u8; 4]) -> u32 {
    read(buf);
    u32::from_be_bytes(*buf)
}

/// The contents of the file `name`, if the host gave one.
pub fn file(name: &str) -> Option<Vec<u8>> {
    select(FILE_DIR);
    let count = be32(&mut [0; 4]);
    (0..count).find_map(|_| {
        // size: be32, select: be16, reserved: u16, name: [u8; 56]
        let mut entry = [0u8; 64];
        read(&mut entry);
        let size = u32::from_be_bytes(entry[0..4].try_into().unwrap()) as usize;
        let key = u16::from_be_bytes(entry[4..6].try_into().unwrap());
        let len = entry[8..].iter().position(|&b| b == 0).unwrap_or(56);
        (&entry[8..8 + len] == name.as_bytes()).then_some((key, size))
    })
    .map(|(key, size)| {
        select(key);
        let mut contents = vec![0; size];
        read(&mut contents);
        contents
    })
}
