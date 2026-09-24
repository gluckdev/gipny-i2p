NDK_TOOLCHAIN_VERSION := clang
APP_STL := c++_static

# C++20, not the c++17 i2pd-android still uses: upstream i2pd dropped C++17
# (libi2pd/Tag.h now defines operator<=>), and its own Makefile asks clang for
# c++20. The pinned revision builds either way, so this costs nothing today and
# keeps the pin bump from breaking the Android router.
APP_CPPFLAGS += -std=c++20 -fexceptions -frtti
# As i2p-embed/build.rs: libi2pd's globals are not destroyed at exit (their
# order is the linker's, and ~Tunnels outlived a mutex it needs on macOS); the
# router is stopped before exit by the shim.
APP_CPPFLAGS += -fno-c++-static-destructors

# No USE_UPNP: the router is only ever reached through its own api in this
# process, so UPnP is a dependency (miniupnpc) and a hole-punching side effect
# nobody asked for. The desktop builds do not enable it either.
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
LIB_SRC_PATH  = $(I2PD_SRC_PATH)/libi2pd

# gipny's C interface over libi2pd/api.h, shared with the desktop builds.
SHIM_PATH = $(abspath $(NDK_PROJECT_PATH)/../i2p-embed/shim)
