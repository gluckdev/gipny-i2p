# Builds libi2pd.so: upstream i2pd plus gipny's two JNI entry points.
#
# Derived from third_party/i2pd-android/app/jni/Android.mk. Differences, both
# deliberate: i2pd_android.cpp is replaced by gipny_i2pd_jni.cpp (so the library
# exports Java_app_gipny_GipnyService_* and nothing else), and miniupnpc is
# dropped along with USE_UPNP.
LOCAL_PATH := $(call my-dir)
include $(CLEAR_VARS)
LOCAL_MODULE := i2pd
LOCAL_CPP_FEATURES := rtti exceptions
LOCAL_C_INCLUDES += $(IFADDRS_PATH) $(LIB_SRC_PATH) $(LIB_CLIENT_SRC_PATH) $(LANG_SRC_PATH) $(DAEMON_SRC_PATH) $(UPSTREAM_JNI_PATH)
LOCAL_STATIC_LIBRARIES := \
	boost_program_options \
	crypto \
	ssl
LOCAL_LDLIBS := -lz

LOCAL_SRC_FILES := \
	gipny_i2pd_jni.cpp \
	$(UPSTREAM_JNI_PATH)/DaemonAndroid.cpp \
	$(IFADDRS_PATH)/ifaddrs.cpp \
	$(IFADDRS_PATH)/bionic_netlink.cpp \
	$(wildcard $(LIB_SRC_PATH)/*.cpp) \
	$(wildcard $(LIB_CLIENT_SRC_PATH)/*.cpp) \
	$(wildcard $(LANG_SRC_PATH)/*.cpp) \
	$(DAEMON_SRC_PATH)/Daemon.cpp \
	$(DAEMON_SRC_PATH)/UPnP.cpp \
	$(DAEMON_SRC_PATH)/HTTPServer.cpp \
	$(DAEMON_SRC_PATH)/I2PControl.cpp \
	$(DAEMON_SRC_PATH)/I2PControlHandlers.cpp

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
