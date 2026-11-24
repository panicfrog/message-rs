// swift-tools-version:5.9
// message-rs Swift runtime + the generated schema of the fixture messages (see SPEC.md).
import PackageDescription

let package = Package(
    name: "message-rs",
    products: [
        .library(name: "MessageRuntime", targets: ["MessageRuntime"]),
    ],
    targets: [
        .target(name: "MessageRuntime"),
        .target(name: "MessageFixtures", dependencies: ["MessageRuntime"]),
        .testTarget(name: "ConformanceTests", dependencies: ["MessageRuntime", "MessageFixtures"]),
    ]
)
