NDK_TOOLCHAIN_VERSION := clang
APP_STL := c++_static

# C++20, not the c++17 i2pd-android still uses: upstream i2pd dropped C++17
# (libi2pd/Tag.h now defines operator<=>), and its own Makefile asks clang for
# c++20. The pinned revision builds either way, so this costs nothing today and
# keeps the pin bump from breaking the Android router.
APP_CPPFLAGS += -std=c++20 -fexceptions -frtti

# NO_TORRENTS: the Android.mk wildcard picks up upstream's BitTorrent client and
# its JSON-RPC server, which need boost_json — a library build_boost.sh does not
# build. The desktop builds pass TORRENTS=no, which defines the same thing.
APP_CPPFLAGS += -DNO_TORRENTS

# No USE_UPNP: gipny only ever talks to a loopback SAM port, so UPnP is a
# dependency (miniupnpc) and a hole-punching side effect nobody asked for. This
# matches the desktop build, which passes USE_UPNP=no.
APP_CPPFLAGS += -DANDROID -D__ANDROID__ -Wno-deprecated-declarations
ifeq ($(TARGET_ARCH_ABI),armeabi-v7a)
APP_CPPFLAGS += -DANDROID_ARM7A
endif

# Dependencies and the i2pd sources are built by i2pd-android's own scripts under
# third_party/i2pd-android/binary/jni, which is what NDK_MODULE_PATH points at.
IFADDRS_PATH  = $(NDK_MODULE_PATH)/android-ifaddrs
BOOST_PATH    = $(NDK_MODULE_PATH)/boost
OPENSSL_PATH  = $(NDK_MODULE_PATH)/openssl

I2PD_SRC_PATH = $(NDK_MODULE_PATH)/i2pd

LIB_SRC_PATH        = $(I2PD_SRC_PATH)/libi2pd
LIB_CLIENT_SRC_PATH = $(I2PD_SRC_PATH)/libi2pd_client
LANG_SRC_PATH       = $(I2PD_SRC_PATH)/i18n
DAEMON_SRC_PATH     = $(I2PD_SRC_PATH)/daemon

# DaemonAndroid.cpp lives in i2pd-android's app/jni; we compile it, not its
# i2pd_android.cpp, so the only JNI symbols in the library are ours.
UPSTREAM_JNI_PATH = $(NDK_MODULE_PATH)/../../app/jni
