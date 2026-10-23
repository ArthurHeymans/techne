#!/usr/bin/env bash
# Boot the unikernel in QEMU, the serial console on stdio. Arguments are
# Hermit's command line, e.g. `./run.sh env=TECHNE_JIT=0` or
# `./run.sh -- /lisp/tests/foo.scm`. QEMU=... and QEMU_ARGS=... override.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
loader=${HERMIT_LOADER:-$here/target/hermit-loader-x86_64-multiboot}
if [[ ! -f $loader ]]; then
  mkdir -p "$(dirname "$loader")"
  curl -fsSL -o "$loader" \
    https://github.com/hermit-os/loader/releases/download/v0.5.7/hermit-loader-x86_64-multiboot
fi
accel=(-cpu Skylake-Client)
freq=()
if [[ -w /dev/kvm ]]; then
  accel=(-enable-kvm -cpu host)
  # Hermit takes the TSC's frequency from the CPU's name, which may be
  # another (its clock then runs fast or slow): the host's, where its
  # kernel told it. HERMIT_FREQ (MHz) overrides.
  mhz=${HERMIT_FREQ:-$( (journalctl -k -b 2>/dev/null || dmesg 2>/dev/null) |
    sed -n 's/.*tsc: Detected \([0-9]*\)\.[0-9]* MHz TSC.*/\1/p' | tail -1)}
  [[ -n $mhz ]] && freq=(-freq "$mhz")
fi
exec "${QEMU:-qemu-system-x86_64}" "${accel[@]}" -smp ${SMP:-2} -m ${MEM:-8G} \
  -display none -serial stdio \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -kernel "$loader" \
  -initrd "$here/target/x86_64-unknown-hermit/release/techne-hermit" \
  -append "${freq[*]} $*" ${QEMU_ARGS:-}
