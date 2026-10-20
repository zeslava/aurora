plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Версия = version из frontend/package.json, передаёт scripts/build-android.sh
val appVersion = providers.gradleProperty("auroraVersion").getOrElse("0.0.0")
val appVersionCode = appVersion.split(".").map { it.toInt() }.let { (major, minor, patch) ->
    major * 10000 + minor * 100 + patch
}

android {
    namespace = "com.aurora.mediaserver"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.aurora.mediaserver"
        minSdk = 24
        targetSdk = 35
        versionCode = appVersionCode
        versionName = appVersion
    }
}

kotlin {
    jvmToolchain(17)
}
