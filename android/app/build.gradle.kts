// Tern for Android: a thin Compose wrapper. All of the app's behaviour is the Rust in
// ../crates/app (loaded as ../crates/ffi), built here for each ABI, with its Kotlin bindings written from the library.
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

val repo: File = rootDir.parentFile
val ndkVersionUsed = "30.0.14904198"
val abis = listOf("arm64-v8a" to "aarch64-linux-android", "x86_64" to "x86_64-linux-android")
val jniDir = layout.projectDirectory.dir("src/main/jniLibs")
val bindingsDir = layout.buildDirectory.dir("generated/uniffi/kotlin")
// A release (-PternSplits, from CI) comes as an APK for each ABI, smaller, and a universal one holding both. Any other
// build is one APK holding both.
val splitting = project.hasProperty("ternSplits")

android {
    namespace = "xyz.hollingworth.tern"
    compileSdk = 37
    compileSdkMinor = 2
    ndkVersion = ndkVersionUsed

    defaultConfig {
        applicationId = "xyz.hollingworth.Tern"
        minSdk = 37
        targetSdk = 37
        // A release build is given these by CI (.github/workflows/release.yml).
        versionCode = (project.findProperty("ternVersionCode") as String?)?.toInt() ?: 1
        versionName = (project.findProperty("ternVersion") as String?) ?: "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    // One versionCode for all of them: an APK of another ABI, or the universal one, installs over any other.
    splits {
        abi {
            isEnable = splitting
            reset()
            include(*abis.map { it.first }.toTypedArray())
            isUniversalApk = true
        }
    }

    // Only a build given the keystore (CI's release secrets) is signed; any other release build stays unsigned.
    signingConfigs {
        System.getenv("TERN_KEYSTORE")?.let { keystore ->
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("TERN_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("TERN_KEY_ALIAS")
                keyPassword = System.getenv("TERN_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
        }
        // For trying on a phone: release's code and shrinking, profileable by the shell, debug-signed, and its own id
        // beside the real install.
        create("profile") {
            initWith(getByName("release"))
            applicationIdSuffix = ".profile"
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += "release"
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
        }
    }

    buildFeatures { compose = true }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets["main"].kotlin.directories += bindingsDir.get().asFile.path
    // Only the ABIs Tern's Rust is built for, in the universal APK too: with the 32-bit libraries the dependencies carry,
    // a 32-bit phone would install it and crash at the first call into Rust, not be refused at install.
    packaging {
        jniLibs {
            useLegacyPackaging = false
            excludes += listOf("lib/armeabi/**", "lib/armeabi-v7a/**", "lib/x86/**", "lib/mips/**", "lib/mips64/**")
        }
    }
}

/** The Rust for every ABI. Rust is optimized in both build types, or the app would not be fast. */
val cargoBuild = tasks.register<Exec>("cargoBuild") {
    workingDir = repo
    val ndk = androidComponents.sdkComponents.sdkDirectory.get().asFile.resolve("ndk/$ndkVersionUsed").path
    environment("ANDROID_NDK_HOME", ndk)
    environment("ANDROID_NDK_ROOT", ndk)
    environment("NDK_HOME", ndk)
    commandLine(
        listOf("cargo", "ndk") + abis.flatMap { listOf("-t", it.first) } +
            listOf("-P", "36", "-o", jniDir.asFile.path, "build", "--profile", "mobile", "--locked", "--lib", "-p", "tern-ffi"),
    )
    // The SDK's crates are libraries of their own too; Tern links them in and needs no copies.
    doLast {
        fileTree(jniDir) { include("**/*.so"); exclude("**/libtern_ffi.so") }.forEach { it.delete() }
    }
}

/** The Kotlin side of the bridge, read from the library so the two cannot disagree. */
val uniffiBindings = tasks.register<Exec>("uniffiBindings") {
    dependsOn(cargoBuild)
    workingDir = repo
    doFirst { bindingsDir.get().asFile.deleteRecursively() }
    commandLine(
        "cargo", "run", "--release", "--locked", "-p", "tern-ffi", "--bin", "uniffi-bindgen", "--",
        "generate", "--library", "--crate", "tern_app", "target/${abis.first().second}/mobile/libtern_ffi.so",
        "--config", "crates/app/uniffi.toml", "--language", "kotlin", "--no-format", "--out-dir", bindingsDir.get().asFile.path,
    )
}

tasks.named("preBuild") { dependsOn(uniffiBindings) }

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.animation:animation")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("androidx.work:work-runtime:2.11.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
}
