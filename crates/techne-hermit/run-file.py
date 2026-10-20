#!/usr/bin/env python3
"""Run a Scheme file in the unikernel, like `techne-vm FILE ARGS...`.

QEMU hands the guest an archive (fw_cfg) of the file, the current
directory and the directories of TECHNE_LIBRARY_PATH, at their paths on
the host; the guest runs the file there. The program's output (stdout and
stderr, both on the serial console) comes back on stdout and its exit
status is ours. TECHNE_* variables are passed to the guest. Meant as
TECHNE_VM_RUNNER for crates/techne-vm/tests/suites.rs.
"""
import ctypes
import os
import signal
import struct
import subprocess
import sys
import tempfile

# The runner's own and the test suites' locations on the host: the guest
# has no use for them, and Hermit's command line is short.
HOST_ONLY = {"TECHNE_VM_RUNNER", "TECHNE_R7RS_TESTS", "TECHNE_R7RS_BENCHMARKS", "TECHNE_SRFI_SOURCES"}
HERE = os.path.dirname(os.path.abspath(__file__))
# A directory larger than this is not sent (a home directory, /tmp).
MAX_DIR = 64 << 20
# What the application prints first.
START = "Jumping into application"


def die_with_parent():
    """QEMU ends with us, even killed: the suites kill us on a timeout."""
    PR_SET_PDEATHSIG = 1
    ctypes.CDLL(None, use_errno=True).prctl(PR_SET_PDEATHSIG, signal.SIGKILL)


def dir_files(top):
    """The directories and files under TOP, following symbolic links, or
    none if too many."""
    files, size = [], 0
    for root, dirs, names in os.walk(top, followlinks=True):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        files.append(root)
        for name in names:
            path = os.path.join(root, name)
            try:
                size += os.path.getsize(path)
            except OSError:
                continue
            if size > MAX_DIR:
                print(f"run-file: {top} is too large to send", file=sys.stderr)
                return []
            files.append(path)
    return files


# The size of a directory's entry.
DIRECTORY = 2**64 - 1


def archive(paths, out):
    for path in dict.fromkeys(paths):
        name = path.encode()
        if os.path.isdir(path):
            out.write(struct.pack("<I", len(name)) + name + struct.pack("<Q", DIRECTORY))
            continue
        try:
            with open(path, "rb") as f:
                data = f.read()
        except OSError:
            continue
        out.write(struct.pack("<I", len(name)) + name + struct.pack("<Q", len(data)) + data)


def main():
    path, args = os.path.abspath(sys.argv[1]), sys.argv[2:]
    cwd = os.getcwd()
    libs = [d for d in os.environ.get("TECHNE_LIBRARY_PATH", "").split(":") if d]
    files = [path] + dir_files(cwd) + [f for d in libs for f in dir_files(os.path.abspath(d))]
    env = [f"env={k}={v}" for k, v in os.environ.items() if k.startswith("TECHNE_") and k not in HOST_ONLY]
    with tempfile.NamedTemporaryFile(prefix="techne-files-") as files_archive:
        archive(files, files_archive)
        files_archive.flush()
        qemu = subprocess.Popen(
            [os.path.join(HERE, "run.sh")] + env + ["--", "--cwd", cwd, path] + args,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            preexec_fn=die_with_parent,
            env={**os.environ, "QEMU_ARGS": f"-fw_cfg name=opt/techne/files,file={files_archive.name}"},
        )
        for line in qemu.stdout:
            if START in line.decode(errors="replace"):
                break
        out = sys.stdout.buffer
        code = None
        for line in qemu.stdout:
            # The kernel's statistics at exit, then the status the program
            # exited with: QEMU's own (isa-debug-exit) tells only success.
            if line.startswith(b"Number of interrupts"):
                for rest in qemu.stdout:
                    if rest.startswith(b"exit status "):
                        code = int(rest.split()[2])
                break
            out.write(line)
        out.flush()
        status = qemu.wait()
    if code is None:
        code = 0 if status == 3 else 1
    sys.exit(code & 0xFF)


main()
