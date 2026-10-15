// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "OmniuSongMac", platforms: [.macOS(.v13)], products: [.executable(name: "OmniuSongMac", targets: ["OmniuSongMac"])], targets: [.executableTarget(name: "OmniuSongMac", linkerSettings: [.linkedFramework("AppKit"), .linkedFramework("WebKit"), .linkedFramework("CoreMIDI")])])
