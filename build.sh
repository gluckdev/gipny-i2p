#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

IMG_LIN="${IMG_LIN:-gipny-builder-linux}"
IMG_WIN="${IMG_WIN:-gipny-builder-win}"

DO_LIN=1
DO_WIN=1
DO_AND=1
WIPE=0
for arg in "$@"; do
    case "$arg" in
        --linux-only)    DO_WIN=0; DO_AND=0 ;;
        --windows-only)  DO_LIN=0; DO_AND=0 ;;
        --android-only)  DO_LIN=0; DO_WIN=0 ;;
        --no-linux)      DO_LIN=0 ;;
        --no-windows)    DO_WIN=0 ;;
        --no-android)    DO_AND=0 ;;
        --wipe)          WIPE=1 ;;
        *) echo "unknown arg: $arg" >&2; exit 1 ;;
    esac
done

SUDO=""
if [[ $DO_LIN -eq 1 || $DO_WIN -eq 1 ]]; then
    if ! command -v docker >/dev/null; then
        echo "docker not found (нужен для linux/windows билдов)" >&2; exit 1
    fi
    if ! docker info >/dev/null 2>&1; then
        SUDO="sudo"
    fi
fi

if [[ $WIPE -eq 1 ]]; then
    rm -rf release-artifacts
fi
mkdir -p release-artifacts

if [[ $DO_LIN -eq 1 ]]; then
    echo "[*] building linux image ($IMG_LIN)"
    $SUDO docker build -t "$IMG_LIN" -f Dockerfile.build .
    echo "[*] running linux build"
    $SUDO docker run --rm -v "$(pwd):/src" "$IMG_LIN"
fi

if [[ $DO_WIN -eq 1 ]]; then
    echo "[*] building windows image ($IMG_WIN)"
    $SUDO docker build -t "$IMG_WIN" -f Dockerfile.win .
    echo "[*] running windows build"
    $SUDO docker run --rm -v "$(pwd):/src" "$IMG_WIN"
fi

if [[ $DO_AND -eq 1 ]]; then
    echo "[*] building Android APKs (debug, arm64-v8a + x86_64)"
    : "${ANDROID_HOME:=$HOME/Android/Sdk}"
    : "${ANDROID_SDK_ROOT:=$ANDROID_HOME}"
    : "${NDK_HOME:=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk 2>/dev/null | sort -V | tail -1)}"
    : "${ANDROID_NDK_ROOT:=$NDK_HOME}"
    : "${ANDROID_NDK_HOME:=$NDK_HOME}"
    if [[ -z "${JAVA_HOME:-}" ]]; then
        for cand in /usr/lib/jvm/openjdk17 /usr/lib/jvm/java-17-openjdk /usr/lib/jvm/java-17-openjdk-amd64 /opt/android-studio/jbr; do
            if [[ -d "$cand" ]]; then JAVA_HOME="$cand"; break; fi
        done
    fi
    : "${JAVA_HOME:=/usr/lib/jvm/openjdk17}"
    if [[ ! -d "$ANDROID_HOME" || ! -d "$NDK_HOME" || ! -d "$JAVA_HOME" ]]; then
        echo "  ANDROID_HOME=$ANDROID_HOME"
        echo "  NDK_HOME=$NDK_HOME"
        echo "  JAVA_HOME=$JAVA_HOME"
        echo "[!] missing Android SDK/NDK/JDK17 — install or set env vars (or run with --no-android)" >&2
        exit 1
    fi
    export ANDROID_HOME ANDROID_SDK_ROOT NDK_HOME ANDROID_NDK_ROOT ANDROID_NDK_HOME JAVA_HOME
    export PATH=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$ANDROID_HOME/platform-tools:$JAVA_HOME/bin:$PATH
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang
    export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/x86_64-linux-android24-clang
    export CC_aarch64_linux_android=aarch64-linux-android24-clang
    export CXX_aarch64_linux_android=aarch64-linux-android24-clang++
    export CC_x86_64_linux_android=x86_64-linux-android24-clang
    export CXX_x86_64_linux_android=x86_64-linux-android24-clang++
    export AR_aarch64_linux_android=llvm-ar
    export AR_x86_64_linux_android=llvm-ar
    export RANLIB_aarch64_linux_android=llvm-ranlib
    export RANLIB_x86_64_linux_android=llvm-ranlib

    VERSION=$(grep '^version' core/Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
    APK_OUT=core/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk

    (cd ui && npm install && npm run build)
    for arch in aarch64 x86_64; do
        echo "[*] android $arch"
        (cd core && cargo tauri android build --debug --apk --target "$arch")
        case "$arch" in
            aarch64) suffix=arm64 ;;
            x86_64)  suffix=x86_64 ;;
        esac
        cp "$APK_OUT" "release-artifacts/gipny-${VERSION}-android-${suffix}.apk"
    done
fi

if [[ -n "$SUDO" ]]; then
    $SUDO chown -R "$(id -u):$(id -g)" release-artifacts 2>/dev/null || true
fi

echo
echo "[+] all artifacts:"
ls -lh release-artifacts/
