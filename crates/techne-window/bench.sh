#!/usr/bin/env bash
# The slice 2 latency benchmarks (PLAN.md, Stage 1): a 100k-line file, a
# file with one 1 MB line and mixed-script Unicode, then the first again
# with a busy Lisp task. Each types 300 keys and reports key-to-frame
# latency. Run on a quiet machine, in `nix develop .#window`:
#
#   crates/techne-window/bench.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --release -p techne-window
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
python3 - "$dir" <<'PY'
import random, sys
d = sys.argv[1]
random.seed(1)
words = ["alpha", "beta", "gamma", "delta", "println!", "fn", "let", "mut", "struct", "impl"]
with open(f"{d}/lines.txt", "w") as f:
    for _ in range(100000):
        f.write(" ".join(random.choice(words) for _ in range(random.randint(0, 14))) + "\n")
with open(f"{d}/longline.txt", "w") as f:
    line = "".join(random.choice(words) + " " for _ in range(260000))[: 1 << 20]
    f.write("start\n" + line + "\nend\n")
samples = ["日本語のテキスト", "Ελληνικά κείμενο", "emoji 👩‍💻🇧🇪 mixed", "e\u0301 combining",
           "עברית טקסט", "한국어 텍스트", "ascii text here"]
with open(f"{d}/unicode.txt", "w") as f:
    for _ in range(20000):
        f.write(" ".join(random.choice(samples) for _ in range(random.randint(1, 6))) + "\n")
PY
run() { crates/techne-window/headless.sh ./target/release/techne --bench 300 "$@" 2>&1 | grep 'key to'; }
for f in lines longline unicode; do
    echo "$f:"
    run "$dir/$f.txt"
done
echo "lines, with a busy Lisp task:"
run --load "$dir/lines.txt"
