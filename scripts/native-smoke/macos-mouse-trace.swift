// Diagnostic only: passive mouse events delivered to one owned CI process.
// https://developer.apple.com/documentation/coregraphics/cgevent
import CoreGraphics
import Foundation
import Darwin

final class MouseTrace {
    let output: FileHandle
    let pid: pid_t
    var count = 0

    init(directory: URL, pid: pid_t) throws {
        self.pid = pid
        let path = directory.appendingPathComponent("native-mouse-events.jsonl")
        guard !FileManager.default.fileExists(atPath: path.path),
              FileManager.default.createFile(atPath: path.path, contents: nil) else {
            throw NSError(domain: "MouseTrace", code: 1)
        }
        output = try FileHandle(forWritingTo: path)
    }

    func windowMetadata(_ identifier: Int64) -> [String: Any] {
        var value: [String: Any] = ["windowNumber": identifier, "available": false]
        guard identifier > 0, identifier <= Int64(UInt32.max),
              let rows = CGWindowListCopyWindowInfo(.optionIncludingWindow, CGWindowID(identifier)) as? [[String: Any]],
              let row = rows.first(where: {
                  ($0[kCGWindowNumber as String] as? NSNumber)?.int64Value == identifier
              }) else { return value }
        value["available"] = true
        // Allowlist metadata only. Never persist window titles or contents.
        for key in [kCGWindowOwnerPID, kCGWindowOwnerName, kCGWindowBounds,
                    kCGWindowLayer, kCGWindowAlpha, kCGWindowIsOnscreen] {
            if let field = row[key as String] { value[key as String] = field }
        }
        return value
    }

    func record(type: CGEventType, event: CGEvent) {
        do {
            count += 1
            let value: [String: Any] = [
                "index": count,
                "ownedPid": pid,
                "type": type.rawValue,
                "eventTimestamp": String(event.timestamp),
                "observedUptime": ProcessInfo.processInfo.systemUptime,
                "position": [event.location.x, event.location.y],
                "flags": event.flags.rawValue,
                "sourcePid": event.getIntegerValueField(.eventSourceUnixProcessID),
                "targetPid": event.getIntegerValueField(.eventTargetUnixProcessID),
                "mouseEventNumber": event.getIntegerValueField(.mouseEventNumber),
                "clickState": event.getIntegerValueField(.mouseEventClickState),
                "windowUnderPointer": event.getIntegerValueField(.mouseEventWindowUnderMousePointer),
                "handlingWindow": event.getIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent),
                "referencedWindows": Array(Set([
                    event.getIntegerValueField(.mouseEventWindowUnderMousePointer),
                    event.getIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent),
                ])).sorted().map { windowMetadata($0) },
                "tapDisabled": type == .tapDisabledByTimeout || type == .tapDisabledByUserInput,
            ]
            var data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
            data.append(0x0a)
            try output.write(contentsOf: data)
        } catch {
            // A broken trace is a failed diagnostic, never a silent empty log.
            fputs("Owned mouse trace could not preserve event metadata.\n", stderr)
            exit(2)
        }
    }
}

func mouseCallback(
    proxy: CGEventTapProxy,
    type: CGEventType,
    event: CGEvent,
    context: UnsafeMutableRawPointer?
) -> Unmanaged<CGEvent>? {
    if let context {
        Unmanaged<MouseTrace>.fromOpaque(context).takeUnretainedValue().record(type: type, event: event)
    }
    return Unmanaged.passUnretained(event)
}

guard ProcessInfo.processInfo.environment["GITHUB_ACTIONS"] == "true",
      ProcessInfo.processInfo.environment["BUDDY_SETTINGS_FIXTURE"] == "1",
      CommandLine.arguments.count == 3,
      let pid = pid_t(CommandLine.arguments[1]), pid > 0,
      kill(pid, 0) == 0 else {
    fputs("Mouse tracing requires one live owned process on a disposable CI Mac.\n", stderr)
    exit(1)
}

let directory = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let trace = try MouseTrace(directory: directory, pid: pid)
let mask = [CGEventType.leftMouseDown, .leftMouseUp, .mouseMoved]
    .reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) }
guard let tap = CGEvent.tapCreateForPid(
    pid: pid,
    place: .tailAppendEventTap,
    options: .listenOnly,
    eventsOfInterest: mask,
    callback: mouseCallback,
    userInfo: Unmanaged.passUnretained(trace).toOpaque()
), let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0) else {
    fputs("The disposable runner did not permit a passive owned-process event tap.\n", stderr)
    exit(1)
}
CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
guard CGEvent.tapIsEnabled(tap: tap) else {
    fputs("The passive owned-process event tap is not enabled.\n", stderr)
    exit(1)
}
let ready: [String: Any] = ["result": "listening", "ownedPid": pid, "listenOnly": true, "mouseOnly": true]
try JSONSerialization.data(withJSONObject: ready, options: [.sortedKeys]).write(
    to: directory.appendingPathComponent("native-mouse-ready.json"), options: [.withoutOverwriting]
)
CFRunLoopRun()
