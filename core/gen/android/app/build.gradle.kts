import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val releaseKeystorePath = System.getenv("ANDROID_KEYSTORE_PATH")
val releaseKeystorePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
val releaseKeyAlias = System.getenv("ANDROID_KEY_ALIAS")
val releaseKeyPassword = System.getenv("ANDROID_KEY_PASSWORD")
val releaseSigningConfigured = listOf(
    releaseKeystorePath,
    releaseKeystorePassword,
    releaseKeyAlias,
    releaseKeyPassword,
).all { !it.isNullOrBlank() }

android {
    compileSdk = 36
    namespace = "app.gipny"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        // Fork identity: must differ from the original gipny (Tor) app so both
        // install side by side. Kotlin/JNI namespace stays app.gipny.
        applicationId = "app.gipny.i2p"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (releaseSigningConfigured) {
            create("release") {
                storeFile = file(releaseKeystorePath!!)
                storePassword = releaseKeystorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (releaseSigningConfigured) {
                signingConfig = signingConfigs.getByName("release")
            }
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

// Builds the embedded go-i2p SAM router (see i2p-router/android_export.go) as
// a per-ABI JNI .so, dropped into src/main/jniLibs so it's packaged alongside
// the Rust/Tauri cdylib. Loaded + started from GipnyService.kt via JNI.
// Skippable with -PskipGoRouter for environments without a Go/NDK toolchain
// (e.g. plain `assembleDebug` iteration on non-router code).
if (!project.hasProperty("skipGoRouter")) {
    val goRouterAbis = listOf(
        Triple("arm64", "arm64-v8a", "aarch64-linux-android"),
        Triple("amd64", "x86_64", "x86_64-linux-android")
    )
    val goRouterUmbrella = tasks.register("buildGoRouterJniLibs") {
        group = "router"
        description = "Build the embedded i2p router JNI .so for all supported ABIs"
    }
    for ((goArch, abi, triple) in goRouterAbis) {
        val abiCapitalized = abi.replace("-", "_").replaceFirstChar { it.uppercase() }
        val abiTask = tasks.register("buildGoRouter$abiCapitalized", GoRouterTask::class.java) {
            group = "router"
            description = "Build the embedded i2p router JNI .so for $abi"
            rootDirRel = "../../../.."
            this.goArch = goArch
            this.abi = abi
            ndkTriple = triple
        }
        goRouterUmbrella.configure { dependsOn(abiTask) }
    }
    tasks.matching { it.name.endsWith("JniLibFolders") }.configureEach {
        dependsOn(goRouterUmbrella)
    }
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")