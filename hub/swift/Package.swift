// swift-tools-version:5.9
// message-rs data hub for Swift: a Clang target with the cxxbridge header, the Rust static
// library as a binaryTarget, and a Swift wrapper target with C++ interop. See hub/DESIGN.md.
import PackageDescription

let package = Package(
    name: "MessageHub",
    platforms: [.macOS(.v13), .iOS(.v13)],
    products: [.library(name: "MessageHub", targets: ["MessageHub"])],
    targets: [
        .target(name: "MessageHubCxx", publicHeadersPath: "include"),
        .binaryTarget(name: "MessageHubRust", path: "Artifacts/MessageHubRust.xcframework"),
        .target(
            name: "MessageHub",
            dependencies: ["MessageHubCxx", "MessageHubRust"],
            swiftSettings: [.interoperabilityMode(.Cxx)]
        ),
        .testTarget(
            name: "MessageHubTests",
            dependencies: ["MessageHub"],
            swiftSettings: [.interoperabilityMode(.Cxx)]
        ),
    ],
    cxxLanguageStandard: .cxx17
)
