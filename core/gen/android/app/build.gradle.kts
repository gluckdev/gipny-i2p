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
        // install side by side. Kotlin/JNI namespace stays app.gipny, and
        // tauri.android.conf.json keeps the tauri identifier app.gipny too —
        // otherwise `tauri android build` expects this generated project under
        // java/app/gipny/i2p. applicationId alone defines the installed identity.
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
            isShrinkResources = true
            // Store native libs compressed inside the APK (extract on install):
            // libi2pd.so dominates APK size and compresses ~2x. Costs some
            // installed-size/extraction, shrinks the download substantially.
            packaging {
                jniLibs.useLegacyPackaging = true
            }
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

// Stages the embedded i2pd router (built in CI from android-router/jni) as a
// per-ABI libi2pd.so in src/main/jniLibs, so it is packaged alongside the
// Rust/Tauri cdylib, plus the reseed certificates it needs to bootstrap.
// GipnyService.kt loads and starts it via JNI.
//
// Unlike the pure-Go predecessor, i2pd is not built here: it is C++ with boost
// and OpenSSL and takes hours per ABI. See I2pdRouterTask for where it looks.
// Skippable with -PskipRouter for iteration on non-router code — the resulting
// APK starts but never connects.
if (!project.hasProperty("skipRouter")) {
    val routerAbis = listOf("arm64-v8a", "x86_64")
    val certsTask = tasks.register("stageI2pdCertificates", I2pdCertificatesTask::class.java) {
        group = "router"
        description = "Copy i2pd reseed certificates into the APK assets"
        rootDirRel = "../../../.."
    }
    val routerUmbrella = tasks.register("stageI2pdJniLibs") {
        group = "router"
        description = "Stage the embedded i2pd router for all supported ABIs"
        dependsOn(certsTask)
    }
    for (abi in routerAbis) {
        val abiCapitalized = abi.replace("-", "_").replaceFirstChar { it.uppercase() }
        val abiTask = tasks.register("stageI2pd$abiCapitalized", I2pdRouterTask::class.java) {
            group = "router"
            description = "Stage the embedded i2pd router for $abi"
            rootDirRel = "../../../.."
            this.abi = abi
        }
        routerUmbrella.configure { dependsOn(abiTask) }
    }
    tasks.matching { it.name.endsWith("JniLibFolders") }.configureEach {
        dependsOn(routerUmbrella)
    }
    tasks.matching { it.name.startsWith("merge") && it.name.endsWith("Assets") }.configureEach {
        dependsOn(certsTask)
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