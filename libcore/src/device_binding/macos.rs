use super::*;
use std::ffi::c_void;
use zeroize::Zeroize;

type Ref = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
    fn CFDictionaryCreate(allocator: Ref, keys: *const Ref, values: *const Ref, count: isize, key_callbacks: Ref, value_callbacks: Ref) -> Ref;
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
    static kSecAttrSynchronizable: Ref;
    static kSecAttrAccessible: Ref;
    static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: Ref;
    static kSecUseDataProtectionKeychain: Ref;
    static kSecUseAuthenticationUI: Ref;
    static kSecUseAuthenticationUIFail: Ref;
    static kSecValueData: Ref;
    static kSecReturnData: Ref;
    fn SecItemAdd(attributes: Ref, result: *mut Ref) -> i32;
    fn SecItemCopyMatching(query: Ref, result: *mut Ref) -> i32;
    fn SecItemDelete(query: Ref) -> i32;
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

fn query(id: &str, extra: &[(Ref, Ref)]) -> Result<Object> {
    let service = text("app.gipny.i2p-device-v2")?;
    let account = text(id)?;
    let mut entries = unsafe { vec![
        (kSecClass, kSecClassGenericPassword),
        (kSecAttrService, service.0), (kSecAttrAccount, account.0),
        (kSecAttrSynchronizable, kCFBooleanFalse),
        (kSecAttrAccessible, kSecAttrAccessibleWhenUnlockedThisDeviceOnly),
        // Do not silently use the legacy, potentially portable file Keychain.
        (kSecUseDataProtectionKeychain, kCFBooleanTrue),
        (kSecUseAuthenticationUI, kSecUseAuthenticationUIFail),
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
    fn kind(&self) -> &'static str { "macos-dataprotection-keychain-v2" }
    fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
        let data = SecretData(Object::checked(unsafe { CFDataCreateMutable(std::ptr::null(), 32) })?);
        unsafe { CFDataAppendBytes(data.0.0.cast_mut(), secret.as_ptr(), 32); }
        let query = query(id, unsafe { &[
            (kSecValueData, data.0.0),
        ] })?;
        if unsafe { SecItemAdd(query.0, std::ptr::null_mut()) } != 0 {
            return Err(BindingError::Unavailable("macOS Data Protection Keychain is locked, unsupported, or the application's signing entitlement is missing"));
        }
        Ok(Vec::new())
    }
    fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        if !wrapped.is_empty() { return Err(BindingError::InvalidMetadata); }
        let query = query(id, unsafe { &[(kSecReturnData, kCFBooleanTrue)] })?;
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
        let query = query(id, &[])?;
        match unsafe { SecItemDelete(query.0) } {
            0 | -25300 => Ok(()), // success or already absent
            _ => Err(BindingError::Unavailable("macOS Data Protection Keychain could not delete the credential")),
        }
    }
}
