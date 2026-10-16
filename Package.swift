// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "BuddyNativeQualification",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "buddy-window-info", targets: ["BuddyWindowInfo"]),
        .executable(name: "buddy-ocr-words", targets: ["BuddyOcrWords"]),
    ],
    targets: [
        .executableTarget(
            name: "BuddyWindowInfo",
            path: "scripts/native-smoke/macos-window-info"
        ),
        .executableTarget(
            name: "BuddyOcrWords",
            path: "scripts/native-smoke/macos-ocr-words"
        ),
    ]
)
