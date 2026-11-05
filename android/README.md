# Tern for Android

Tern on Android 17 and newer. All of the app's behaviour is Rust: `crates/app` holds the state,
the navigation and what every action does, and `crates/ffi` is the library this app loads. This folder is a thin Jetpack Compose wrapper: it draws
the views the engine publishes and sends back what the person did.

Kotlin only does what the platform must:

- `ui/`: drawing, and how it moves between screens
- `KeystoreSecrets.kt`: the session and the cache's key, encrypted by the Android Keystore
- `MainActivity.kt`: mail links coming in, opening and saving files
- `TernApp.kt`: starting the engine, and telling it when the network changes

## Build

Needs the Android SDK with NDK 30.0.14904198, Rust with the `aarch64-linux-android` and
`x86_64-linux-android` targets, and `cargo-ndk`. Gradle builds the Rust for both ABIs and writes the
Kotlin bindings from the library, so the two cannot disagree.

```sh
cd android
./gradlew :app:assembleProfile     # app/build/outputs/apk/profile/app-profile.apk
```

`assembleProfile` makes a build to try on a phone: release's code, debug-signed, installable beside the
real app as `xyz.hollingworth.Tern.profile`. A plain release build stays unsigned unless given a key.

## Publish

A version tag runs `.github/workflows/release.yml`, as in Fob: one APK for each ABI, about half the size,
and a universal one holding both, with `SHA256SUMS`, on the Releases page beside the Linux downloads. All
share one versionCode, so any installs over any other. The tag must name the version in
`crates/ffi/Cargo.toml`, and needs these repository secrets:

```sh
keytool -genkeypair -v -keystore tern.jks -alias tern -keyalg RSA -keysize 4096 -validity 10000
base64 -w0 tern.jks    # TERN_KEYSTORE_BASE64; the others are TERN_KEYSTORE_PASSWORD, TERN_KEY_ALIAS, TERN_KEY_PASSWORD
```

Keep the keystore safe and out of the repository: updates must be signed with the same key.

## Background checks and notifications

Settings → In the background. Both are off until asked for, and notifications turn the checks on. The
system's WorkManager wakes the app no closer than 15 minutes apart, and later to save battery, so this
is not instant: Tern does not use Tuta's own push. The engine does the check and says what is new;
Kotlin shows it. Sender and subject stay off the lock screen until it is unlocked.

## Test

```sh
cargo test -p tern-app                   # the whole app against the demo account, on the host
ANDROID_SERIAL=emulator-5580 ./gradlew :app:connectedDebugAndroidTest   # the Keystore, on a device
python3 tools/drive.py tour              # taps through every screen on emulator-5580, with screenshots
```

Sign in as `demo` to try the app without an account.

Fonts: Noto Sans and Noto Sans Mono, under the SIL Open Font License (`app/src/main/assets/OFL.txt`).
