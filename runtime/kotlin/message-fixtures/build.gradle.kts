plugins {
    kotlin("jvm")
}

dependencies {
    api(project(":message-runtime"))
    testImplementation(kotlin("test"))
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
}
