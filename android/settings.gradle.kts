// Iris for Android: the APK around the Rust library `libiris_android.so`.
// Built by CI (`gradle -p android assembleDebug` or `assembleRelease`) after
// `cargo ndk` has put the library in app/src/main/jniLibs. See android/README.md.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "Iris"
include(":app")
