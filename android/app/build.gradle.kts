plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.beebox.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.beebox.android"
        minSdk = 29          // Android 10: the oldest release still getting security patches
        targetSdk = 36       // Android 16
        versionCode = 2
        versionName = "0.2.0"
        // Phones only. The x86 builds of ML Kit's scanner were 12 MB of a
        // 32 MB APK, for emulators nobody runs this on.
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a") }
    }

    buildTypes {
        release {
            // The default rules keep @JavascriptInterface methods, which is the
            // only reflection this app relies on; ML Kit ships its own.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            // Debug-signed so `gradlew assembleRelease` produces something
            // installable without a keystore. A store build will need its own.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        viewBinding = false
        buildConfig = true
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.activity:activity-ktx:1.9.3")
    // For addDocumentStartJavaScript: the page must see the marker before its
    // own bundle runs, because the socket URL is built at module scope.
    implementation("androidx.webkit:webkit:1.12.1")
    // The scanner. ML Kit's bundled model keeps the first scan offline — a LAN
    // tool that needs the internet to read its own code would be absurd.
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
    implementation("androidx.camera:camera-camera2:1.4.1")
    implementation("androidx.camera:camera-lifecycle:1.4.1")
    implementation("androidx.camera:camera-view:1.4.1")
}
