#!/usr/bin/env bash
# Concatenate prelude + program for each Scheme implementation into build/<impl>/.
set -euo pipefail
cd "$(dirname "$0")"
for impl in steel steel-resumable techne chez guile; do
  mkdir -p build/$impl
  for p in programs/*.scm; do
    cat prelude/$impl.scm "$p" > build/$impl/$(basename "$p")
  done
done
cp org-sample.org build/
