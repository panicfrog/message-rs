// swift-tools-version:5.9
// message-rs Swift runtime + the generated schema of the fixture messages (see SPEC.md).
import PackageDescription

let package = Package(
    name: "message-rs",
    products: [
        .library(name: "MessageRuntime", targets: ["MessageRuntime"]),
        // The generated fixture schema, for other packages' tests (e.g. hub/apple proving a
        // real MessageCodec type publishes/pulls end to end).
        .library(name: "MessageFixtures", targets: ["MessageFixtures"]),
    ],
    targets: [
        .target(name: "MessageRuntime"),
        .target(name: "MessageFixtures", dependencies: ["MessageRuntime"]),
        .testTarget(name: "ConformanceTests", dependencies: ["MessageRuntime", "MessageFixtures"]),
    ]
)
