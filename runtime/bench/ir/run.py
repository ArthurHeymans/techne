#!/usr/bin/env python3
"""Measure paired source transformations; no production optimizer switches.

Run inside `nix shell nixpkgs#valgrind --command ...` for instruction counts.
Only the Rust driver's timed_kernel is counted, not startup or compilation.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def execute(command, env):
    return subprocess.run(
        command, env=env, check=True, text=True, capture_output=True, timeout=180
    )


def environment(mode, passes=None):
    # Do not inherit a different nursery, GC stress, JIT dumping, etc.
    env = {k: v for k, v in os.environ.items() if not k.startswith("TECHNE_")}
    env.update(TECHNE_JIT="1" if mode == "jit" else "0", TECHNE_JIT_SYNC="1")
    if passes is not None:
        env["TECHNE_IR"] = passes
    return env


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/examples/ir")
    parser.add_argument("--metric", choices=["wall", "instructions"], default="wall")
    parser.add_argument("--modes", nargs="+", choices=["jit", "interp"], default=["jit", "interp"])
    parser.add_argument("--cases", nargs="+")
    parser.add_argument("--passes", help="explicit TECHNE_IR pass selection; omit for the build's default")
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--iterations", type=int, default=20000)
    parser.add_argument("--repeats", type=int, default=10)
    parser.add_argument("--cpu", type=int, default=4)
    parser.add_argument("--revision", required=True, help="revision of the runtime being measured")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.samples < 1 or args.repeats < 1 or not 1 <= args.iterations <= 20000:
        parser.error("samples/repeats must be positive; iterations must be 1..20000")
    binary = args.binary.resolve()
    cases = execute([str(binary), "--list"], environment("interp", args.passes)).stdout.splitlines()
    if args.cases:
        if not set(args.cases) <= set(cases):
            parser.error("unknown case")
        cases = args.cases
    if args.metric == "instructions" and not shutil.which("valgrind"):
        parser.error("valgrind not on PATH; use nix shell nixpkgs#valgrind --command ...")
    metadata = {
        "revision": args.revision,
        "passes": args.passes if args.passes is not None else "default",
        "date": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "platform": platform.platform(),
        "cpu": args.cpu,
        "affinity": sorted(os.sched_getaffinity(0)),
        "cpuinfo": Path("/proc/cpuinfo").read_text().split("\n\n")[0],
        "binary_sha256": digest(binary),
        "kernels_sha256": digest(Path(__file__).with_name("kernels.scm")),
        "driver_sha256": digest(ROOT / "crates/techne-vm/examples/ir.rs"),
        "metric": args.metric,
        "iterations": args.iterations,
        "repeats": args.repeats,
        "jit": "1 (synchronous, first-call compilation)",
        "units": "ns/iteration" if args.metric == "wall" else "instructions/iteration",
    }
    if args.metric == "instructions":
        metadata["valgrind"] = execute(["valgrind", "--version"], os.environ).stdout.strip()
    records = []
    args.out.parent.mkdir(parents=True, exist_ok=True)

    def save():
        temporary = args.out.with_suffix(".tmp")
        # Keep every sample, but use one record per line for readable diffs.
        header = json.dumps(metadata, indent=2)
        rows = ",\n".join("    " + json.dumps(record) for record in records)
        temporary.write_text('{\n  "metadata": ' + header + ',\n  "records": [\n' + rows + '\n  ]\n}\n')
        temporary.replace(args.out)

    for mode in args.modes:
        env = environment(mode, args.passes)
        execute([str(binary), "--check"], env)
        for case in cases:
            for sample in range(args.samples):
                # Alternate before/after order to reduce drift bias. Each
                # process separately warms its kernel before starting timing.
                variants = ["before", "after"] if sample % 2 == 0 else ["after", "before"]
                for variant in variants:
                    command = ["taskset", "-c", str(args.cpu)]
                    if args.metric == "instructions":
                        command += [
                            "valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null",
                            "--collect-atstart=no", "--toggle-collect=*timed_kernel*",
                        ]
                    command += [str(binary), case, variant, str(args.iterations), str(args.repeats)]
                    completed = execute(command, env)
                    try:
                        record = json.loads(completed.stdout)
                    except json.JSONDecodeError as error:
                        raise RuntimeError(f"invalid driver output: {completed.stdout!r}") from error
                    assert (record["case"], record["variant"], record["iterations"], record["repeats"]) == (
                        case, variant, args.iterations, args.repeats
                    )
                    record.update(mode=mode, sample=sample)
                    if args.metric == "instructions":
                        match = re.search(r"Collected\s*:\s*([\d,]+)", completed.stderr)
                        if not match:
                            raise RuntimeError(f"no timed_kernel instructions collected: {completed.stderr}")
                        try:
                            count = int(match[1].replace(",", ""))
                        except ValueError as error:
                            raise RuntimeError(f"invalid Callgrind count: {match[1]!r}") from error
                        if count <= 0:
                            raise RuntimeError("empty timed_kernel instruction count")
                        record["instructions"] = count
                    records.append(record)
                    save()
            field = "elapsed_ns" if args.metric == "wall" else "instructions"
            medians = {
                v: statistics.median(r[field] for r in records if r["mode"] == mode and r["case"] == case and r["variant"] == v)
                / (args.iterations * args.repeats)
                for v in ["before", "after"]
            }
            print(f"{mode:6} {case:18} {medians['before']:9.2f} -> {medians['after']:9.2f} "
                  f"{metadata['units']}  {medians['before'] / medians['after']:6.2f}x", flush=True)
    save()


if __name__ == "__main__":
    main()
