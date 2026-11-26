// swift-tools-version:5.9
// message-rs data hub for Swift. Two link modes (see hub/DESIGN.md and example/PLAN.md):
//
//   standalone (default) — the package carries its own Rust static library as a binaryTarget
//     xcframework. For apps that use only the Swift surface.
//
//   superlib (`MESSAGE_HUB_SUPERLIB=1`) — headers only; the Rust symbols come from the app's
//     aggregate library (a React Native app's librust_app.a, which hub-turbo already links the
//     core into). Linking both would give two `Hub::shared()` registries and the ends would
//     silently fail to see each other, so exactly one copy must provide the symbols.
//
// The Swift wrapper sources are identical in both modes: the bridge is a stable C ABI layer.
import PackageDescription
import Foundation

let superlib = ProcessInfo.processInfo.environment["MESSAGE_HUB_SUPERLIB"] == "1"

var targets: [Target] = [
    .target(name: "MessageHubCxx", publicHeadersPath: "include"),
]
var hubDeps: [Target.Dependency] = ["MessageHubCxx"]
var products: [Product] = []

if superlib {
    // No binaryTarget: the app links the superlib, which already contains message-hub and
    // hub-ffi. The wrapper resolves the same symbols at final link time.
    targets.append(
        .target(
            name: "MessageHub",
            dependencies: hubDeps,
            swiftSettings: [.interoperabilityMode(.Cxx)]
        )
    )
    products.append(.library(name: "MessageHub", targets: ["MessageHub"]))
} else {
    targets.append(.binaryTarget(name: "MessageHubRust", path: "Artifacts/MessageHubRust.xcframework"))
    hubDeps.append("MessageHubRust")
    targets.append(
        .target(
            name: "MessageHub",
            dependencies: hubDeps,
            swiftSettings: [.interoperabilityMode(.Cxx)]
        )
    )
    targets.append(
        .testTarget(
            name: "MessageHubTests",
            dependencies: ["MessageHub"],
            swiftSettings: [.interoperabilityMode(.Cxx)]
        )
    )
    products.append(.library(name: "MessageHub", targets: ["MessageHub"]))
}

let package = Package(
    name: "MessageHub",
    platforms: [.macOS(.v13), .iOS(.v13)],
    products: products,
    targets: targets,
    cxxLanguageStandard: .cxx17
)
