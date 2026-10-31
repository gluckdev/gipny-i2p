//! Android's non-exportable hardware wrapping key stays in AndroidKeyStore.
//! Only the small binding secret crosses JNI, in explicitly wiped byte arrays.
use std::sync::{Arc, LazyLock};

use gipny_libcore::device_binding::{self, BindingError, DeviceKeyProvider, Result};
use jni::errors::Result as JniResult;
use jni::objects::{Global, JByteArray, JClass, JObject, JValue};
use tauri::Manager;
use zeroize::Zeroizing;

struct Provider {
    vm: jni::JavaVM,
    class: Global<JClass<'static>>,
    ui_thread: std::thread::ThreadId,
}

// send_replace retains the result even before the first command subscribes.
// Commands wait asynchronously, so a fast first click cannot race enrollment.
static READY: LazyLock<tokio::sync::watch::Sender<u8>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);

pub async fn wait_ready() -> std::result::Result<(), String> {
    let mut state = READY.subscribe();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match *state.borrow_and_update() {
                1 => return Ok(()),
                2 => return Err("Android protected credential provider could not initialize".into()),
                _ => {}
            }
            state.changed().await.map_err(|_| "Android credential initialization closed".to_string())?;
        }
    }).await.map_err(|_| "Android protected credential provider is not ready".to_string())?
}

/// Capture the actual Tauri JVM and application class loader. Do not create a
/// second VM or depend on ndk-context, which Tauri does not initialize.
pub fn configure(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { READY.send_replace(2); return; };
    let dispatched = window.with_webview(|platform| {
        platform.jni_handle().exec(|env, activity, _webview| {
            // Tauri and wry are built against jni 0.21 while this crate uses
            // 0.22, so the Tauri-owned references are bridged through the raw
            // JNI pointers. Both crates share the same jni-sys pointer types.
            let raw_env = env.get_raw() as *mut jni::sys::JNIEnv;
            let raw_activity = activity.as_raw() as jni::sys::jobject;
            let ready = with_tauri_env(raw_env, |env| {
                let result: JniResult<bool> = env.with_local_frame(16, |env| {
                    let activity = unsafe { JObject::from_raw(env, raw_activity) };
                    let loader = env.call_method(&activity, jni::jni_str!("getClassLoader"), jni::jni_sig!("()Ljava/lang/ClassLoader;"), &[])?.l()?;
                    let name = env.new_string("app.gipny.DeviceKeystore")?;
                    let class = env.call_method(&loader, jni::jni_str!("loadClass"), jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"), &[JValue::Object(&name)])?.l()?;
                    let provider = Provider {
                        vm: env.get_java_vm()?,
                        class: env.new_cast_global_ref::<JClass>(class)?,
                        ui_thread: std::thread::current().id(),
                    };
                    // OnceLock rejects duplicate initialization; never replace an
                    // active provider or weaken a failed Keystore initialization.
                    Ok(device_binding::install_provider(Arc::new(provider)).is_ok())
                });
                let ready = matches!(&result, Ok(true));
                if result.is_err() { let _ = failure(env); }
                Ok(ready)
            }).unwrap_or(false);
            READY.send_replace(if ready { 1 } else { 2 });
        });
    });
    if dispatched.is_err() { READY.send_replace(2); }
}

fn with_tauri_env<T>(
    raw_env: *mut jni::sys::JNIEnv,
    f: impl FnOnce(&mut jni::Env<'_>) -> JniResult<T>,
) -> Option<T> {
    let mut unowned = unsafe { jni::EnvUnowned::from_raw(raw_env) };
    match unowned.with_env(f).into_outcome() {
        jni::Outcome::Ok(value) => Some(value),
        jni::Outcome::Err(_) | jni::Outcome::Panic(_) => {
            clear_pending_exception(raw_env);
            None
        }
    }
}

fn clear_pending_exception(raw_env: *mut jni::sys::JNIEnv) {
    let mut guard = unsafe { jni::AttachGuard::from_unowned(raw_env) };
    let env = guard.borrow_env_mut();
    if env.exception_check() { env.exception_clear(); }
}

fn failure(env: &mut jni::Env<'_>) -> BindingError {
    let missing = if env.exception_check() {
        let exception = env.exception_occurred();
        env.exception_clear();
        exception.as_ref().is_some_and(|exception| {
            env.is_instance_of(exception, jni::jni_str!("java/security/UnrecoverableKeyException")).unwrap_or(false)
        })
    } else { false };
    // is_instance_of may itself fail. Leave no pending exception for the next
    // JNI operation and never log platform exception strings with user data.
    if env.exception_check() { env.exception_clear(); }
    if missing { BindingError::MissingCredential }
    else { BindingError::Unavailable("hardware Android Keystore operation failed") }
}

fn wipe(env: &mut jni::Env<'_>, bytes: &JByteArray<'_>) -> JniResult<()> {
    env.call_static_method(jni::jni_str!("java/util/Arrays"), jni::jni_str!("fill"), jni::jni_sig!("([BB)V"), &[
        JValue::Object(bytes.as_ref()), JValue::Byte(0),
    ])?;
    Ok(())
}

impl Provider {
    fn attached<T>(&self, f: impl FnOnce(&mut jni::Env<'_>) -> JniResult<T>) -> Result<T> {
        if std::thread::current().id() == self.ui_thread {
            return Err(BindingError::Unavailable("Keystore must run on a background command thread"));
        }
        self.vm.attach_current_thread(|env| {
            Ok::<_, jni::errors::Error>(match f(env) {
                Ok(value) => Ok(value),
                Err(_) => Err(failure(env)),
            })
        }).map_err(|_| BindingError::Unavailable("Android JVM thread attachment failed"))?
    }
}

impl DeviceKeyProvider for Provider {
    fn kind(&self) -> &'static str { "android-keystore-hardware-v2" }

    fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
        self.attached(|env| {
            let result: JniResult<Result<Vec<u8>>> = env.with_local_frame(16, |env| {
                let id = env.new_string(id)?;
                let input = env.byte_array_from_slice(secret)?;
                let called = env.call_static_method(&self.class, jni::jni_str!("seal"), jni::jni_sig!("(Ljava/lang/String;[B)[B"), &[
                    JValue::Object(&id), JValue::Object(input.as_ref()),
                ]);
                let output = match called {
                    Ok(value) => Ok(JByteArray::cast_local(env, value.l()?)?),
                    Err(_) => Err(failure(env)),
                };
                // Kotlin also wipes in finally. Cover method-resolution and other
                // JNI failures that happen before the Kotlin method starts.
                wipe(env, &input)?;
                let output = match output { Ok(output) => output, Err(error) => return Ok(Err(error)) };
                if output.len(env)? != 60 { return Ok(Err(BindingError::InvalidMetadata)); }
                Ok(Ok(env.convert_byte_array(&output)?))
            });
            result
        })?
    }

    fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        if wrapped.len() != 60 { return Err(BindingError::InvalidMetadata); }
        self.attached(|env| {
            let result: JniResult<Result<Zeroizing<[u8; 32]>>> = env.with_local_frame(16, |env| {
                let id = env.new_string(id)?;
                let input = env.byte_array_from_slice(wrapped)?;
                let called = env.call_static_method(&self.class, jni::jni_str!("unseal"), jni::jni_sig!("(Ljava/lang/String;[B)[B"), &[
                    JValue::Object(&id), JValue::Object(input.as_ref()),
                ]);
                let output = match called {
                    Ok(value) => JByteArray::cast_local(env, value.l()?)?,
                    Err(_) => return Ok(Err(failure(env))),
                };
                if output.len(env)? != 32 {
                    wipe(env, &output)?;
                    return Ok(Err(BindingError::InvalidMetadata));
                }
                let copied = env.convert_byte_array(&output).map(Zeroizing::new);
                if copied.is_err() { let _ = failure(env); }
                wipe(env, &output)?;
                let bytes = copied?;
                let mut secret = Zeroizing::new([0u8; 32]);
                secret.copy_from_slice(&bytes);
                Ok(Ok(secret))
            });
            result
        })?
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.attached(|env| {
            let result: JniResult<()> = env.with_local_frame(16, |env| {
                let id = env.new_string(id)?;
                env.call_static_method(&self.class, jni::jni_str!("delete"), jni::jni_sig!("(Ljava/lang/String;)V"), &[JValue::Object(&id)])?;
                Ok(())
            });
            result
        })
    }
}
