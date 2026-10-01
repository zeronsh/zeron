plugins {
    alias(libs.plugins.android.library)
}

// The on-device engine: proot guest bootstrap + RuntimeService
// (docs/android.md § Runtime API). Its executables and the Alpine rootfs are
// packaged by :app from target/android-runtime (jniLibs + assets).
android {
    namespace = "sh.zeron.runtime"
    compileSdk = 37
    defaultConfig { minSdk = 29 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.android)
}
