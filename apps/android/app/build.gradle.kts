plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

// The Rust mobile core (crates/mobile) — the library the iOS app links —
// built for Android with its generated Kotlin bindings. `-PzeronSkipCore`
// reuses the last build while iterating on Kotlin only.
val NDK_VERSION = "29.0.14206865"
val repoRoot = rootProject.projectDir.resolve("../..").canonicalFile
val coreOut = repoRoot.resolve("target/android-core")
val iconsOut = layout.buildDirectory.dir("generated/zeron-icons")
val skipCore = providers.gradleProperty("zeronSkipCore").isPresent

val buildCore by tasks.registering(Exec::class) {
    description = "Builds crates/mobile for Android and generates Kotlin bindings."
    val ndk = System.getenv("ANDROID_NDK_HOME")
        ?: androidComponents.sdkComponents.sdkDirectory.get().dir("ndk/$NDK_VERSION").asFile.path
    val lib = coreOut.resolve("jniLibs/arm64-v8a/libzeron_mobile.so")
    val skip = skipCore
    workingDir = repoRoot
    commandLine("bash", "scripts/android/build-core.sh", coreOut.path)
    environment("ANDROID_NDK_HOME", ndk)
    onlyIf { !skip || !lib.exists() }
}

// The on-device engine's payload (docs/android.md § Build): proot and its
// libs, the static musl engine (jniLibs/<abi>/lib*.so) and the Alpine rootfs
// (assets/rootfs-<abi>.tar.gz). Each script runs only while its output is
// missing — rerun it by hand to refresh; `-PzeronSkipRuntime` never runs them.
val runtimeOut = repoRoot.resolve("target/android-runtime")
val skipRuntime = providers.gradleProperty("zeronSkipRuntime").isPresent

fun runtimeScript(name: String, script: String, output: String) = tasks.register<Exec>(name) {
    description = "Runs scripts/android/$script when $output is missing."
    val ndk = System.getenv("ANDROID_NDK_HOME")
        ?: androidComponents.sdkComponents.sdkDirectory.get().dir("ndk/$NDK_VERSION").asFile.path
    val out = runtimeOut.resolve(output)
    val skip = skipRuntime
    workingDir = repoRoot
    commandLine("bash", "scripts/android/$script", runtimeOut.path)
    environment("ANDROID_NDK_HOME", ndk)
    onlyIf { !skip && !out.exists() }
}

// The first of ZERON_ANDROID_ABIS (as build-core.sh / build-engine.sh read it) stands for the set.
val runtimeAbi = System.getenv("ZERON_ANDROID_ABIS")?.trim()?.split(Regex("\\s+"))?.firstOrNull() ?: "arm64-v8a"
val fetchProot = runtimeScript("fetchProot", "fetch-proot.sh", "jniLibs/$runtimeAbi/libproot.so")
val fetchRootfs = runtimeScript("fetchRootfs", "fetch-rootfs.sh", "assets/rootfs-$runtimeAbi.tar.gz")
val buildEngine = runtimeScript("buildEngine", "build-engine.sh", "jniLibs/$runtimeAbi/libzeron.so")

// Tool and file icons: the iOS asset catalog's SVGs, rasterized.
val genIcons by tasks.registering(Exec::class) {
    description = "Rasterizes the shared transcript icons."
    val out = iconsOut.get().asFile
    inputs.dir(repoRoot.resolve("apps/ios/Zeron/Assets.xcassets"))
    inputs.dir(repoRoot.resolve("crates/ui/assets/icons"))
    inputs.file(repoRoot.resolve("scripts/android/svg2vd.py"))
    outputs.dir(out)
    commandLine("bash", repoRoot.resolve("scripts/android/gen-icons.sh").path, out.resolve("assets/icons").path, out.resolve("res").path)
}

android {
    namespace = "sh.zeron.android"
    compileSdk = 37
    ndkVersion = NDK_VERSION

    defaultConfig {
        applicationId = "sh.zeron.android"
        minSdk = 29
        targetSdk = 37
        versionCode = 1
        versionName = "0.2.97"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures { compose = true }

    sourceSets["main"].apply {
        kotlin.directories.add(coreOut.resolve("kotlin").path)
        jniLibs.directories.add(coreOut.resolve("jniLibs").path)
        jniLibs.directories.add(runtimeOut.resolve("jniLibs").path)
        assets.directories.add(runtimeOut.resolve("assets").path)
        // The exact font bytes the Rust layout engine measures.
        assets.directories.add(repoRoot.resolve("apps/ios/Zeron/Fonts").path)
        assets.directories.add(iconsOut.get().asFile.resolve("assets").path)
        res.directories.add(iconsOut.get().asFile.resolve("res").path)
    }

    packaging {
        jniLibs {
            // The runtime ships real executables as lib*.so: they must be
            // extracted to nativeLibraryDir, the only exec-allowed location.
            useLegacyPackaging = true
            keepDebugSymbols += "**/libzeron.so"
            keepDebugSymbols += "**/libproot*.so"
            // patchelf'd (SONAME/NEEDED rewritten) by fetch-proot.sh: the strip
            // pass corrupts them and libproot then fails to link.
            keepDebugSymbols += "**/libtalloc.so"
            keepDebugSymbols += "**/libandroid-shmem.so"
        }
    }

    // Pure-Kotlin unit tests only: Android stubs return defaults instead of throwing.
    testOptions { unitTests.isReturnDefaultValues = true }
}

tasks.named("preBuild") { dependsOn(buildCore, genIcons, fetchProot, fetchRootfs, buildEngine) }

dependencies {
    implementation(project(":runtime"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.core.splashscreen)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.browser)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    implementation(libs.kotlinx.coroutines.android)
    implementation("${libs.jna.get()}@aar")
    debugImplementation(libs.compose.ui.tooling)

    testImplementation(libs.junit)
    // android.jar's org.json is a stub under unit tests; the Agents parsers need the real one.
    testImplementation(libs.org.json)
}

kotlin {
    compilerOptions {
        optIn.addAll(
            "androidx.compose.material3.ExperimentalMaterial3Api",
            "androidx.compose.material3.ExperimentalMaterial3ExpressiveApi",
        )
    }
}
