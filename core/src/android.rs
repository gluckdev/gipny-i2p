#[cfg(target_os = "android")]
mod android {
    use jni::errors::Result as JniResult;
    use jni::objects::{JClass, JObject, JString, JValue};

    use tauri::Manager;

    static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

    pub fn configure(app: tauri::AppHandle) {
        crate::android_keystore::configure(&app);
        let _ = APP.set(app);
    }

    /// Run JNI on the live Tauri webview thread. Tauri does not initialize the
    /// unrelated ndk-context crate, and borrowed JNI references never leave this
    /// callback. Android still requires the person to authorize installation.
    pub async fn install_apk(path: &str) -> Result<(), String> {
        let app = APP.get().ok_or("Android app is not initialized")?;
        let window = app.get_webview_window("main").ok_or("Android main window is unavailable")?;
        let path = path.to_owned();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let dispatch = std::sync::Arc::new(crate::update_staging::InstallerDispatch::default());
        let callback = dispatch.clone();
        window.with_webview(move |platform| {
            platform.jni_handle().exec(move |env, activity, _webview| {
                // Cancellation and callback start have a single atomic winner.
                if sender.is_closed() || !callback.try_start() { return; }
                // Tauri and wry are built against jni 0.21 while this crate uses
                // 0.22, so the Tauri-owned references are bridged through the raw
                // JNI pointers. Both crates share the same jni-sys pointer types.
                let raw_env = env.get_raw() as *mut jni::sys::JNIEnv;
                let raw_activity = activity.as_raw() as jni::sys::jobject;
                let result = install_raw(raw_env, raw_activity, &path);
                let _ = sender.send(result);
            });
        }).map_err(|error| format!("Android installer dispatch failed: {error}"))?;
        dispatch.wait(receiver, std::time::Duration::from_secs(15)).await
    }

    fn install_raw(
        raw_env: *mut jni::sys::JNIEnv,
        raw_activity: jni::sys::jobject,
        path: &str,
    ) -> Result<(), String> {
        let mut unowned = unsafe { jni::EnvUnowned::from_raw(raw_env) };
        match unowned.with_env(|env| -> JniResult<Result<(), String>> {
            Ok(install_with_context(env, raw_activity, path))
        }).into_outcome() {
            jni::Outcome::Ok(result) => result,
            jni::Outcome::Err(error) => Err(format!("Android APK installer could not start: {error}")),
            jni::Outcome::Panic(_) => Err("Android APK installer could not start: panic".into()),
        }
    }

    fn install_with_context(env: &mut jni::Env<'_>, raw_activity: jni::sys::jobject, path: &str) -> Result<(), String> {
        let result: JniResult<()> = env.with_local_frame(32, |env| {
            let activity = unsafe { JObject::from_raw(env, raw_activity) };
            // This copy belongs to our local frame; Tauri retains its activity.
            let context = env.new_local_ref(&activity)?;
            let package_name = env.call_method(
                &context, jni::jni_str!("getPackageName"), jni::jni_sig!("()Ljava/lang/String;"), &[],
            )?.l()?;
            let package_name = JString::cast_local(env, package_name)?;
            let package: String = env.get_string(&package_name)?.into();
            let authority = env.new_string(format!("{package}.fileprovider"))?;
            let file_path = env.new_string(path)?;
            let file = env.new_object(
                jni::jni_str!("java/io/File"), jni::jni_sig!("(Ljava/lang/String;)V"), &[JValue::Object(&file_path)],
            )?;
            let provider = load_app_class(env, &context, "androidx.core.content.FileProvider")?;
            let uri = env.call_static_method(
                &provider, jni::jni_str!("getUriForFile"),
                jni::jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/io/File;)Landroid/net/Uri;"),
                &[JValue::Object(&context), JValue::Object(&authority), JValue::Object(&file)],
            )?.l()?;
            let action = env.new_string("android.intent.action.VIEW")?;
            let intent = env.new_object(
                jni::jni_str!("android/content/Intent"), jni::jni_sig!("(Ljava/lang/String;)V"), &[JValue::Object(&action)],
            )?;
            let mime = env.new_string("application/vnd.android.package-archive")?;
            env.call_method(
                &intent, jni::jni_str!("setDataAndType"),
                jni::jni_sig!("(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;"),
                &[JValue::Object(&uri), JValue::Object(&mime)],
            )?;
            // NEW_TASK is needed for a Context that is not an Activity. Grant
            // only read access to this content URI; do not mark the source trusted.
            env.call_method(
                &intent, jni::jni_str!("addFlags"), jni::jni_sig!("(I)Landroid/content/Intent;"),
                &[JValue::Int(0x10000000 | 0x00000001)],
            )?;
            env.call_method(
                &context, jni::jni_str!("startActivity"), jni::jni_sig!("(Landroid/content/Intent;)V"),
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        });
        if result.is_err() && env.exception_check() {
            // Return a Rust error instead of leaking a pending Java exception
            // into later calls. Do not dump exception details containing paths.
            env.exception_clear();
        }
        result.map_err(|error| format!("Android APK installer could not start: {error}"))
    }

    fn load_app_class<'a>(
        env: &mut jni::Env<'a>,
        context: &JObject<'a>,
        name: &str,
    ) -> jni::errors::Result<JClass<'a>> {
        let class_loader = env
            .call_method(context, jni::jni_str!("getClassLoader"), jni::jni_sig!("()Ljava/lang/ClassLoader;"), &[])?
            .l()?;
        let class_name = env.new_string(name)?;
        let class_obj = env
            .call_method(
                &class_loader,
                jni::jni_str!("loadClass"),
                jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                &[JValue::Object(&class_name)],
            )?
            .l()?;
        JClass::cast_local(env, class_obj)
    }
}

#[cfg(not(target_os = "android"))]
mod android {
    pub async fn install_apk(_path: &str) -> Result<(), String> { Err("not on Android".into()) }
}

#[cfg(target_os = "android")]
pub use android::configure;
pub use android::install_apk;
