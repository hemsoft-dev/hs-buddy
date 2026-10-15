// Print the actual owned OS window ID and bounds for window-only captures.
import Foundation
import CoreGraphics
import AppKit

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
    let receipt: [String: Any] = [
        "availableModes": modes.map { ["width": $0.width, "height": $0.height] },
        "visibleFrame": ["width": visible.width, "height": visible.height],
        "largeWindow": ["width": width, "height": height],
        "largeViewportLimited": width != 1200 || height != 820,
        "scope": "Disposable CI only; process-scoped display mode restores on exit",
    ]
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
let data = try JSONSerialization.data(withJSONObject: matches[0], options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
