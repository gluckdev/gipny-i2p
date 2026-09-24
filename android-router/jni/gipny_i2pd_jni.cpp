/*
 * The one JNI entry point in gipny's libi2pd.so.
 *
 * The router itself is started, used and stopped by the app's Rust side,
 * in process, through the C shim compiled into this same library
 * (i2p-embed/shim). What only the Java side can see is the phone's network
 * changing, so that is the whole of this file.
 */

#include <jni.h>

#include "shim.h"

extern "C" {

/*
 * The phone's network changed: Wi-Fi to LTE and back, or out of sleep. The
 * router is not told otherwise, and keeps the reachability it measured on the
 * old network — on a phone that went away and came back, no tunnels and
 * "Destination to connect not found" for our own relay (seen 2026-09-24 on a
 * Realme after a Wi-Fi/LTE switch). Going offline and online again makes it
 * test how the new network sees it (Transports::SetOnline → PeerTest), as
 * upstream i2pd-android does from its NetworkCallback. Before the Rust side
 * has started the router this does nothing.
 */
JNIEXPORT void JNICALL
Java_app_gipny_GipnyService_nativeNetworkChanged(JNIEnv * /*env*/, jobject /*thiz*/, jboolean online) {
	gipny_router_set_online(online == JNI_TRUE ? 1 : 0);
}

} // extern "C"
