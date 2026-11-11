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


# Each fixture gets a fresh HOME, so without this the Convex CLI downloads the
# backend binary and dashboard from GitHub releases, and asks version.convex.dev
# for the latest version, on every scene. A network blip at any of those calls
# crashed the CLI before readiness. The cache holds only those downloads, never
# deployment state, so one job can share it across its fixtures.
def convex_cache_dir(profile):
    if os.name == "nt":
        return Path(profile) / "AppData/Local/convex"
    return Path(profile) / ".cache/convex"


def cached_backend_version(cache):
    """Return the version a complete binary and dashboard cache serves, if any."""
    try:
        version = json.loads((Path(cache) / "dashboard/config.json").read_text(encoding="utf-8"))["version"]
    except (OSError, ValueError, KeyError, TypeError):
        return None
    name = "convex-local-backend.exe" if os.name == "nt" else "convex-local-backend"
    if (not isinstance(version, str) or not version or "/" in version or "\\" in version
            or not (Path(cache) / "binaries" / version / name).is_file()
            or not (Path(cache) / "dashboard/out").is_dir()):
        return None
    return version


def restore_convex_cache(shared, cache):
    """Seed a fixture's private cache from the job cache; return its version."""
    version = cached_backend_version(shared)
    if version is None:
        return None
    try:
        shutil.copytree(shared, cache, dirs_exist_ok=True)
    except OSError:
        shutil.rmtree(cache, ignore_errors=True)
        return None  # Fall back to the CLI's own download.
    return version if cached_backend_version(cache) == version else None


def save_convex_cache(cache, shared):
    """Keep the first complete download for the job's later fixtures."""
    if cached_backend_version(shared) is not None or cached_backend_version(cache) is None:
        return
    staging = Path(tempfile.mkdtemp(prefix="buddy-convex-cache-", dir=Path(shared).parent))
    try:
        shutil.copytree(cache, staging / "cache")
        shutil.rmtree(shared, ignore_errors=True)
        (staging / "cache").rename(shared)
    finally:
        shutil.rmtree(staging, ignore_errors=True)


class BackendNotReady(RuntimeError):
    """The owned CLI exited or timed out before the fixture API was ready."""


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
        self.cleanup_failed = False
        self.shared_cache = Path(os.environ["RUNNER_TEMP"]) / "buddy-convex-cache"
        self.cache = convex_cache_dir(self.profile)

    def query(self, path):
        body = json.dumps(dict(path=path, args={}, format="json")).encode()
        request = urllib.request.Request(self.url + "/api/query", data=body, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=2) as response:
            result = json.loads(response.read(1024 * 1024))
        if result.get("status") != "success":
            raise RuntimeError("Fixture functions are not ready")
        return result["value"]

    def __enter__(self):
        # Convex 1.48 still asks version.convex.dev for the latest backend on
        # every new anonymous deployment, and treats a failure as fatal, so a
        # restored cache cannot make startup fully offline. One more attempt
        # absorbs a transient network failure. It starts from a fresh profile
        # (keeping the first attempt's log) and only after the first attempt's
        # cleanup left nothing behind.
        try:
            return self._start()
        except BackendNotReady as failure:
            if self.cleanup_failed or self.generated.exists() or self.local_state.exists():
                raise
            first_log = self.profile / "backend.log"
            self.__init__(self.checkout)
            try:
                return self._start()
            except BaseException as retry_failure:
                if hasattr(retry_failure, "add_note"):
                    retry_failure.add_note(f"First attempt also failed ({failure}); its log: {first_log}")
                raise

    def _start(self):
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
            version = restore_convex_cache(self.shared_cache, self.cache)
            pinned = ["--local-backend-version", version] if version else []
            self.child = subprocess.Popen([node, str(self.checkout / "node_modules/convex/bin/main.js"), "dev", *pinned,
                "--local-cloud-port", str(cloud_port), "--local-site-port", str(site_port)],
                cwd=self.checkout, env=environment, stdin=subprocess.DEVNULL, stdout=self.log, stderr=subprocess.STDOUT, **options)
            self.owns_local_state = True
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                if self.generated.exists() and self.generated_hash is None:
                    self.generated_hash = hashlib.sha256(self.generated.read_bytes()).digest()
                if self.child.poll() is not None:
                    raise BackendNotReady("The owned anonymous backend exited before readiness")
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
            raise BackendNotReady("The owned anonymous backend did not become ready")
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
        if cleanup_error is None:
            try:
                # The CLI has stopped, so a complete download can be shared.
                save_convex_cache(self.cache, self.shared_cache)
            except OSError:
                pass  # Sharing only saves time; the next fixture downloads again.
            shutil.rmtree(self.cache, ignore_errors=True)
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
        self.cleanup_failed = cleanup_error is not None
        if cleanup_error is not None:
            if error is None:
                raise cleanup_error
            if hasattr(error, "add_note"):
                error.add_note("Owned backend cleanup also failed")
        return False
