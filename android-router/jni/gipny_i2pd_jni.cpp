/*
 * JNI entry points for gipny's embedded i2pd.
 *
 * Upstream i2pd-android exports Java_org_purplei2p_i2pd_I2PD_1JNI_* and drives a
 * full router UI. gipny needs two calls and nothing else: start the daemon
 * against a data directory the service prepared, and stop it. The daemon plumbing
 * itself is upstream's DaemonAndroid.cpp, which is compiled in alongside this
 * file — the only thing that is ours is the symbol names and the narrow surface.
 *
 * Everything about *how* the router runs (SAM on, console and proxies off) comes
 * from the i2pd.conf that GipnyService.kt writes into the data dir before calling
 * in: DaemonAndroid::start() passes no argv, so config is the only channel.
 */

#include <jni.h>
#include <string>

#include "DaemonAndroid.h"
#include "Transports.h"

namespace {

std::string jstring_to_std(JNIEnv *env, jstring s) {
	if (!s) return std::string();
	const char *chars = env->GetStringUTFChars(s, nullptr);
	if (!chars) return std::string();
	std::string out(chars);
	env->ReleaseStringUTFChars(s, chars);
	return out;
}

} // namespace

extern "C" {

/*
 * Returns null on success, or an error string the service logs and surfaces.
 *
 * samListen is accepted for symmetry with the old go-i2p shim and to keep the
 * port contract visible at the call site; i2pd itself reads the address and port
 * from i2pd.conf, so this only guards against a caller expecting a different one.
 */
JNIEXPORT jstring JNICALL
Java_app_gipny_GipnyService_nativeStartSam(JNIEnv *env, jobject /*thiz*/, jstring dataDir, jstring samListen) {
	const std::string dir = jstring_to_std(env, dataDir);
	if (dir.empty())
		return env->NewStringUTF("empty data dir");

	i2p::android::SetDataDir(dir);

	// "ok" is success here; anything else is the failure detail.
	const std::string result = i2p::android::start();
	if (result != "ok")
		return env->NewStringUTF(result.c_str());

	(void)samListen;
	return nullptr;
}

JNIEXPORT void JNICALL
Java_app_gipny_GipnyService_nativeStopSam(JNIEnv * /*env*/, jobject /*thiz*/) {
	i2p::android::stop();
}

/*
 * The phone's network changed: Wi-Fi to LTE and back, or out of sleep. The
 * router is not told otherwise, and keeps the reachability it measured on the
 * old network — on a phone that went away and came back, no tunnels and
 * "Destination to connect not found" for our own relay (seen 2026-09-24 on a
 * Realme after a Wi-Fi/LTE switch). Going offline and online again makes it
 * test how the new network sees it (Transports::SetOnline → PeerTest), as
 * upstream i2pd-android does from its NetworkCallback.
 */
JNIEXPORT void JNICALL
Java_app_gipny_GipnyService_nativeNetworkChanged(JNIEnv * /*env*/, jobject /*thiz*/, jboolean online) {
	i2p::transport::transports.SetOnline(online == JNI_TRUE);
}

} // extern "C"
