plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

/**
 * The release version, read from the workspace `Cargo.toml`.
 *
 * One source of truth on purpose. Duplicating the number here is how an APK ends
 * up claiming a version it is not, and the desktop update check compares the
 * GitHub tag against the crate version — a drift there means a permanent
 * "update available" prompt. CI refuses to build when the tag and this disagree.
 */
val cargoVersion: String = run {
    val manifest = rootProject.file("../Cargo.toml")
    require(manifest.isFile) { "cannot find ${manifest.absolutePath}" }

    // Scoped to [workspace.package] so a dependency's version cannot be picked up
    // by accident.
    val section = manifest.readText()
        .substringAfter("[workspace.package]", "")
        .substringBefore("\n[")
    Regex("""^\s*version\s*=\s*"([^"]+)"""", RegexOption.MULTILINE)
        .find(section)
        ?.groupValues
        ?.get(1)
        ?: error("no version found in [workspace.package] of ${manifest.absolutePath}")
}

/**
 * Android requires a monotonically increasing integer. `major * 10000 + minor *
 * 100 + patch` keeps that true for any version this project will plausibly reach,
 * and stays readable: 0.2.0 is 200, 1.3.7 is 10307.
 *
 * A pre-release suffix is ignored here — `0.2.0-beta.1` and `0.2.0` share a code,
 * which is correct: installing one over the other should be an upgrade, not a
 * downgrade.
 */
val cargoVersionCode: Int = run {
    val parts = cargoVersion.substringBefore('-').split('.')
    val major = parts.getOrNull(0)?.toIntOrNull() ?: 0
    val minor = parts.getOrNull(1)?.toIntOrNull() ?: 0
    val patch = parts.getOrNull(2)?.toIntOrNull() ?: 0
    major * 10000 + minor * 100 + patch
}

android {
    namespace = "dev.synctus.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.synctus.app"
        minSdk = 26
        targetSdk = 35
        versionCode = cargoVersionCode
        versionName = cargoVersion

        // Only 64-bit ABIs. 32-bit Android is effectively gone and each extra ABI
        // adds a full copy of the Rust library to the APK.
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    signingConfigs {
        // Release signing comes from the environment so CI can sign without a
        // keystore in the repository. Falls back to unsigned when unset, which is
        // what a local debug build wants.
        create("release") {
            val storeFilePath = System.getenv("SYNCTUS_KEYSTORE")
            if (!storeFilePath.isNullOrBlank()) {
                storeFile = file(storeFilePath)
                storePassword = System.getenv("SYNCTUS_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("SYNCTUS_KEY_ALIAS")
                keyPassword = System.getenv("SYNCTUS_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Only use the release signing config when a keystore was provided.
            signingConfig = if (System.getenv("SYNCTUS_KEYSTORE").isNullOrBlank()) {
                null
            } else {
                signingConfigs.getByName("release")
            }
        }
        debug {
            applicationIdSuffix = ".debug"
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

    // The Rust .so files are built by CI (or `scripts/build-android-libs.*`) into
    // `app/src/main/jniLibs/<abi>/libsynctus.so`.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    packaging {
        resources {
            excludes += setOf("/META-INF/{AL2.0,LGPL2.1}")
        }
        jniLibs {
            // The Rust library is already stripped by the release profile.
            useLegacyPackaging = false
        }
    }

    lint {
        // A missing translation must not fail the release build.
        disable += "MissingTranslation"
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-service:2.8.7")
    implementation("androidx.activity:activity-compose:1.9.3")

    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.material3:material3")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.compose.ui:ui-tooling-preview")

    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.9.0")
}
