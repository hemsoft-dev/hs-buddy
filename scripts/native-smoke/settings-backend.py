"""Anonymous Convex backend owned by a disposable native-settings CI fixture."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request


class SettingsBackend:
    def __init__(self, checkout):
        if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
            raise RuntimeError("Settings backend requires disposable GitHub Actions fixtures")
        self.checkout = Path(checkout).resolve()
        self.child, self.log, self.generated_hash = None, None, None
        self.profile = Path(tempfile.mkdtemp(prefix="buddy-settings-backend-", dir=os.environ["RUNNER_TEMP"]))
        self.url = None
        self.generated = self.checkout / ".env.local"
        self.local_state = self.checkout / ".convex" / "local" / "default"
        self.owns_local_state = False

    def query(self, path):
        body = json.dumps(dict(path=path, args={}, format="json")).encode()
        request = urllib.request.Request(self.url + "/api/query", data=body, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=2) as response:
            result = json.loads(response.read(1024 * 1024))
        if result.get("status") != "success":
            raise RuntimeError("Fixture functions are not ready")
        return result["value"]

    def __enter__(self):
        try:
            for name in (".env", ".env.local"):
                if (self.checkout / name).exists():
                    raise RuntimeError("Preserve existing deployment selectors")
            if (self.local_state.exists() or self.local_state.is_symlink()
                    or not self.local_state.resolve().is_relative_to(self.checkout)):
                raise RuntimeError("Preserve existing local backend state")
            # Consecutive fixtures must not depend on the preceding backend's
            # fixed ports leaving TIME_WAIT. Reserve two distinct fresh ports
            # together, then release them for this owned CLI's startup.
            with socket.socket() as cloud, socket.socket() as site:
                cloud.bind(("127.0.0.1", 0))
                site.bind(("127.0.0.1", 0))
                cloud_port, site_port = cloud.getsockname()[1], site.getsockname()[1]
            self.url = f"http://127.0.0.1:{cloud_port}"
            node = shutil.which("node")
            if not node:
                raise RuntimeError("Node is required for the disposable backend")
            allowed = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "TEMP", "TMP", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "OS"}
            environment = {key: value for key, value in os.environ.items() if key.upper() in allowed}
            for name in ("AppData/Roaming", "AppData/Local", "empty-gh"):
                (self.profile / name).mkdir(parents=True)
            environment.update(HOME=str(self.profile), USERPROFILE=str(self.profile),
                APPDATA=str(self.profile / "AppData/Roaming"), LOCALAPPDATA=str(self.profile / "AppData/Local"),
                GH_CONFIG_DIR=str(self.profile / "empty-gh"), CONVEX_AGENT_MODE="anonymous", NO_COLOR="1")
            self.log = (self.profile / "backend.log").open("w", encoding="utf-8")
            options = dict(creationflags=subprocess.CREATE_NO_WINDOW) if os.name == "nt" else dict(start_new_session=True)
            self.child = subprocess.Popen([node, str(self.checkout / "node_modules/convex/bin/main.js"), "dev", "--local-cloud-port", str(cloud_port), "--local-site-port", str(site_port)],
                cwd=self.checkout, env=environment, stdin=subprocess.DEVNULL, stdout=self.log, stderr=subprocess.STDOUT, **options)
            self.owns_local_state = True
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                if self.generated.exists() and self.generated_hash is None:
                    self.generated_hash = hashlib.sha256(self.generated.read_bytes()).digest()
                if self.child.poll() is not None:
                    raise RuntimeError("The owned anonymous backend exited before readiness")
                try:
                    self.query("settings:get")
                    accounts = self.query("githubAccounts:list")
                except (OSError, ValueError, KeyError, RuntimeError):
                    time.sleep(0.25)
                    continue
                if accounts != []:
                    raise RuntimeError("The disposable account list is not empty")
                if self.generated_hash is None:
                    raise RuntimeError("The anonymous backend did not write its deployment selector")
                # The CLI writes the selector more than once during initial
                # setup. API readiness can precede its finished-deploy log.
                # Take ownership of the completed file, then keep the strict
                # unchanged-file cleanup guard for the entire UI exercise.
                if "Convex functions ready!" not in (self.profile / "backend.log").read_text(encoding="utf-8", errors="replace"):
                    time.sleep(0.25)
                    continue
                self.generated_hash = hashlib.sha256(self.generated.read_bytes()).digest()
                return self
            raise RuntimeError("The owned anonymous backend did not become ready")
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, kind, error, traceback):
        cleanup_error = None
        try:
            if self.child is not None:
                if os.name == "nt":
                    if self.child.poll() is None:
                        result = subprocess.run(["taskkill", "/PID", str(self.child.pid), "/T", "/F"], capture_output=True, timeout=20,
                            creationflags=subprocess.CREATE_NO_WINDOW)
                        if result.returncode != 0 and self.child.poll() is None:
                            raise RuntimeError("Owned backend process-tree cleanup failed")
                else:
                    try:
                        os.killpg(self.child.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                try:
                    self.child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    if os.name != "nt":
                        os.killpg(self.child.pid, signal.SIGKILL)
                    else:
                        self.child.kill()
                    self.child.wait(timeout=10)
        except BaseException as failure:
            cleanup_error = failure
        finally:
            if self.log is not None:
                self.log.close()
        if self.generated_hash is not None and self.generated.exists():
            if hashlib.sha256(self.generated.read_bytes()).digest() == self.generated_hash:
                self.generated.unlink()
            else:
                cleanup_error = RuntimeError("Generated fixture environment changed; preserve it")
        if cleanup_error is None and self.owns_local_state and self.local_state.exists():
            try:
                # Convex stores anonymous databases in the checkout, even with
                # an isolated HOME. Preserve our completed database separately
                # so the next fixture starts empty, without deleting user data.
                source = self.local_state.resolve()
                destination = (self.profile / "backend-state").resolve()
                if (not source.is_relative_to(self.checkout)
                        or not destination.is_relative_to(self.profile.resolve())
                        or destination.exists()):
                    raise RuntimeError("Owned backend state boundaries changed; preserve it")
                source.rename(destination)
                self.owns_local_state = False
            except BaseException as failure:
                cleanup_error = failure
        if cleanup_error is not None:
            if error is None:
                raise cleanup_error
            if hasattr(error, "add_note"):
                error.add_note("Owned backend cleanup also failed")
        return False
