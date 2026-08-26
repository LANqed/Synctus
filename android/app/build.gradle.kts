plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "dev.synctus.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.synctus.app"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

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
