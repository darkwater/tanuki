#!/usr/bin/env python3
"""Explicit POSIX PTY smoke test; run after cargo build -p tanuki -p tanuki-ui.
Uses real binaries, HTTP production writes, and real terminal attributes.
Separate from headless Cargo CI. No Python dependencies beyond the stdlib.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
UI = ROOT / "target/debug/tanuki-ui"
SERVER = ROOT / "target/debug/tanuki"


def read_until(master, output, expected):
    deadline = time.monotonic() + 8
    while expected not in output:
        remaining = deadline - time.monotonic()
        assert remaining > 0, f"missing terminal output {expected!r}"
        assert select.select([master], [], [], remaining)[0], expected
        chunk = os.read(master, 65536)
        assert chunk, f"terminal ended before {expected!r}"
        output += chunk
    return output


def smoke(command, exit_mode="q", full_output=False):
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 140, 0, 0))
    output_file = open("/dev/full", "wb") if full_output else None
    process = subprocess.Popen(command, cwd=ROOT, stdin=slave,
                               stdout=output_file or slave, stderr=slave)
    output = b""
    try:
        if exit_mode == "panic" or full_output:
            process.wait(timeout=8)
            assert process.returncode != 0
        else:
            output = read_until(master, output, b"LIVE")
            assert b"state" in output
            if exit_mode == "q":
                os.write(master, b"?")
                output = read_until(master, output, b"Read-only")
                os.write(master, b"?/hue\r\x1b[B")
                output = read_until(master, output, b"/hue")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 4, 30, 0, 0))
                os.kill(process.pid, signal.SIGWINCH)
                output = read_until(master, output, b"Resize")
                os.write(master, b"q")
            elif exit_mode == "key":
                os.write(master, b"\x03")
            else:
                os.kill(process.pid, signal.SIGINT)
            process.wait(timeout=8)
            assert process.returncode == 0, process.returncode
        assert termios.tcgetattr(slave) == before, "raw mode was not restored"
        if not full_output:
            output = read_until(master, output, b"\x1b[?1049l")
            output = read_until(master, output, b"\x1b[?25h")
        print(f"PASS: {exit_mode if not full_output else 'setup-output-error'}; termios restored")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        if output_file:
            output_file.close()
        os.close(master)
        os.close(slave)


def main():
    with tempfile.TemporaryDirectory(prefix="tanuki-terminal-smoke-") as directory:
        env = dict(os.environ, TANUKI_LISTEN="127.0.0.1:0",
                   TANUKI_SNAPSHOT=f"{directory}/snapshot.db", NO_COLOR="1")
        server = subprocess.Popen([SERVER], cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT)
        try:
            assert select.select([server.stdout], [], [], 8)[0], "server did not start"
            address = re.search(rb"127\.0\.0\.1:\d+", server.stdout.readline()).group().decode()
            url = f"ws://{address}/v1/ws"
            for path, value in [("lamp", 1), ("lamp/hue", 120)]:
                request = urllib.request.Request(f"http://{address}/v1/state/{path}",
                    data=json.dumps({"value": value}).encode(),
                    headers={"Content-Type": "application/json", "tanuki-client": "PTY producer"})
                with urllib.request.urlopen(request, timeout=8) as response:
                    assert response.status == 200
            command = [UI, "--url", url]
            smoke(command)
            smoke(command, "key")
            smoke(command, "signal")
            smoke(command, full_output=True)
            # Exercise the production owning guard during real Rust unwinding,
            # without a panic switch or panic-catching wrapper in the application.
            probe = Path(directory) / "probe"
            (probe / "src").mkdir(parents=True)
            (probe / "Cargo.toml").write_text(
                '[package]\nname="tanuki-terminal-unwind-probe"\nversion="0.0.0"\nedition="2024"\n'
                '[dependencies]\ntanuki-ui={path=' + json.dumps(str(ROOT / "tanuki-ui")) + '}\n')
            (probe / "src/main.rs").write_text('fn main() { let _guard = tanuki_ui::terminal::TerminalGuard::enter().unwrap(); panic!("terminal unwind smoke probe"); }')
            subprocess.run(["cargo", "build", "--offline", "--manifest-path", str(probe / "Cargo.toml"),
                            "--target-dir", str(ROOT / "target")], cwd=ROOT, check=True)
            executable = ROOT / "target/debug/tanuki-terminal-unwind-probe"
            smoke([executable], "panic")
        finally:
            server.send_signal(signal.SIGINT)
            server.wait(timeout=8)
            assert server.returncode == 0


if __name__ == "__main__":
    main()
