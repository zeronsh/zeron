import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Release identity: apps/android/version.properties (overridable with
// -PzeronVersionCode / -PzeronVersionName, e.g. to test the updater).
val versionProps = Properties().apply {
    rootProject.file("version.properties").takeIf { it.exists() }?.inputStream()?.use { load(it) }
}
val zeronVersionCode = (findProperty("zeronVersionCode") as String?)?.toInt()
    ?: versionProps.getProperty("versionCode", "1").toInt()
val zeronVersionName = (findProperty("zeronVersionName") as String?)
    ?: versionProps.getProperty("versionName", "dev")

// One stable signing key for every release (kept OUTSIDE the repo): pass
// -PzeronKeystoreProperties=/path/keystore.properties (storeFile, storePassword,
// keyAlias, keyPassword). Without it the default debug key is used.
val keystoreProps = (findProperty("zeronKeystoreProperties") as String?)?.let { path ->
    Properties().apply { file(path).inputStream().use { load(it) } }
}

// Release APKs carry only arm64-v8a (every supported phone); the x86_64 core
// adds ~14 MB and is only for emulators. -PzeronWithX86_64=true (build-apk.sh:
// ZERON_WITH_X86_64=1) puts it back into release; debug always takes both.
val releaseAbis = if ((findProperty("zeronWithX86_64") as String?) == "true") listOf("arm64-v8a", "x86_64") else listOf("arm64-v8a")

android {
    namespace = "sh.zeron.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "sh.zeron.android"
        minSdk = 26
        targetSdk = 35
        versionCode = zeronVersionCode
        versionName = zeronVersionName
        // Boot into the demo workspace when nothing is set up yet (both the
        // shipped release build and local debug builds).
        buildConfigField("boolean", "DEMO_BY_DEFAULT", "true")
    }

    signingConfigs {
        if (keystoreProps != null) {
            create("zeron") {
                storeFile = file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        // Shipped builds are `release`: not debuggable, so ART compiles the
        // app normally and applies the Compose baseline profiles. A debuggable
        // build runs Compose several times slower (scrolling stuttered).
        // R8 + resource shrinking for release only (debug stays readable and
        // fast to build). JNA / UniFFI work by reflection: see
        // proguard-rules.pro. The mapping file is in
        // build/outputs/mapping/release/ (build-apk.sh keeps a copy).
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfigs.findByName("zeron")?.let { signingConfig = it }
            ndk { abiFilters += releaseAbis }
        }
        debug {
            isDebuggable = true
            signingConfigs.findByName("zeron")?.let { signingConfig = it }
            ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
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
        compose = true
        buildConfig = true
    }

    testOptions {
        unitTests {
            // Robolectric screenshot tests read the app's assets (fonts).
            isIncludeAndroidResources = true
            all { test ->
                // Screenshot renders (src/test/.../screenshots) run only with
                // -PzeronScreenshots=true and need the HOST build of the Rust
                // core (see README): JNA loads it from target/mobile.
                test.systemProperty("jna.library.path", rootProject.file("../../target/mobile").absolutePath)
                test.systemProperty("zeron.screenshots", (findProperty("zeronScreenshots") as String?) ?: "false")
                test.systemProperty("zeron.screenshots.dir", (findProperty("zeronScreenshotsDir") as String?) ?: layout.buildDirectory.dir("screenshots").get().asFile.absolutePath)
                test.systemProperty("roborazzi.test.record", "true")
                test.systemProperty("robolectric.pixelCopyRenderMode", "hardware")
                test.maxHeapSize = "3g"
            }
        }
    }

    packaging {
        jniLibs {
            // JNA dlopens libzeron_mobile.so from the extracted native dir.
            useLegacyPackaging = true
        }
    }
}

composeCompiler {
    // See the file: UniFFI records are read-only snapshots, so unchanged list
    // rows can skip recomposition.
    stabilityConfigurationFile = rootProject.layout.projectDirectory.file("compose-stability.conf")
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("com.caverock:androidsvg-aar:1.4")
    implementation("androidx.browser:browser:1.8.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.9.0")
    // JVM screenshot renders (Robolectric + Roborazzi), see README.
    testImplementation(composeBom)
    testImplementation("androidx.compose.ui:ui-test-junit4")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("io.github.takahirom.roborazzi:roborazzi:1.39.0")
    testImplementation("io.github.takahirom.roborazzi:roborazzi-compose:1.39.0")
    testImplementation("io.github.takahirom.roborazzi:roborazzi-junit-rule:1.39.0")
    testImplementation("androidx.test.ext:junit:1.2.1")
    // Desktop JNA (linux-x86-64 libjnidispatch) for the host Rust core.
    testImplementation("net.java.dev.jna:jna:5.17.0")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
