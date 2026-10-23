#!/usr/bin/env python3
"""Compile/freeze/instantiate/first-use/hot SRFI timings, separately.

Snapshot reuse is an existing in-process cache, NOT the proposed file cache.
All configurations use the same binary; rotate their order between samples.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import statistics
import subprocess
import tempfile
import time
from itertools import combinations
from pathlib import Path

PASSES = ["0", "bindings,local,scalar,dead", "all"]
WORKLOADS = ["list", "vector", "set"]
METRICS = ["compile_us", "freeze_us", "world_us", "first_us", "hot_us"]


def execute(command, env, label):
    """A stalled child must fail the campaign, naming what it was running."""
    try:
        return subprocess.run(command, env=env, check=True, text=True, capture_output=True, timeout=180)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"timed out: {label} with TECHNE_IR={env['TECHNE_IR']}") from error


def gc_counter(stderr, key):
    match = re.search(rf"{key}: (\d+)", stderr)
    if match is None:
        raise RuntimeError(f"missing GC counter {key}: {stderr}")
    return int(match.group(1))


def applications(args):
    """Existing editor workloads and the previously regressing GC workload."""
    data = {"startup": [], "keys": [], "gc": []}
    for sample in range(11):
        order = PASSES[sample % 3:] + PASSES[:sample % 3]
        for passes in order:
            env = {**os.environ, "TECHNE_IR": passes, "TECHNE_JIT": "1000"}
            for name, binary in [("startup", args.startup), ("keys", args.keys)]:
                if binary is None or (name == "keys" and sample >= 3):
                    continue
                start = time.perf_counter_ns()
                result = execute([str(binary.resolve())], env, name)
                data[name].append({"sample": sample, "passes": passes,
                                   "wall_ns": time.perf_counter_ns() - start, "stdout": result.stdout})
    if args.vm:
        bench = Path(__file__).resolve().parent.parent
        source = (bench / "prelude/techne.scm").read_text() + (bench / "programs/callbacks.scm").read_text()
        with tempfile.TemporaryDirectory(prefix="techne-ir-gc-") as directory:
            file = Path(directory) / "callbacks.scm"
            file.write_text(source)
            subsets = [",".join(xs) for size in (2, 3) for xs in combinations(["bindings", "local", "scalar", "dead"], size)]
            for passes in [*PASSES, "bindings", "local", "contify", "scalar", "cse", "licm", "dead", "hoist", *subsets]:
                env = {**os.environ, "TECHNE_IR": passes, "TECHNE_JIT": "1000",
                       "TECHNE_JIT_SYNC": "1", "TECHNE_GC_STATS": "1"}
                result = execute([str(args.vm.resolve()), str(file)], env, "callbacks GC")
                stats = {key: gc_counter(result.stderr, key)
                         for key in ["max_copied_words", "max_slice_words"]}
                data["gc"].append({"passes": passes, **stats, "stdout": result.stdout, "stderr": result.stderr})
        assert len({row["stdout"] for row in data["gc"]}) == 1, "callback oracles differ"
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/examples/ir_cache"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--size", type=int, default=128)
    parser.add_argument("--repeats", type=int, default=30)
    parser.add_argument("--worlds", type=int, default=5)
    parser.add_argument("--startup", type=Path)
    parser.add_argument("--keys", type=Path)
    parser.add_argument("--vm", type=Path, help="also measure callback GC-pause work")
    args = parser.parse_args()
    assert min(args.samples, args.size, args.repeats, args.worlds) > 0
    binary = args.binary.resolve()
    rows = []
    for sample in range(args.samples):
        order = PASSES[sample % len(PASSES):] + PASSES[:sample % len(PASSES)]
        for workload in WORKLOADS:
            for jit in ["0", "1"]:
                for passes in order:
                    env = {**os.environ, "TECHNE_IR": passes, "TECHNE_JIT": jit}
                    result = execute(
                        [str(binary), workload, str(args.size), str(args.repeats), str(args.worlds)],
                        env, f"{workload} (TECHNE_JIT={jit})",
                    )
                    try:
                        row = json.loads(result.stdout)
                    except json.JSONDecodeError as error:
                        raise RuntimeError(f"invalid benchmark output: {result.stdout!r}") from error
                    row.update(sample=sample, jit=jit, passes=passes)
                    rows.append(row)
    summary = []
    for workload in WORKLOADS:
        for jit in ["0", "1"]:
            for passes in PASSES:
                group = [r for r in rows if (r["workload"], r["jit"], r["passes"]) == (workload, jit, passes)]
                summary.append({
                    "workload": workload, "jit": jit, "passes": passes,
                    **{metric: statistics.median(r[metric] for r in group) for metric in METRICS},
                })
    artifact = {
        "cache": "existing in-process Snapshot, not persistent file cache or lazy loading",
        "machine": platform.platform(),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "samples": args.samples, "rows": rows, "summary": summary,
        "applications": applications(args),
        "application_hashes": {name: hashlib.sha256(binary.read_bytes()).hexdigest()
                               for name, binary in [("startup", args.startup), ("keys", args.keys), ("vm", args.vm)] if binary},
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(artifact, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
