// JNI shim for the in-process i2p router on Android.
//
// This file is only compiled when GOOS=android (Go's build system applies
// GOOS/GOARCH filename-suffix constraints to .c files the same way it does to
// .go files), so it never affects the desktop CLI build.
//
// It bridges the JNI calls made from Kotlin (GipnyService.kt, declared as
// `external fun`) to the plain C-ABI functions exported by android_export.go
// via cgo (`//export StartSam` / `//export StopSam` / `//export FreeCString`).
#include <jni.h>
#include <stdlib.h>

// Implemented in android_export.go, exported via cgo.
extern char *StartSam(char *dataDir, char *samListen);
extern void StopSam(void);
extern void FreeCString(char *s);

// Java_app_gipny_GipnyService_nativeStartSam(dataDir: String, samListen: String): String?
// Returns null on success, or an error message string on failure.
JNIEXPORT jstring JNICALL
Java_app_gipny_GipnyService_nativeStartSam(JNIEnv *env, jobject thiz, jstring dataDir, jstring samListen) {
    (void)thiz;
    const char *c_data_dir = (*env)->GetStringUTFChars(env, dataDir, NULL);
    if (c_data_dir == NULL) {
        return NULL;
    }
    const char *c_sam_listen = (*env)->GetStringUTFChars(env, samListen, NULL);
    if (c_sam_listen == NULL) {
        (*env)->ReleaseStringUTFChars(env, dataDir, c_data_dir);
        return NULL;
    }

    char *err = StartSam((char *)c_data_dir, (char *)c_sam_listen);

    (*env)->ReleaseStringUTFChars(env, dataDir, c_data_dir);
    (*env)->ReleaseStringUTFChars(env, samListen, c_sam_listen);

    if (err == NULL) {
        return NULL;
    }
    jstring result = (*env)->NewStringUTF(env, err);
    FreeCString(err);
    return result;
}

// Java_app_gipny_GipnyService_nativeStopSam(): Unit
JNIEXPORT void JNICALL
Java_app_gipny_GipnyService_nativeStopSam(JNIEnv *env, jobject thiz) {
    (void)env;
    (void)thiz;
    StopSam();
}
