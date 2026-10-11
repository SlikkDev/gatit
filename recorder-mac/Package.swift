// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "GatitRecorder",
    platforms: [.macOS("14.2")],
    products: [
        .executable(name: "gatit-recorder", targets: ["GatitRecorder"]),
    ],
    targets: [
        .executableTarget(name: "GatitRecorder", path: "Sources/GatitRecorder"),
    ]
)
