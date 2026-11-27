plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Версия = version из frontend/package.json, передаёт scripts/build-android.sh
val appVersion = providers.gradleProperty("auroraVersion").getOrElse("0.0.0")
val defaultVersionCode = appVersion.split(".").map { it.toInt() }.let { (major, minor, patch) ->
    major * 10000 + minor * 100 + patch
}
val appVersionCode = providers.gradleProperty("auroraVersionCode").orNull?.let {
    requireNotNull(it.toIntOrNull()) { "auroraVersionCode must be an integer" }
} ?: defaultVersionCode.coerceAtLeast(1)
require(appVersionCode in 1..2100000000) { "auroraVersionCode must be between 1 and 2100000000" }

val releaseKeystore = providers.environmentVariable("AURORA_ANDROID_KEYSTORE").orNull
fun signingSecret(name: String): String = providers.environmentVariable(name).orNull
    ?.takeIf { it.isNotBlank() } ?: error("Missing signing environment variable: $name")

android {
    namespace = "ru.zeslava.aurora"
    compileSdk = 36

    defaultConfig {
        applicationId = "ru.zeslava.aurora"
        minSdk = 24
        targetSdk = 36
        versionCode = appVersionCode
        versionName = appVersion
    }

    signingConfigs {
        if (!releaseKeystore.isNullOrBlank()) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = signingSecret("AURORA_ANDROID_STORE_PASSWORD")
                keyAlias = signingSecret("AURORA_ANDROID_KEY_ALIAS")
                keyPassword = signingSecret("AURORA_ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        getByName("release") {
            isDebuggable = false
            if (!releaseKeystore.isNullOrBlank()) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }
}

// Не позволяем получить неподписанный release при прямом запуске Gradle.
tasks.configureEach {
    if (name == "preReleaseBuild") {
        doFirst {
            check(!releaseKeystore.isNullOrBlank()) {
                "Release requires AURORA_ANDROID_KEYSTORE and signing credentials"
            }
        }
    }
}

kotlin {
    jvmToolchain(17)
}
