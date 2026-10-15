"""Authorize only two executables for an owned disposable Keychain entry.

No password data is read or replaced. The private Security API used by Apple's
security CLI authorizes this ACL update with the public fixture-keychain password.
https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_keychain/lib/SecKeychainItemPriv.h
This helper is CI tooling. It is never included in either desktop application.
"""
import ctypes as c
import hashlib
import os
from pathlib import Path
import sys


def run():
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_CREDENTIAL_FIXTURE") != "1":
        raise RuntimeError("Disposable macOS CI fixture required")
    config, node, rust = map(Path, sys.argv[1:])
    profile = config.parent
    if (profile / ".buddy-credential-fixture").read_text() != "buddy credential fixture v1":
        raise RuntimeError("Owned fixture marker required")
    keychain = profile / "fixture.keychain-db"
    if not keychain.is_file() or not profile.name.startswith("buddy-credential-interop-"):
        raise RuntimeError("Owned fixture keychain required")
    security = c.CDLL("/System/Library/Frameworks/Security.framework/Security")
    core = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    pointer, out = c.c_void_p, c.POINTER(c.c_void_p)
    core.CFStringCreateWithCString.argtypes = [pointer, c.c_char_p, c.c_uint32]
    core.CFStringCreateWithCString.restype = pointer
    core.CFArrayCreate.argtypes = [pointer, c.POINTER(pointer), c.c_long, pointer]
    core.CFArrayCreate.restype = pointer
    core.CFRelease.argtypes = [pointer]
    signatures = {
        "SecKeychainOpen": [c.c_char_p, out],
        "SecKeychainFindGenericPassword": [pointer, c.c_uint32, c.c_char_p, c.c_uint32, c.c_char_p, pointer, pointer, out],
        "SecTrustedApplicationCreateFromPath": [c.c_char_p, out],
        "SecAccessCreate": [pointer, pointer, out],
        "SecKeychainItemSetAccessWithPassword": [pointer, pointer, c.c_uint32, c.c_char_p],
    }
    for name, arguments in signatures.items():
        function = getattr(security, name)
        function.argtypes, function.restype = arguments, c.c_int32
    references = []

    def check(status):
        if status != 0:
            raise RuntimeError(f"Fixture ACL operation failed with OS status {status}")

    def allocate(name, *arguments):
        result = pointer()
        check(getattr(security, name)(*arguments, c.byref(result)))
        references.append(result)
        return result

    try:
        store = allocate("SecKeychainOpen", os.fsencode(keychain))
        service = b"hs-buddy-pollen-v1"
        account = ("pollen-" + hashlib.sha256(os.fsencode(os.path.abspath(config))).hexdigest()).encode()
        # Null password-data pointers request only the item reference.
        item = allocate("SecKeychainFindGenericPassword", store, len(service), service, len(account), account, None, None)
        apps = [allocate("SecTrustedApplicationCreateFromPath", os.fsencode(path.resolve())) for path in (node, rust)]
        array = core.CFArrayCreate(None, (pointer * len(apps))(*(app.value for app in apps)), len(apps), None)
        references.append(array)
        label = core.CFStringCreateWithCString(None, b"Buddy disposable credential fixture", 0x08000100)
        references.append(label)
        access = allocate("SecAccessCreate", label, array)
        password = b"buddy-ci-fixture-keychain"
        check(security.SecKeychainItemSetAccessWithPassword(item, access, len(password), password))
    finally:
        for reference in reversed(references):
            if reference:
                core.CFRelease(reference)


if __name__ == "__main__":
    try:
        run()
    except Exception as error:
        # This helper never requests password data. Its exception contains
        # only guard failures, API type errors, or a numeric OS status.
        print(f"Owned fixture ACL authorization failed: {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(1)
