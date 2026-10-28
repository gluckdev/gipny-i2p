use super::*;
use std::ffi::c_void;
use zeroize::Zeroize;

type Ref = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kCFTypeArrayCallBacks: u8;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
    fn CFDictionaryCreate(allocator: Ref, keys: *const Ref, values: *const Ref, count: isize, key_callbacks: Ref, value_callbacks: Ref) -> Ref;
    fn CFArrayCreate(allocator: Ref, values: *const Ref, count: isize, callbacks: Ref) -> Ref;
    fn CFStringCreateWithBytes(allocator: Ref, bytes: *const u8, count: isize, encoding: u32, external: u8) -> Ref;
    fn CFDataCreateMutable(allocator: Ref, capacity: isize) -> *mut c_void;
    fn CFDataAppendBytes(data: *mut c_void, bytes: *const u8, count: isize);
    fn CFDataGetMutableBytePtr(data: *mut c_void) -> *mut u8;
    fn CFDataGetBytePtr(data: Ref) -> *const u8;
    fn CFDataGetLength(data: Ref) -> isize;
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFRelease(value: Ref);
}
#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecClass: Ref;
    static kSecClassGenericPassword: Ref;
    static kSecAttrService: Ref;
    static kSecAttrAccount: Ref;
    static kSecUseDataProtectionKeychain: Ref;
    static kSecUseKeychain: Ref;
    static kSecMatchSearchList: Ref;
    static kSecValueData: Ref;
    static kSecReturnData: Ref;
    fn SecItemAdd(attributes: Ref, result: *mut Ref) -> i32;
    fn SecItemCopyMatching(query: Ref, result: *mut Ref) -> i32;
    fn SecItemDelete(query: Ref) -> i32;
    fn SecKeychainCopyDefault(keychain: *mut Ref) -> i32;
}

struct Object(Ref);
impl Object {
    fn checked(value: Ref) -> Result<Self> {
        if value.is_null() { Err(BindingError::Unavailable("macOS protected Keychain allocation failed")) } else { Ok(Self(value)) }
    }
}
impl Drop for Object { fn drop(&mut self) { unsafe { CFRelease(self.0); } } }

struct SecretData(Object);
impl Drop for SecretData {
    fn drop(&mut self) {
        // This CFMutableData belongs to us; clear the native input copy before
        // releasing it. SecItemCopyMatching's OS-owned result has its own short
        // framework lifetime and is not represented as a persistent Rust key.
        unsafe {
            let data = self.0.0.cast_mut();
            let size = CFDataGetLength(data);
            if size > 0 { std::slice::from_raw_parts_mut(CFDataGetMutableBytePtr(data), size as usize).zeroize(); }
        }
    }
}

fn text(value: &str) -> Result<Object> {
    Object::checked(unsafe { CFStringCreateWithBytes(std::ptr::null(), value.as_ptr(), value.len() as isize, 0x0800_0100, 0) })
}

fn query(id: &str, adding: bool, extra: &[(Ref, Ref)]) -> Result<Object> {
    let service = text("app.gipny.i2p-device-v2")?;
    let account = text(id)?;
    let mut keychain = std::ptr::null();
    if unsafe { SecKeychainCopyDefault(&mut keychain) } != 0 {
        return Err(BindingError::Unavailable("macOS default local Keychain is unavailable; unlock or provision the OS Keychain"));
    }
    let keychain = Object::checked(keychain)?;
    let search = Object::checked(unsafe {
        CFArrayCreate(std::ptr::null(), &keychain.0, 1, std::ptr::addr_of!(kCFTypeArrayCallBacks).cast())
    })?;
    let mut entries = unsafe { vec![
        (kSecClass, kSecClassGenericPassword),
        (kSecAttrService, service.0), (kSecAttrAccount, account.0),
        // One explicit software tier, never an entitlement-dependent DP
        // provider with a hidden fallback. File Keychain does not iCloud-sync;
        // no DP-only synchronizable/accessibility attributes are supplied.
        (kSecUseDataProtectionKeychain, kCFBooleanFalse),
        if adding { (kSecUseKeychain, keychain.0) }
        else { (kSecMatchSearchList, search.0) },
    ] };
    entries.extend_from_slice(extra);
    let keys: Vec<_> = entries.iter().map(|(key, _)| *key).collect();
    let values: Vec<_> = entries.iter().map(|(_, value)| *value).collect();
    Object::checked(unsafe { CFDictionaryCreate(
        std::ptr::null(), keys.as_ptr(), values.as_ptr(), entries.len() as isize,
        std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
        std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
    ) })
}

pub(super) struct Provider;
impl DeviceKeyProvider for Provider {
    fn kind(&self) -> &'static str { "macos-local-keychain-v2" }
    fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
        let data = SecretData(Object::checked(unsafe { CFDataCreateMutable(std::ptr::null(), 32) })?);
        unsafe { CFDataAppendBytes(data.0.0.cast_mut(), secret.as_ptr(), 32); }
        let query = query(id, true, unsafe { &[
            (kSecValueData, data.0.0),
        ] })?;
        if unsafe { SecItemAdd(query.0, std::ptr::null_mut()) } != 0 {
            return Err(BindingError::Unavailable("macOS local Keychain could not store the credential; unlock it and allow OS access when prompted"));
        }
        Ok(Vec::new())
    }
    fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        if !wrapped.is_empty() { return Err(BindingError::InvalidMetadata); }
        let query = query(id, false, unsafe { &[(kSecReturnData, kCFBooleanTrue)] })?;
        let mut result = std::ptr::null();
        if unsafe { SecItemCopyMatching(query.0, &mut result) } != 0 { return Err(BindingError::MissingCredential); }
        let data = Object::checked(result)?;
        if unsafe { CFGetTypeID(data.0) != CFDataGetTypeID() || CFDataGetLength(data.0) != 32 } {
            return Err(BindingError::MissingCredential);
        }
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(data.0), 32) });
        Ok(secret)
    }
    fn delete(&self, id: &str) -> Result<()> {
        let query = query(id, false, &[])?;
        match unsafe { SecItemDelete(query.0) } {
            0 | -25300 => Ok(()), // success or already absent
            _ => Err(BindingError::Unavailable("macOS local Keychain could not delete the credential; allow OS access when prompted")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use crate::security::{ArgonParams, DuressMode, SecurityError, UnlockOutcome, Vault};

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecKeychainOpen(path: *const std::ffi::c_char, keychain: *mut Ref) -> i32;
        fn SecKeychainSetDefault(keychain: Ref) -> i32;
        fn SecKeychainSetUserInteractionAllowed(allowed: u8) -> i32;
    }

    struct RestoreDefault(Object);
    impl Drop for RestoreDefault {
        fn drop(&mut self) { let _ = unsafe { SecKeychainSetDefault(self.0.0) }; }
    }

    #[test]
    #[ignore = "requires GHA isolated local Keychains and an explicitly ad-hoc signed test binary"]
    fn native_macos_local_keychain_enrollment_copy_isolation_and_delete() {
        assert_eq!(std::env::var("GIPNY_CI_MAC_KEYCHAINS_ACTIVE").as_deref(), Ok("1"));
        // This dedicated test process must fail rather than hang on a hidden
        // GUI prompt. Production leaves normal OS user consent enabled.
        assert_eq!(unsafe { SecKeychainSetUserInteractionAllowed(0) }, 0);
        let other = CString::new(std::env::var("GIPNY_CI_MAC_OTHER_KEYCHAIN").unwrap()).unwrap();
        let original = tempfile::tempdir().unwrap();
        let copied = tempfile::tempdir().unwrap();
        let vault = Vault::create_with_params(original.path(), "fixture pass", None, DuressMode::Wipe, 1,
            ArgonParams { m: 8192, t: 1, p: 1 }).unwrap();
        let metadata = crate::device_binding::metadata(original.path()).unwrap();
        assert_eq!(metadata.provider, "macos-local-keychain-v2");
        let before = std::fs::read(Vault::path_of(original.path())).unwrap();
        let binding = std::fs::read(original.path().join(FILE_NAME)).unwrap();
        for file in std::fs::read_dir(original.path()).unwrap() {
            let file = file.unwrap();
            if file.path().is_file() { std::fs::copy(file.path(), copied.path().join(file.file_name())).unwrap(); }
        }
        let mut first = std::ptr::null();
        assert_eq!(unsafe { SecKeychainCopyDefault(&mut first) }, 0);
        let restore = RestoreDefault(Object::checked(first).unwrap());
        let mut second = std::ptr::null();
        assert_eq!(unsafe { SecKeychainOpen(other.as_ptr(), &mut second) }, 0);
        let second = Object::checked(second).unwrap();
        assert_eq!(unsafe { SecKeychainSetDefault(second.0) }, 0);
        // Both Keychains are unlocked. Access must nevertheless stay scoped
        // to the currently selected OS store rather than a broad search list.
        assert!(matches!(Vault::open(copied.path()), Err(SecurityError::DeviceBinding(BindingError::MissingCredential))));
        assert_eq!(std::fs::read(Vault::path_of(copied.path())).unwrap(), before);
        assert_eq!(std::fs::read(copied.path().join(FILE_NAME)).unwrap(), binding);
        drop(restore);
        assert!(matches!(Vault::open(original.path()).unwrap().unlock("fixture pass"), Ok(UnlockOutcome::Primary(_))));
        vault.wipe_all().unwrap();
        assert!(matches!(Provider.unseal(&metadata.id, &[]), Err(BindingError::MissingCredential)));
        assert!(!Vault::exists(original.path()));
    }
}
