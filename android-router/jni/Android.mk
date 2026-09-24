# Builds libi2pd.so: the i2p router library (upstream libi2pd), gipny's C shim
# over its api.h (i2p-embed/shim, the same one the desktop builds compile in),
# and one JNI entry point for the network callback.
#
# The app's Rust side links this library and starts the router itself, in
# process: no daemon, no SAM, no proxies, no port. So libi2pd_client (SAM,
# proxies, tunnels), the daemon and its web console, and i18n are not built.
# Derived from third_party/i2pd-android/app/jni/Android.mk; miniupnpc is
# dropped along with USE_UPNP.
LOCAL_PATH := $(call my-dir)
include $(CLEAR_VARS)
LOCAL_MODULE := i2pd
LOCAL_CPP_FEATURES := rtti exceptions
LOCAL_C_INCLUDES += $(IFADDRS_PATH) $(LIB_SRC_PATH) $(SHIM_PATH)
LOCAL_STATIC_LIBRARIES := \
	boost_program_options \
	crypto \
	ssl
LOCAL_LDLIBS := -lz

LOCAL_SRC_FILES := \
	gipny_i2pd_jni.cpp \
	$(SHIM_PATH)/shim.cpp \
	$(IFADDRS_PATH)/ifaddrs.cpp \
	$(IFADDRS_PATH)/bionic_netlink.cpp \
	$(wildcard $(LIB_SRC_PATH)/*.cpp)

include $(BUILD_SHARED_LIBRARY)

LOCAL_PATH := $(call my-dir)
include $(CLEAR_VARS)
LOCAL_MODULE := boost_program_options
LOCAL_SRC_FILES := $(BOOST_PATH)/build/out/$(TARGET_ARCH_ABI)/lib/libboost_program_options.a
LOCAL_EXPORT_C_INCLUDES := $(BOOST_PATH)/build/out/$(TARGET_ARCH_ABI)/include
include $(PREBUILT_STATIC_LIBRARY)

LOCAL_PATH := $(call my-dir)
include $(CLEAR_VARS)
LOCAL_MODULE := crypto
LOCAL_SRC_FILES := $(OPENSSL_PATH)/out/$(TARGET_ARCH_ABI)/lib/libcrypto.a
LOCAL_EXPORT_C_INCLUDES := $(OPENSSL_PATH)/out/$(TARGET_ARCH_ABI)/include
include $(PREBUILT_STATIC_LIBRARY)

LOCAL_PATH := $(call my-dir)
include $(CLEAR_VARS)
LOCAL_MODULE := ssl
LOCAL_SRC_FILES := $(OPENSSL_PATH)/out/$(TARGET_ARCH_ABI)/lib/libssl.a
LOCAL_EXPORT_C_INCLUDES := $(OPENSSL_PATH)/out/$(TARGET_ARCH_ABI)/include
LOCAL_STATIC_LIBRARIES := crypto
include $(PREBUILT_STATIC_LIBRARY)
