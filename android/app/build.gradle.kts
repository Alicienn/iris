// The APK: IrisActivity, Android's NativeActivity with a QR scanner and the link it was
// opened with (src/main/java), loads libiris_android.so, which is all of Iris.

plugins {
    id("com.android.application")
}

// The version is the workspace's (Cargo.toml), passed in by CI; one number for
// Windows and Android. versionCode: 6.3.0 → 60300.
val irisVersion: String = System.getenv("IRIS_VERSION") ?: "0.0.0"
val irisVersionCode: Int = irisVersion.split(".").let { p ->
    (p.getOrNull(0)?.toIntOrNull() ?: 0) * 10000 +
        (p.getOrNull(1)?.toIntOrNull() ?: 0) * 100 +
        (p.getOrNull(2)?.toIntOrNull() ?: 0)
}.coerceAtLeast(1)

android {
    namespace = "fr.iris.mail"
    compileSdk = 35

    defaultConfig {
        applicationId = "fr.iris.mail"
        minSdk = 26
        targetSdk = 35
        versionCode = irisVersionCode
        versionName = irisVersion
    }

    // Release signing: the project's key, from CI's secrets. Without them the release
    // build is signed with the debug key of the machine that built it.
    signingConfigs {
        create("iris") {
            val magasin = System.getenv("ANDROID_KEYSTORE_FILE")
            if (magasin != null && file(magasin).exists()) {
                storeFile = file(magasin)
                storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANDROID_KEY_PASSWORD") ?: System.getenv("ANDROID_KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            val iris = signingConfigs.getByName("iris")
            signingConfig = if (iris.storeFile != null) iris else signingConfigs.getByName("debug")
        }
    }

    // The library cargo-ndk built, one folder per ABI.
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")

    // Compressed in the APK and unpacked at install: a smaller download, the same
    // space once installed.
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}

dependencies {
    // Google's code scanner: the camera and the reading are Play services', so Iris
    // asks for no camera permission and ships no camera code.
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")
}
