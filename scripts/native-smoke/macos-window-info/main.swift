// Print the actual owned OS window ID and bounds for window-only captures.
import Foundation
import CoreGraphics
import AppKit

// Comparison admission only. Cocoa frames use a lower-left origin; Quartz
// window/event bounds use the main display's upper-left origin.
@MainActor func placementDisplay() -> [String: Any] {
    let environment = ProcessInfo.processInfo.environment
    guard environment["GITHUB_ACTIONS"] == "true",
          environment["BUDDY_SETTINGS_FIXTURE"] == "1",
          environment["BUDDY_SETTINGS_COMPARISON"] == "initial-x-v1" else {
        fatalError("Placement display requires explicit disposable comparison")
    }
#if arch(arm64)
    var count: UInt32 = 0
    guard CGGetActiveDisplayList(0, nil, &count) == .success, count == 1,
          NSScreen.screens.count == 1, let screen = NSScreen.screens.first,
          (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == CGMainDisplayID()
    else { fatalError("Placement requires one unambiguous main display") }
    let display = CGDisplayBounds(CGMainDisplayID())
    let frame = screen.frame
    let visible = screen.visibleFrame
    let scale = screen.backingScaleFactor
    guard display.origin == .zero, frame.origin == .zero,
          display.size == frame.size, frame.contains(visible),
          scale.isFinite, scale > 0,
          [visible.minX, visible.minY, visible.width, visible.height,
           display.width, display.height].allSatisfy({ $0.isFinite }),
          visible.width > 0, visible.height > 0 else {
        fatalError("Placement display coordinate conversion is ambiguous")
    }
    return [
        "space": "cg-top-left-points", "main": true, "single": true, "scale": scale,
        "bounds": ["x": display.minX, "y": display.minY, "width": display.width, "height": display.height],
        "usable": ["x": display.minX + visible.minX - frame.minX,
                   "y": display.minY + frame.maxY - visible.maxY,
                   "width": visible.width, "height": visible.height],
    ]
#else
    fatalError("Placement comparison is ARM only")
#endif
}

// Only disposable CI may change a display mode. The mode is process-scoped;
// WindowServer restores it when this owned helper exits, including on failure.
@MainActor func settingsDisplayFixture(_ output: String) throws -> Never {
    let environment = ProcessInfo.processInfo.environment
    guard environment["GITHUB_ACTIONS"] == "true",
          environment["BUDDY_SETTINGS_FIXTURE"] == "1",
          let temporary = environment["RUNNER_TEMP"] else {
        fatalError("Display fixtures require disposable GitHub Actions")
    }
    let destination = URL(fileURLWithPath: output).standardizedFileURL
    let boundary = URL(fileURLWithPath: temporary).standardizedFileURL.path + "/"
    guard destination.path.hasPrefix(boundary),
          !FileManager.default.fileExists(atPath: destination.path) else {
        fatalError("Expected a fresh owned display receipt")
    }
    let display = CGMainDisplayID()
    guard let modes = CGDisplayCopyAllDisplayModes(display, nil) as? [CGDisplayMode], !modes.isEmpty else {
        fatalError("The runner did not report its display modes")
    }
    let before = CGDisplayBounds(display)
    let candidate = modes.filter { $0.width >= 1600 && $0.height >= 1000 }
        .sorted { $0.width * $0.height < $1.width * $1.height }.first
    if let candidate, before.width < 1600 || before.height < 1000 {
        guard CGDisplaySetDisplayMode(display, candidate, nil) == .success else {
            fatalError("The runner rejected its supported display fixture mode")
        }
        Thread.sleep(forTimeInterval: 2)
    }
    guard let screen = NSScreen.screens.first(where: {
        ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == display
    }) else { fatalError("The runner did not report a usable screen") }
    let visible = screen.visibleFrame
    let width = min(1200, Int(visible.width)), height = min(820, Int(visible.height))
    guard width >= 900, height >= 650 else { fatalError("The runner cannot fit the supported minimum window") }
    var receipt: [String: Any] = [
        "availableModes": modes.map { ["width": $0.width, "height": $0.height] },
        "visibleFrame": ["width": visible.width, "height": visible.height],
        "largeWindow": ["width": width, "height": height],
        "largeViewportLimited": width != 1200 || height != 820,
        "scope": "Disposable CI only; process-scoped display mode restores on exit",
    ]
    if environment["BUDDY_SETTINGS_COMPARISON"] == "initial-x-v1" {
        let canonicalRoot = URL(fileURLWithPath: temporary).resolvingSymlinksInPath().path + "/"
        guard destination.resolvingSymlinksInPath().path == destination.path,
              destination.path.hasPrefix(canonicalRoot) else {
            fatalError("Placement display receipt escaped canonical temporary data")
        }
        receipt["comparisonDisplay"] = placementDisplay()
    }
    try JSONSerialization.data(withJSONObject: receipt, options: [.sortedKeys]).write(to: destination, options: .atomic)
    while true { Thread.sleep(forTimeInterval: 1) }
}

if CommandLine.arguments.count == 3, CommandLine.arguments[1] == "--settings-display" {
    try settingsDisplayFixture(CommandLine.arguments[2])
}

guard CommandLine.arguments.count == 2, let pid = Int(CommandLine.arguments[1]),
      let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]]
else { fatalError("Expected a process ID and a working WindowServer") }
let matches = windows.filter { window in
    let bounds = window[kCGWindowBounds as String] as? [String: Any]
    return window[kCGWindowOwnerPID as String] as? Int == pid
        && window[kCGWindowLayer as String] as? Int == 0
        && (bounds?["Width"] as? Double ?? 0) >= 700
}
guard matches.count == 1 else { fatalError("Expected exactly one owned Buddy window") }
var owned = matches[0]
if ProcessInfo.processInfo.environment["BUDDY_SETTINGS_COMPARISON"] == "initial-x-v1" {
#if arch(arm64)
    let environment = ProcessInfo.processInfo.environment
    guard environment["GITHUB_ACTIONS"] == "true", environment["BUDDY_SETTINGS_FIXTURE"] == "1",
          let temporary = environment["RUNNER_TEMP"], let config = environment["BUDDY_CONFIG_PATH"] else {
        fatalError("Placement capture requires explicit canonical config")
    }
    let root = URL(fileURLWithPath: temporary).resolvingSymlinksInPath().path + "/"
    let path = URL(fileURLWithPath: config).standardizedFileURL
    guard path.resolvingSymlinksInPath().path == path.path, path.path.hasPrefix(root) else {
        fatalError("Placement capture config escaped canonical temporary data")
    }
    let attributes = try FileManager.default.attributesOfItem(atPath: path.path)
    guard attributes[.type] as? FileAttributeType == .typeRegular,
          try String(contentsOf: path.deletingLastPathComponent().appendingPathComponent(".buddy-settings-fixture"),
                     encoding: .utf8) == "buddy settings fixture v1" else {
        fatalError("Placement capture config is not the owned fixture")
    }
    owned["comparisonDisplay"] = placementDisplay()
#else
    fatalError("Placement capture is ARM only")
#endif
}
let data = try JSONSerialization.data(withJSONObject: owned, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
