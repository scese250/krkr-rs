// krkr-rs Android app (experimental). See docs/code/android/build.md §5 (the
// Gradle project).
//
// arm64-v8a only — the engine ships a single ABI, and there is no x86_64
// build for emulators yet.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "krkr-rs"
include(":app")
