"""Fixture-only process tap. Receiving observations never replace app assertions."""
import argparse
import ctypes as c
import json
import math
import os
from pathlib import Path
import selectors
import subprocess
import sys
import threading
import time
import uuid
from receipt_clock import stamp

KINDS = {5: "move", 1: "down", 2: "up"}
LIMIT = 4096
CONTROL_SECONDS = 15
HEALTH_LIMIT = 32
FAULTS = frozenset(("timeout-notification", "user-input-notification",
    "unexpected-control", "flag-disabled", "owned-exit", "overflow",
    "callback-failure", "reader-failure", "startup-timeout", "unavailable",
    "cleanup-failure", "health-overflow"))
PHASES = frozenset(("stop-request", "owned-kill", "owned-reap",
    "source-invalidated", "reader-joined"))


def remaining(deadline):
    value = deadline - time.monotonic()
    if value <= 0:
        raise TimeoutError("Owned receive control deadline")
    return value


def canonical_guard(config=None):
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Owned receive requires canonical Mac fixture")
    root = os.environ.get("RUNNER_TEMP")
    config = config if config is not None else os.environ.get("BUDDY_CONFIG_PATH")
    if not root or not config:
        raise RuntimeError("Owned receive requires explicit fixture paths")
    root, config = Path(root).resolve(strict=True), Path(config).resolve(strict=True)
    if not root.is_dir() or not config.is_file() or not config.is_relative_to(root):
        raise RuntimeError("Owned receive requires canonical fixture config")


def target_guard(process, executable, *, window=None, native=False, fixture_config=None):
    canonical_guard(fixture_config)
    expected = Path(executable).resolve(strict=True)
    if not isinstance(process, subprocess.Popen) or process.pid <= 0 or process.poll() is not None:
        raise RuntimeError("Owned receive requires live Popen")
    if Path(process.args[0]).resolve(strict=True) != expected:
        raise RuntimeError("Owned receive executable mismatch")
    if native:
        if os.environ.get("BUDDY_RECEIVER_NATIVE_CHILD") != "fixture":
            raise RuntimeError("Owned receive native child refused")
    elif window is None or window.process is not process or window.session != "macos":
        raise RuntimeError("Owned receive requires the owned visible Mac window")


class Native:
    """Only public PID-specific tap and CF ownership/run-loop functions."""
    def __init__(self):
        self.cg = c.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
        self.cf = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        self.callback_type = c.CFUNCTYPE(c.c_void_p, c.c_void_p, c.c_uint32, c.c_void_p, c.c_void_p)
        signatures = [
            (self.cg, "CGEventTapCreateForPid", c.c_void_p, [c.c_int32, c.c_uint32, c.c_uint32, c.c_uint64, self.callback_type, c.c_void_p]),
            (self.cg, "CGEventGetTimestamp", c.c_uint64, [c.c_void_p]),
            (self.cg, "CGEventTapIsEnabled", c.c_bool, [c.c_void_p]),
            (self.cf, "CFMachPortCreateRunLoopSource", c.c_void_p, [c.c_void_p, c.c_void_p, c.c_long]),
            (self.cf, "CFRunLoopGetCurrent", c.c_void_p, []),
            (self.cf, "CFRunLoopAddSource", None, [c.c_void_p, c.c_void_p, c.c_void_p]),
            (self.cf, "CFRunLoopRunInMode", c.c_int32, [c.c_void_p, c.c_double, c.c_bool]),
            (self.cf, "CFRunLoopStop", None, [c.c_void_p]),
            (self.cf, "CFRunLoopWakeUp", None, [c.c_void_p]),
            (self.cf, "CFRunLoopSourceInvalidate", None, [c.c_void_p]),
            (self.cf, "CFMachPortInvalidate", None, [c.c_void_p]),
            (self.cf, "CFRelease", None, [c.c_void_p]),
        ]
        for library, name, result, args in signatures:
            function = getattr(library, name)
            function.restype, function.argtypes = result, args
        self.mode = c.c_void_p.in_dll(self.cf, "kCFRunLoopDefaultMode").value

    def install(self, pid, callback):
        block = self.callback_type(callback)
        port = self.cg.CGEventTapCreateForPid(pid, 0, 1, sum(1 << k for k in KINDS), block, None)
        if not port:
            return None
        source = self.cf.CFMachPortCreateRunLoopSource(None, port, 0)
        if not source:
            self.cf.CFMachPortInvalidate(port)
            self.cf.CFRelease(port)
            return None
        loop = self.cf.CFRunLoopGetCurrent()
        self.cf.CFRunLoopAddSource(loop, source, self.mode)
        return port, source, loop, block

    def read(self, resource):
        # This bounded CF dispatch wait is independent of the app's main loop.
        self.cf.CFRunLoopRunInMode(self.mode, 0.05, False)
        return bool(self.cg.CGEventTapIsEnabled(resource[0]))

    def wake(self, resource):
        self.cf.CFRunLoopStop(resource[2])
        self.cf.CFRunLoopWakeUp(resource[2])

    def remove(self, resource):
        port, source, _, _ = resource
        self.cf.CFRunLoopSourceInvalidate(source)
        self.cf.CFMachPortInvalidate(port)
        self.cf.CFRelease(source)
        self.cf.CFRelease(port)

    def timestamp(self, event):
        return int(self.cg.CGEventGetTimestamp(event))


class Observer:
    def __init__(self, process, executable, *, window=None, native=False, fixture_config=None, api_factory=Native):
        # Refusal precedes library loading, thread or native resource creation.
        target_guard(process, executable, window=window, native=native, fixture_config=fixture_config)
        self.process, self.api_factory = process, api_factory
        self.rows = []
        self._first, self._transitions, self._once = {}, {}, {}
        self._slots = iter(range(HEALTH_LIMIT))
        self.health_overflow, self.cleanup_failed = False, False
        self.active, self.stopping = False, threading.Event()
        self.admitted, self.admission_deadline = False, 0
        self.admission_lock = threading.Lock()
        self._record_lock = threading.Lock()
        self.ready, self.finished = threading.Event(), threading.Event()
        self.resource, self.api, self.thread = None, None, None

    @property
    def reason(self):
        first = self._first.get("fault")
        return first["category"] if first is not None else None

    def _event(self, kind, category):
        try:
            clock = stamp()
        except BaseException:
            clock = dict(wallMillis=None, monotonicNanos=None)
        slot = next(self._slots, None)
        event = dict(kind=kind, category=category, afterStop=self.stopping.is_set(),
            sequence=slot if slot is not None else HEALTH_LIMIT, clock=clock)
        if slot is None:
            self.health_overflow = True
            overflow = dict(event, kind="fault", category="health-overflow")
            self._first.setdefault("fault", overflow)
            self.active, self.admitted = False, False
        else:
            self._transitions[slot] = event
        if clock["monotonicNanos"] is None:
            self._first.setdefault("fault", dict(event, kind="fault",
                category=category if kind == "fault" else "reader-failure"))
            self.active, self.admitted = False, False
        return event

    def _fault(self, category):
        category = category if category in FAULTS else "unexpected-control"
        # setdefault is the single first-fault commit. Later teardown cannot replace it.
        self._first.setdefault("fault", self._event("fault", category))
        self.active, self.admitted = False, False

    def lifecycle(self, phase):
        if phase not in PHASES:
            raise RuntimeError("Invalid owned lifecycle phase")
        if phase not in ("owned-kill", "owned-reap"):
            # A bounded key set; repeated stop/join is idempotent.
            marker = object()
            if self._once.setdefault(phase, marker) is not marker:
                return
        self._event("lifecycle", phase)

    def callback(self, proxy, kind, event, user):
        # No event ownership is acquired; EVERY branch returns the exact pointer.
        try:
            if not self.active or not self.admitted:
                return event
            if kind not in KINDS:
                self._fault({4294967294: "timeout-notification",
                    4294967295: "user-input-notification"}.get(kind, "unexpected-control"))
                return event
            if self.process.poll() is not None:
                self._fault("owned-exit")
                return event
            if len(self.rows) >= LIMIT:
                self._fault("overflow")
                return event
            timestamp = self.api.timestamp(event)
            if type(timestamp) is not int or timestamp <= 0:
                raise ValueError("Unavailable timestamp")
            # The callback never waits on a lock. Contention invalidates evidence.
            if not self._record_lock.acquire(blocking=False):
                self._fault("callback-failure")
                return event
            try:
                if len(self.rows) >= LIMIT:
                    self._fault("overflow")
                    return event
                if self.active and self.admitted and self.reason is None and not self.stopping.is_set():
                    self.rows.append(dict(phase="receive", kind=KINDS[kind],
                        timestamp=timestamp, wallMillis=time.time_ns() // 1000000,
                        sequence=len(self.rows)))
                    # Stop is a nonblocking signal. Reject an in-flight commit
                    # before quiescence if the signal raced the append.
                    if self.stopping.is_set():
                        self.rows.pop()
            finally:
                self._record_lock.release()
        except BaseException:
            self._fault("callback-failure")
        return event

    def _reader(self):
        resource = None
        try:
            self.api = self.api_factory()
            resource = self.api.install(self.process.pid, self.callback)
            self.resource = resource
            with self.admission_lock, self._record_lock:
                if resource is None:
                    self._fault("unavailable")
                    return
                if self.stopping.is_set() or self.reason is not None or time.monotonic() >= self.admission_deadline:
                    self._fault("startup-timeout")
                    return
                self.active = True
                if self.stopping.is_set() or self.reason is not None:
                    self.active = False
                    return
                self.ready.set()
            while not self.stopping.is_set():
                if self.process.poll() is not None:
                    self._fault("owned-exit")
                    break
                if not self.api.read(resource):
                    self._fault("flag-disabled")
                    break
        except Exception:
            self._fault("reader-failure")
        finally:
            self.active = False
            try:
                if resource is not None:
                    # Run-loop callback has returned on THIS thread before release.
                    self.api.wake(resource)
                    self.api.remove(resource)
                    self.lifecycle("source-invalidated")
            except Exception:
                self.cleanup_failed = True
                self._fault("cleanup-failure")
            self.ready.set()
            self.finished.set()

    def start(self, deadline):
        if self.reason is not None or self.stopping.is_set():
            return False
        self.admission_deadline = deadline
        try:
            remaining(deadline)  # Refuse BEFORE constructing any thread/API.
        except TimeoutError:
            self._fault("startup-timeout")
            self.request_stop()
            self.finished.set()
            raise
        self.thread = threading.Thread(target=self._reader, name="owned-receive", daemon=True)
        self.thread.start()
        ready = self.ready.wait(max(0, deadline - time.monotonic()))
        with self.admission_lock:
            if not ready or time.monotonic() >= deadline:
                self._fault("startup-timeout")
                self.request_stop()
                return False
            with self._record_lock:
                self.admitted = self.active and self.reason is None and not self.stopping.is_set()
                if self.stopping.is_set():
                    self.admitted = False
                return self.admitted

    def request_stop(self):
        self.stopping.set()
        self.active, self.admitted = False, False
        self.lifecycle("stop-request")

    def stop(self, deadline):
        self.request_stop()
        if self.thread is not None:
            self.thread.join(max(0, deadline - time.monotonic()))
            if self.thread.is_alive():
                # Thread retains self/callback/resources. Never free in-flight state.
                self._fault("cleanup-failure")
                raise RuntimeError("Owned receive reader did not quiesce")
            self.lifecycle("reader-joined")
        if self.cleanup_failed:
            raise RuntimeError("Owned receive cleanup failed")
        return self.receipt()

    def receipt(self):
        return dict(phase="observer", status="unqualified", reason=self.reason,
            stopped=self.finished.is_set(), received=list(self.rows), coverageQualified=False,
            firstHealth=(dict(self._first["fault"], clock=dict(self._first["fault"]["clock"]))
                if "fault" in self._first else None),
            transitions=[dict(self._transitions[i], clock=dict(self._transitions[i]["clock"]))
                for i in sorted(self._transitions)], healthOverflow=self.health_overflow)


class Channel:
    """Fixed frames on private inherited pipes, one caller-owned deadline."""
    def __init__(self, read_fd, write_fd, deadline):
        self.read_fd, self.write_fd, self.deadline = read_fd, write_fd, deadline
        self.buffer = bytearray()
        self.parent = os.getppid()
        os.set_blocking(read_fd, False)
        os.set_blocking(write_fd, False)

    def expect(self, expected):
        with selectors.DefaultSelector() as selector:
            selector.register(self.read_fd, selectors.EVENT_READ)
            while True:
                if not selector.select(remaining(self.deadline)):
                    raise TimeoutError("Owned receive protocol timeout")
                if os.getppid() != self.parent:
                    raise RuntimeError("Owned receive controller parent exited")
                data = os.read(self.read_fd, 64)
                if not data:
                    raise RuntimeError("Owned receive protocol EOF")
                self.buffer.extend(data)
                if len(self.buffer) > 64:
                    raise RuntimeError("Owned receive protocol oversized")
                if b"\n" in self.buffer:
                    frame, _, tail = self.buffer.partition(b"\n")
                    if tail or frame != expected.encode("ascii"):
                        raise RuntimeError("Owned receive protocol phase")
                    self.buffer.clear()
                    return

    def send(self, frame):
        data = (frame + "\n").encode("ascii")
        with selectors.DefaultSelector() as selector:
            selector.register(self.write_fd, selectors.EVENT_WRITE)
            while data:
                if not selector.select(remaining(self.deadline)):
                    raise TimeoutError("Owned receive protocol write timeout")
                count = os.write(self.write_fd, data)
                if count <= 0:
                    raise RuntimeError("Owned receive protocol closed")
                data = data[count:]



def cleanup_owned(children, observer, deadline):
    """Kill/reap owned children FIRST; reserve the remainder for CF quiescence."""
    errors = []
    if observer is not None:
        try:
            getattr(observer, "request_stop", lambda: None)()
        except Exception:
            errors.append("stop-request-failed")
    def phase(name):
        if observer is not None:
            try:
                getattr(observer, "lifecycle", lambda _name: None)(name)
            except Exception:
                errors.append("lifecycle-record-failed")
    for child in children:
        if child is not None and child.poll() is None:
            try:
                child.kill()
                phase("owned-kill")
            except Exception:
                errors.append("owned-kill-failed")
    reap_deadline = time.monotonic() + max(0, deadline - time.monotonic()) / 2
    for child in children:
        if child is not None:
            try:
                child.wait(timeout=max(0, reap_deadline - time.monotonic()))
                phase("owned-reap")
            except Exception:
                errors.append("owned-reap-failed")
    if observer is not None:
        try:
            observer.stop(deadline)
        except Exception:
            errors.append("reader-not-quiescent")
    return errors


def evidence_output(root):
    """Original CI root, separate from the disposable canonical config root."""
    root = Path(root).resolve(strict=True)
    if not root.is_dir():
        raise RuntimeError("Invalid original evidence root")
    directory = root
    for name in ("native-evidence", "process-receive"):
        directory = directory / name
        if directory.is_symlink():
            raise RuntimeError("Linked evidence directory refused")
        if not directory.exists():
            directory.mkdir()
        if not directory.is_dir() or not directory.resolve().is_relative_to(root):
            raise RuntimeError("Evidence escaped original root")
    return directory / ("control-" + uuid.uuid4().hex + ".json")


def write_receipt(output, status):
    # Exclusive creation refuses collisions/overwrite and final-component links.
    output = Path(output).absolute()
    directory_fd = os.open(output.anchor, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in output.parts[1:-1]:
            following = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory_fd)
            os.close(directory_fd)
            directory_fd = following
        fd = os.open(output.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o600, dir_fd=directory_fd)
        with os.fdopen(fd, "w", encoding="utf-8") as file:
            file.write(json.dumps(status) + "\n")
    finally:
        os.close(directory_fd)



def native_control(executable, output, *, observer_factory=Observer):
    canonical_guard()
    deadline = time.monotonic() + CONTROL_SECONDS
    work_deadline = deadline - min(1, CONTROL_SECONDS / 2)
    executable = Path(executable).resolve(strict=True)
    # Rust parent supplies its exact current_exe. No PID or target discovery.
    if os.environ.get("BUDDY_RECEIVER_CONTROL_EXECUTABLE") != str(executable):
        raise RuntimeError("Native receive executable admission")
    to_child_r, to_child_w = os.pipe()
    from_child_r, from_child_w = os.pipe()
    child, observer = None, None
    status = dict(functional=False, diagnostic="unqualified", coverageQualified=False, phases=[])
    try:
        env = dict(os.environ, BUDDY_RECEIVE_READ=str(to_child_r), BUDDY_RECEIVE_WRITE=str(from_child_w),
            BUDDY_RECEIVER_NATIVE_CHILD="fixture")
        child = subprocess.Popen([str(executable)], env=env, pass_fds=(to_child_r, from_child_w))
        status["phases"].append("started")
        os.close(to_child_r); to_child_r = None
        os.close(from_child_w); from_child_w = None
        channel = Channel(from_child_r, to_child_w, work_deadline)
        channel.expect("ready")
        status["phases"].append("ready")
        admitted = False
        try:
            observer = observer_factory(child, executable, native=True)
            admitted = observer.start(work_deadline)
        except Exception:
            status["diagnostic"] = "unavailable"
        channel.send("attached" if admitted else "unavailable")
        status["phases"].append("attached" if admitted else "unavailable")
        channel.expect("dispatched")
        status["phases"].append("dispatched")
        stopped = observer.stop(work_deadline) if observer is not None else dict(reason="unavailable", stopped=True)
        status["observer"] = stopped
        channel.send("quiesced")
        status["phases"].append("quiesced")
        channel.expect("completed")
        status["phases"].append("completed")
        status["functional"] = child.wait(timeout=remaining(work_deadline)) == 0
        # No CGEventPost producer here: NEVER invent a WindowServer positive.
        status["diagnostic"] = "boundary-negative-observed" if admitted and not stopped["reason"] else "unqualified"
        if not status["functional"]:
            raise RuntimeError("Native receive functional child failed")
    finally:
        primary = sys.exc_info()[1]
        errors = cleanup_owned([child], observer, deadline)
        for fd in (to_child_r, to_child_w, from_child_r, from_child_w):
            if fd is not None:
                try:
                    os.close(fd)
                except OSError:
                    errors.append("pipe-close-failed")
        if errors:
            status["functional"], status["diagnostic"] = False, "unqualified"
            status["cleanupErrors"] = errors
        try:
            write_receipt(output, status)
        except Exception:
            errors.append("evidence-write-failed")
        print("buddy-process-control " + json.dumps(status), flush=True)
        if errors:
            if primary is not None:
                primary.add_note("Owned native control cleanup/evidence failed: " + ",".join(errors))
            else:
                raise RuntimeError("Owned native control cleanup/evidence failed")
    return status

def correlate(directory, received):
    """Unique constructed/tap/AppKit/root joins. Unknowns never mean OS loss."""
    def lines(path):
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
    inputs = [row for row in lines(directory / "input-state.jsonl") if row.get("stage") == "after-click"]
    targets = lines(directory / "targets.jsonl")
    unknown = dict(phase="correlation", status="unqualified", coverageQualified=False,
        timestampRelationshipObserved=False, joins=[dict(sequence=i, kind=kind, status="unknown")
            for i, kind in enumerate(name for _ in targets for name in ("move", "down", "up"))])
    if len(inputs) != len(targets) or not targets:
        return unknown
    seen, producers = set(), set()
    previous_end = -1
    for row, target in zip(inputs, targets):
        if any(row.get(key) != target.get(key) or row.get(key) is None for key in ("kind", "label")):
            return unknown
        point = target.get("windowPoint")
        if not isinstance(point, list) or len(point) != 2 or any(type(x) not in (int, float) or not math.isfinite(x) for x in point):
            return unknown
        trace = row.get("clickTrace", [])
        if not isinstance(trace, list) or any(not isinstance(p, dict) for p in trace) or len(trace) != 3 or [p.get("eventType") for p in trace] != [5, 1, 2]:
            return unknown
        try:
            identity = tuple(p["postCall"]["start"]["monotonicNanos"] for p in trace)
            if any(type(p["postCall"]["start"]["wallMillis"]) is not int for p in trace):
                return unknown
            if any(type(x) is not int for x in identity) or list(identity) != sorted(set(identity)) or identity in seen or identity[0] <= previous_end:
                return unknown
        except (KeyError, TypeError):
            return unknown
        for phase in trace:
            timestamp = phase.get("producerTimestamp", {})
            value = timestamp.get("value") if timestamp.get("status") == "observed" else None
            if type(value) is int and value > 0:
                key = (phase["eventType"], value)
                if key in producers:
                    return unknown
                producers.add(key)
        seen.add(identity)
        previous_end = identity[-1]
    appkit, roots = [], []
    for line in (directory / "app.log").read_text(encoding="utf-8").splitlines():
        for prefix, rows in (("buddy-appkit-receive ", appkit), ("buddy-sound-dispatch ", roots)):
            if line.startswith(prefix):
                rows.append(json.loads(line[len(prefix):]))
    result, positive = [], set()
    for index, item in enumerate(inputs):
        point = targets[index].get("windowPoint")
        trace = item.get("clickTrace", [])
        if not point or not trace:
            continue
        start = min(p["postCall"]["start"]["wallMillis"] for p in trace)
        following = inputs[index + 1].get("clickTrace", []) if index + 1 < len(inputs) else []
        end = min(p["postCall"]["start"]["wallMillis"] for p in following) if following else float("inf")
        for phase in trace:
            kind = KINDS.get(phase.get("eventType"))
            timestamp = phase.get("producerTimestamp", {})
            value = timestamp.get("value") if timestamp.get("status") == "observed" else None
            taps = [r for r in received.get("received", []) if r.get("kind") == kind and r.get("timestamp") == value] if type(value) is int and value > 0 else []
            # NSEvent's uptime is measured independently. A mismatch is unknown.
            native = [r for r in appkit if r.get("phase") == "receive" and r.get("kind") == kind
                and type(value) is int and abs(r.get("eventUptimeSeconds", -1) * 1000000000 - value) <= 1024
                and abs(r.get("x", -10000) - point[0]) <= 1
                and abs(r.get("contentHeight", 0) - r.get("yFromBottom", -10000) - point[1]) <= 1]
            gpui = [r for r in roots if r.get("kind") == "event" and r.get("label") == "app-root"
                and r.get("capture") is True and len(r.get("eventPosition", [])) == 2 and r.get("event") == kind and start <= r.get("millis", -1) < end
                and all(abs(a-b) <= 1 for a,b in zip(r.get("eventPosition", [-10000, -10000]), point))]
            observed = (len(taps) == len(native) == 1 and (kind == "move" or len(gpui) == 1) and received.get("reason") is None and received.get("stopped") is True)
            if observed:
                positive.add(kind)
            result.append(dict(sequence=len(result), kind=kind, status="observed" if observed else "unknown"))
    return dict(phase="correlation", status="unqualified", coverageQualified=False,
        timestampRelationshipObserved={"move", "down", "up"}.issubset(positive), joins=result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--native-control", type=Path, required=True)
    parser.add_argument("--evidence-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        native_control(args.native_control, evidence_output(args.evidence_root))
    except Exception:
        print("buddy-process-control failed", file=sys.stderr)
        raise SystemExit(1) from None
