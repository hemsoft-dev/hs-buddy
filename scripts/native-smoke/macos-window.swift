// Print the actual owned OS window ID and bounds for window-only captures.
import Foundation
import CoreGraphics

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
