NDK_TOOLCHAIN_VERSION := clang
APP_STL := c++_static

# C++20, as in third_party/i2pd-android/native/jni/Application.mk: upstream
# libi2pd/Tag.h uses operator<=>.
APP_CPPFLAGS += -std=c++20 -fexceptions -frtti
# As i2p-embed/build.rs: libi2pd's globals are not destroyed at exit (their
# order is the linker's, and ~Tunnels outlived a mutex it needs on macOS); the
# router is stopped before exit by the shim.
APP_CPPFLAGS += -fno-c++-static-destructors

# The embedded Rust manager uses our pinned miniupnpc C library for the
# router's own NTCP2/SSU2 ports. The upstream daemon's UPnP manager is not built.
APP_CPPFLAGS += -DANDROID -D__ANDROID__ -Wno-deprecated-declarations
ifeq ($(TARGET_ARCH_ABI),armeabi-v7a)
APP_CPPFLAGS += -DANDROID_ARM7A
endif

# fetch_deps.sh unpacks sources and build_openssl.sh builds OpenSSL under
# third_party/i2pd-android/binary/jni, which NDK_MODULE_PATH points at.
# Our Android.mk compiles Boost program_options and our pinned i2pd sources.
IFADDRS_PATH  = $(NDK_MODULE_PATH)/ifaddrs
BOOST_PATH    = $(NDK_MODULE_PATH)/deps/boost
OPENSSL_PATH  = $(NDK_MODULE_PATH)/deps/openssl

I2PD_SRC_PATH = $(NDK_MODULE_PATH)/i2pd
LIB_SRC_PATH  = $(I2PD_SRC_PATH)/libi2pd

# gipny's C interface over libi2pd/api.h, shared with the desktop builds.
SHIM_PATH = $(abspath $(NDK_PROJECT_PATH)/../i2p-embed/shim)
MINIUPNPC_PATH = $(abspath $(SHIM_PATH)/../vendor/miniupnpc)
