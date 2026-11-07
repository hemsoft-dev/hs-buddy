"""Test-only POSIX boundary model on Windows; real POSIX operations elsewhere.

Windows runs real socket IPC, Python children, exclusive file creation and staging.
Virtual directory descriptors/links model unavailable openat semantics. This is
neither Windows openat coverage nor native Mac/process receiving evidence.
"""
from contextlib import ExitStack
import base64
import json
import os
from pathlib import Path
import selectors
import socket
import subprocess
import sys
import tempfile
import threading
from unittest.mock import patch

WINDOWS = os.name == "nt"


class Facade:
    def __init__(self, module):
        self.module = module
    def __getattr__(self, name):
        return getattr(self.module, name)


class Boundary:
    def __init__(self, module):
        self.stack = ExitStack()
        self.sockets, self.directories, self.links = {}, {}, {}
        self.scripts = set()
        self.next_directory = -1
        self.temp = self.stack.enter_context(tempfile.TemporaryDirectory(prefix="receiver-contract-"))
        self.mode = "windows-posix-model" if WINDOWS else "real-posix"
        owner = self
        class Popen(subprocess.Popen):
            def __init__(self, args, **kwargs):
                original = list(args)
                if owner.key(args[0]) in owner.scripts:
                    descriptors = kwargs.pop("pass_fds", ())
                    if descriptors:
                        share = Path(owner.temp) / ("share-" + str(len(list(Path(owner.temp).iterdir()))) + ".json")
                        wrapper = """
import base64,json,os,runpy,socket,sys,threading,time
script,share=sys.argv[1:]
deadline=time.monotonic()+1
while not os.path.isfile(share):
 if time.monotonic()>=deadline:raise RuntimeError('test socket handoff timeout')
 threading.Event().wait(0.001)
with open(share,encoding='utf-8') as file:values=json.load(file)
owned={int(fd):socket.fromshare(base64.b64decode(value)) for fd,value in values.items()}
read,write=os.read,os.write
os.read=lambda fd,n:owned[fd].recv(n) if fd in owned else read(fd,n)
os.write=lambda fd,data:owned[fd].send(data) if fd in owned else write(fd,data)
try:runpy.run_path(script,run_name='__main__')
finally:
 for stream in owned.values():stream.close()
"""
                        super().__init__([sys.executable, "-c", wrapper, str(args[0]), str(share)], **kwargs)
                        payload = {str(fd): base64.b64encode(owner.sockets[fd].share(self.pid)).decode("ascii")
                            for fd in descriptors}
                        partial = share.with_suffix(".tmp")
                        partial.write_text(json.dumps(payload), encoding="utf-8")
                        partial.replace(share)
                    else:
                        super().__init__([sys.executable, *args], **kwargs)
                    self.args = original
                else:
                    super().__init__(args, **kwargs)
        self.popen_type = Popen if WINDOWS else subprocess.Popen
        if WINDOWS:
            self.os = Facade(os)
            for name in ("pipe", "read", "write", "close", "set_blocking", "open"):
                setattr(self.os, name, getattr(self, name))
            self.os.O_DIRECTORY, self.os.O_NOFOLLOW = 1 << 28, 1 << 29
            self.selectors = Facade(selectors)
            class Selector:
                def __init__(self):
                    self.actual = selectors.DefaultSelector()
                def __enter__(self):
                    return self
                def __exit__(self, *args):
                    self.actual.close()
                def register(self, descriptor, events):
                    return self.actual.register(owner.sockets[descriptor], events)
                def select(self, timeout):
                    return self.actual.select(timeout)
            self.selectors.DefaultSelector = Selector
            base = type(Path())
            class ModelPath(base):
                def is_symlink(self):
                    return owner.key(self) in owner.links or super().is_symlink()
                def resolve(self, strict=False):
                    absolute = Path(self).absolute()
                    for alias, target in owner.links.items():
                        if absolute.is_relative_to(Path(alias)):
                            return ModelPath(target / absolute.relative_to(Path(alias))).resolve(strict=strict)
                    return super().resolve(strict=strict)
            self.path_type = ModelPath
        self.bind(module)

    @staticmethod
    def key(path):
        return os.path.normcase(os.path.abspath(path))

    def register_script(self, path):
        self.scripts.add(self.key(path))

    def bind(self, module):
        facade = Facade(subprocess)
        facade.Popen = self.popen_type
        self.stack.enter_context(patch.object(module, "subprocess", facade))
        if WINDOWS:
            self.stack.enter_context(patch.object(module, "os", self.os))
            self.stack.enter_context(patch.object(module, "selectors", self.selectors))
            self.stack.enter_context(patch.object(module, "Path", self.path_type))

    def bind_settings(self, settings):
        self.bind(settings.process_receive)
        facade = Facade(subprocess)
        facade.Popen = self.popen_type
        self.stack.enter_context(patch.object(settings, "subprocess", facade))

    def symlink(self, path, target):
        if WINDOWS:
            self.links[self.key(path)] = Path(target).absolute()
        else:
            path.symlink_to(target, target_is_directory=target.is_dir())

    def unlink(self, path):
        if WINDOWS and self.key(path) in self.links:
            del self.links[self.key(path)]
        else:
            path.unlink()

    def pipe(self):
        first, second = socket.socketpair()
        for stream in (first, second):
            self.sockets[stream.fileno()] = stream
        return first.fileno(), second.fileno()

    def read(self, descriptor, size):
        return self.sockets[descriptor].recv(size) if descriptor in self.sockets else os.read(descriptor, size)

    def write(self, descriptor, data):
        return self.sockets[descriptor].send(data) if descriptor in self.sockets else os.write(descriptor, data)

    def set_blocking(self, descriptor, value):
        return self.sockets[descriptor].setblocking(value)

    def close(self, descriptor=None):
        if descriptor is None:
            try:
                for stream in self.sockets.values():
                    stream.close()
                self.sockets.clear()
            finally:
                self.stack.close()
        elif descriptor in self.sockets:
            self.sockets.pop(descriptor).close()
        elif descriptor in self.directories:
            del self.directories[descriptor]
        else:
            os.close(descriptor)

    def open(self, path, flags, mode=0o777, *, dir_fd=None):
        parent = self.directories[dir_fd] if dir_fd is not None else None
        path = parent / path if parent is not None else Path(path)
        if any(path.absolute().is_relative_to(Path(alias)) for alias in self.links):
            raise OSError("test model refuses linked path")
        if path.is_symlink() or path.is_junction():
            raise OSError("test model refuses linked path")
        if flags & self.os.O_DIRECTORY:
            if not path.is_dir():
                raise NotADirectoryError("test model requires directory")
            descriptor = self.next_directory
            self.next_directory -= 1
            self.directories[descriptor] = path
            return descriptor
        return os.open(path, flags & ~(self.os.O_DIRECTORY | self.os.O_NOFOLLOW), mode)
