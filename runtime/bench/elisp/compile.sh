#!/usr/bin/env bash
# Byte-compile and native-compile (speed 2 and 3) every benchmark into ../build/elisp/.
set -euo pipefail
cd "$(dirname "$0")"; O=../build/elisp; mkdir -p $O/s2 $O/s3
for f in *.el; do
  b=${f%.el}; cp $f $O/$f
  emacs -Q --batch -f batch-byte-compile $O/$f 2>&1 | grep -iE "error|warn" || true
  for s in 2 3; do
    emacs -Q --batch --eval "(progn (setq native-comp-speed $s) (native-compile \"$PWD/$f\" \"$PWD/$O/s$s/$b.eln\"))" 2>&1 | grep -iE "error" || true
  done
done
ls $O $O/s2 | head -40
