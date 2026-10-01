#[cfg(target_os = "android")]
mod android {
    use jni::objects::{JClass, JObject, JString, JValue};
    use jni::sys::{jboolean, jint, jlong};
    use jni::JNIEnv;
    use std::ffi::CString;
    use std::os::raw::c_char;

    /// Calls Android's PackageInstaller to install the APK at the given path.
    /// Uses FileProvider to get a content:// URI for the file.
    pub fn install_apk(path: &str) -> Result<(), String> {
        let vm = unsafe { jni::JavaVM::from_raw(ndk_context::android_context() as *mut _) }
            .map_err(|e| format!("failed to get JavaVM: {e}"))?;

        let mut env = vm.get_env().map_err(|e| format!("failed to get JNIEnv: {e}"))?;

        let context = ndk_context::android_context();
        let context_obj = unsafe { JObject::from_raw(context as *mut _) };

        let file_provider_authority = format!("{}.fileprovider", get_package_name(&mut env, &context_obj)?);
        let file = std::path::Path::new(path);
        let file_name = file.file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid file path")?;

        let file_class = env.find_class("java/io/File").map_err(|e| format!("find File class: {e}"))?;
        let file_obj = env.new_object(
            &file_class,
            "(Ljava/lang/String;)V",
            &[JValue::Object(env.new_string(path).map_err(|e| format!("new path string: {e}"))?.into())],
        ).map_err(|e| format!("new File: {e}"))?;

        let file_provider_class = env.find_class("androidx/core/content/FileProvider")
            .map_err(|e| format!("find FileProvider class: {e}"))?;
        let uri = env.call_static_method(
            &file_provider_class,
            "getUriForFile",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/io/File;)Landroid/net/Uri;",
            &[
                JValue::Object(&context_obj),
                JValue::Object(env.new_string(&file_provider_authority).map_err(|e| format!("new authority string: {e}"))?.into()),
                JValue::Object(&file_obj),
            ],
        ).map_err(|e| format!("getUriForFile: {e}"))?;

        let intent_class = env.find_class("android/content/Intent").map_err(|e| format!("find Intent class: {e}"))?;
        let intent = env.new_object(
            &intent_class,
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[
                JValue::Object(env.new_string("android.intent.action.VIEW").map_err(|e| format!("new action string: {e}"))?.into()),
                JValue::Object(uri.l().unwrap()),
            ],
        ).map_err(|e| format!("new Intent: {e}"))?;

        env.call_void_method(
            &intent,
            "setFlags",
            "(I)V",
            &[JValue::Int(0x04000000 | 0x00000001)],
        ).map_err(|e| format!("setFlags: {e}"))?;

        env.call_void_method(
            &intent,
            "addFlags",
            "(I)V",
            &[JValue::Int(0x00000001)],
        ).map_err(|e| format!("addFlags: {e}"))?;

        env.call_void_method(
            &intent,
            "setDataAndType",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;",
            &[
                JValue::Object(uri.l().unwrap()),
                JValue::Object(env.new_string("application/vnd.android.package-archive").map_err(|e| format!("new mime string: {e}"))?.into()),
            ],
        ).map_err(|e| format!("setDataAndType: {e}"))?;

        env.call_void_method(
            &intent,
            "putExtra",
            "(Ljava/lang/String;Z)Landroid/content/Intent;",
            &[
                JValue::Object(env.new_string("android.intent.extra.NOT_UNKNOWN_SOURCE").map_err(|e| format!("new extra string: {e}"))?.into()),
                JValue::Bool(1),
            ],
        ).map_err(|e| format!("putExtra NOT_UNKNOWN_SOURCE: {e}"))?;

        let activity_class = env.find_class("android/app/Activity").map_err(|e| format!("find Activity class: {e}"))?;
        env.call_void_method(
            &context_obj,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        ).map_err(|e| format!("startActivity: {e}"))?;

        Ok(())
    }

    fn get_package_name(env: &mut JNIEnv, context: &JObject) -> Result<String, String> {
        let context_class = env.find_class("android/content/Context").map_err(|e| format!("find Context class: {e}"))?;
        let package_name = env.call_method(
            context,
            "getPackageName",
            "()Ljava/lang/String;",
            &[],
        ).map_err(|e| format!("getPackageName: {e}"))?;

        let package_name_str: JString = package_name.l().unwrap().into();
        env.get_string(&package_name_str).map_err(|e| format!("get package name string: {e}")).map(|s| s.into())
    }
}

#[cfg(not(target_os = "android"))]
mod android {
    pub fn install_apk(_path: &str) -> Result<(), String> {
        Err("not on Android".into())
    }
}

pub use android::install_apk;