plugins {
    kotlin("jvm")
}

group = "message.rs"

// The Rust dylib/so for host-JVM tests; overridable with -PhubLibDir=...
val hubLibDir = providers.gradleProperty("hubLibDir")
    .orElse(rootDir.resolve("../../target/debug").absolutePath)
    .get()

dependencies {
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    api("message.rs:message-runtime")
    testImplementation(kotlin("test"))
    testImplementation("message.rs:message-fixtures")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
    systemProperty("hub.lib", hubLibDir)
}
