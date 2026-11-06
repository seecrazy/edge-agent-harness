plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.edgeassistant.harness.vehicle"
    compileSdk = 34

    defaultConfig {
        minSdk = 29
    }
}

kotlin {
    jvmToolchain(17)
}
