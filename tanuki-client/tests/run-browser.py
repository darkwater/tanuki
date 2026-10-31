#!/usr/bin/env python3
"""Run the WASM SDK in a browser against an isolated production server.

Prerequisites: wasm32-unknown-unknown, matching wasm-bindgen-cli, and a browser
plus WebDriver on PATH (or the runner's CHROMEDRIVER/GECKODRIVER environment).
Optional arguments are passed to cargo test, e.g. --release or a test filter.
"""
import json
import os
from pathlib import Path
import queue
import re
import signal
import socketserver
import subprocess
import sys
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]


class StalledHandshake(socketserver.BaseRequestHandler):
    def handle(self):
        # A deliberately silent peer for timeout injection, not a fake Tanuki server.
        self.request.settimeout(30)
        try:
            while self.request.recv(4096):
                pass
        except TimeoutError:
            pass


class StalledServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


def main():
    release = "--release" in sys.argv[1:]
    build = ["cargo", "build", "-p", "tanuki", "--bin", "tanuki", "--locked"]
    if release:
        build.append("--release")
    subprocess.run(build, cwd=ROOT, check=True)
    metadata = subprocess.check_output(["cargo", "metadata", "--format-version=1", "--no-deps"], cwd=ROOT, text=True)
    binary = Path(json.loads(metadata)["target_directory"]) / ("release" if release else "debug") / "tanuki"
    with tempfile.TemporaryDirectory(prefix="tanuki-browser-") as directory, StalledServer(("127.0.0.1", 0), StalledHandshake) as stalled:
        env = os.environ.copy()
        env.update(TANUKI_LISTEN="127.0.0.1:0", TANUKI_SNAPSHOT=str(Path(directory) / "snapshot.json"), NO_COLOR="1")
        if "CHROME_BIN" in env and "WASM_BINDGEN_TEST_WEBDRIVER_JSON" not in env:
            capabilities = Path(directory) / "webdriver.json"
            capabilities.write_text(json.dumps({"goog:chromeOptions": {"binary": env["CHROME_BIN"]}}))
            env["WASM_BINDGEN_TEST_WEBDRIVER_JSON"] = str(capabilities)
        server = subprocess.Popen([str(binary)], cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        addresses = queue.Queue()

        def collect_logs():
            with open(Path(directory) / "server.log", "w") as log:
                for line in server.stdout:
                    log.write(line)
                    if "Tanuki HTTP server listening" in line:
                        match = re.search(r"127\.0\.0\.1:\d+", line)
                        if match:
                            addresses.put(match.group())

        logs = threading.Thread(target=collect_logs, daemon=True)
        logs.start()
        thread = threading.Thread(target=stalled.serve_forever, daemon=True)
        thread.start()
        try:
            address = addresses.get(timeout=20)
            env.update(
                TANUKI_TEST_WS_URL=f"ws://{address}/v1/ws",
                TANUKI_TEST_STALL_URL=f"ws://127.0.0.1:{stalled.server_address[1]}",
                CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="wasm-bindgen-test-runner",
                WASM_BINDGEN_USE_BROWSER="1",
                WASM_BINDGEN_TEST_TIMEOUT="30",
            )
            result = subprocess.run(["cargo", "test", "-p", "tanuki-client", "--target", "wasm32-unknown-unknown", "--lib", "--test", "browser", "--locked", *sys.argv[1:]], cwd=ROOT, env=env)
        finally:
            server.send_signal(signal.SIGINT)
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
            logs.join(timeout=2)
            stalled.shutdown()
            thread.join(timeout=2)
            if server.returncode:
                print((Path(directory) / "server.log").read_text(), file=sys.stderr)
        return result.returncode or server.returncode


if __name__ == "__main__":
    sys.exit(main())
