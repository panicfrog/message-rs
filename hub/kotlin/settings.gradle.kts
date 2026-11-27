// message-rs data hub for Kotlin (host JVM module; the Android library wraps the same sources).
rootProject.name = "message-hub-kotlin"

include(":hub-kotlin")

// MessageEncodable / MessageCodecCompanion live in the runtime, not here: the hub only moves
// bytes, conforming a type to that one protocol is what makes it publishable (see Typed.kt).
// Included as source (not a published artifact) — same composite-build pattern the Swift
// package uses for MessageRuntime.
includeBuild("../../runtime/kotlin") {
    name = "message-rs"
}
