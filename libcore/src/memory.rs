//! Small, locked OS pages for scoped access to sealed key material.
//!
//! This is not a global allocator. Ciphertext lives in ordinary memory. One
//! reference-counted page contains a random wrapping key and a fixed scratch
//! region; it is NOACCESS between operations and destroyed with its last owner.
use std::marker::PhantomData;
use std::cell::Cell;
use std::ops::Deref;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use chacha20poly1305::{AeadInOut, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use rand::rand_core::TryRng;
use thiserror::Error;
use zeroize::Zeroize;

const KEY_LEN: usize = 32;
const SCRATCH_OFFSET: usize = 64;
const MAX_SECRET: usize = 2048;

thread_local! { static PAGE_OPEN: Cell<bool> = const { Cell::new(false) }; }

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("protected memory {operation}: {source}")]
    System { operation: &'static str, #[source] source: std::io::Error },
    #[error("protected memory is unsupported on this platform")]
    Unsupported,
    #[error("protected memory size is unsupported")]
    Size,
    #[error("protected memory authentication failed")]
    Authentication,
    #[error("protected memory lock is poisoned")]
    Poisoned,
    #[error("nested protected memory access is forbidden")]
    NestedAccess,
    #[error("system randomness is unavailable")]
    Random,
}

pub type Result<T> = std::result::Result<T, MemoryError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub page_size: usize,
    pub locked: bool,
    pub idle_no_access: bool,
    pub excluded_from_core: bool,
}

/// Preflight the strict policy. No unlocked/best-effort fallback exists.
pub fn capabilities() -> Result<Capabilities> { Protector::shared()?.capabilities() }

fn system_error(operation: &'static str) -> MemoryError {
    MemoryError::System { operation, source: std::io::Error::last_os_error() }
}

#[cfg(windows)]
fn lock_windows_page(ptr: NonNull<u8>, len: usize) -> Result<()> {
    use windows_sys::Win32::System::{Memory::*, Threading::GetCurrentProcess};
    const QUOTA_STEP: usize = 1024 * 1024;
    const MAX_QUOTA_INCREASE: usize = 16 * QUOTA_STEP;
    // Shared process quota, not a fresh allowance for each key/profile. Windows
    // limits VirtualLock to the minimum working set minus OS overhead. SQLCipher
    // also uses that quota; only a real successful VirtualLock admits our page.
    // https://learn.microsoft.com/windows/win32/api/memoryapi/nf-memoryapi-virtuallock
    static INCREASED: Mutex<usize> = Mutex::new(0);
    if unsafe { VirtualLock(ptr.as_ptr().cast(), len) } != 0 { return Ok(()); }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() != Some(1453) {
        return Err(MemoryError::System { operation: "VirtualLock", source: error });
    }
    let mut increased = INCREASED.lock().map_err(|_| MemoryError::Poisoned)?;
    loop {
        // Another protected allocation may already have provisioned quota or
        // released its page while this caller waited for the process mutex.
        if unsafe { VirtualLock(ptr.as_ptr().cast(), len) } != 0 { return Ok(()); }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(1453) || *increased >= MAX_QUOTA_INCREASE {
            return Err(MemoryError::System { operation: "VirtualLock", source: error });
        }
        let (mut minimum, mut maximum, mut flags) = (0, 0, 0);
        let process = unsafe { GetCurrentProcess() };
        if unsafe { GetProcessWorkingSetSizeEx(process, &mut minimum, &mut maximum, &mut flags) } == 0 {
            return Err(system_error("GetProcessWorkingSetSizeEx"));
        }
        let step = QUOTA_STEP.min(MAX_QUOTA_INCREASE - *increased);
        let minimum = minimum.checked_add(step).ok_or(MemoryError::Size)?;
        // Retain existing larger limits and hard/soft working-set flags. This
        // asks the OS for quota; it does not replace VirtualLock or pin 16 MiB.
        let maximum = maximum.max(minimum);
        if unsafe { SetProcessWorkingSetSizeEx(process, minimum, maximum, flags) } == 0 {
            return Err(system_error("SetProcessWorkingSetSizeEx"));
        }
        *increased = increased.checked_add(step).ok_or(MemoryError::Size)?;
    }
}

struct LockedPage { ptr: NonNull<u8>, len: usize }

// The allocation is exclusively owned; access is always through the Mutex in
// Protector. PageAccess/SecretGuard deliberately remain !Send and !Sync.
unsafe impl Send for LockedPage {}

impl LockedPage {
    fn allocate() -> Result<Self> {
        #[cfg(unix)]
        {
            let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if size <= 0 { return Err(system_error("page size")); }
            let len = usize::try_from(size).map_err(|_| MemoryError::Size)?;
            if len < SCRATCH_OFFSET + MAX_SECRET { return Err(MemoryError::Size); }
            let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0) };
            if ptr == libc::MAP_FAILED { return Err(system_error("mmap")); }
            let Some(ptr) = NonNull::new(ptr.cast()) else {
                unsafe { libc::munmap(ptr, len); }
                return Err(MemoryError::Size);
            };
            let page = Self { ptr, len };
            if unsafe { libc::mlock(page.ptr.as_ptr().cast(), len) } != 0 {
                return Err(system_error("mlock"));
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            if unsafe { libc::madvise(page.ptr.as_ptr().cast(), len, libc::MADV_DONTDUMP) } != 0 {
                return Err(system_error("MADV_DONTDUMP"));
            }
            Ok(page)
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::{Memory::*, SystemInformation::*};
            let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
            unsafe { GetSystemInfo(&mut info) };
            let len = info.dwPageSize as usize;
            if len < SCRATCH_OFFSET + MAX_SECRET { return Err(MemoryError::Size); }
            let ptr = unsafe { VirtualAlloc(std::ptr::null(), len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
            let page = Self { ptr: NonNull::new(ptr.cast()).ok_or_else(|| system_error("VirtualAlloc"))?, len };
            lock_windows_page(page.ptr, len)?;
            Ok(page)
        }
        #[cfg(not(any(unix, windows)))]
        { Err(MemoryError::Unsupported) }
    }

    fn protect(&mut self, accessible: bool) -> Result<()> {
        #[cfg(unix)]
        {
            let protection = if accessible { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_NONE };
            if unsafe { libc::mprotect(self.ptr.as_ptr().cast(), self.len, protection) } != 0 {
                return Err(system_error("mprotect"));
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Memory::*;
            let protection = if accessible { PAGE_READWRITE } else { PAGE_NOACCESS };
            let mut old = 0;
            if unsafe { VirtualProtect(self.ptr.as_ptr().cast(), self.len, protection, &mut old) } == 0 {
                return Err(system_error("VirtualProtect"));
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        { let _ = accessible; Err(MemoryError::Unsupported) }
    }

    // Only used after a successful RW transition, under exclusive ownership.
    fn bytes(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for LockedPage {
    fn drop(&mut self) {
        // Continuing after a failed protection transition could expose a key.
        // Abort instead of silently retaining an unlocked/plaintext fallback.
        if self.protect(true).is_err() { std::process::abort(); }
        self.bytes().zeroize();
        #[cfg(unix)]
        unsafe {
            libc::munlock(self.ptr.as_ptr().cast(), self.len);
            libc::munmap(self.ptr.as_ptr().cast(), self.len);
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Memory::*;
            VirtualUnlock(self.ptr.as_ptr().cast(), self.len);
            VirtualFree(self.ptr.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

struct Protector { page: Mutex<LockedPage> }

impl Protector {
    fn shared() -> Result<Arc<Self>> {
        // Weak is intentional: closing the last owner destroys the wrapping
        // key. A second active profile must keep its own live keys usable.
        static CURRENT: OnceLock<Mutex<Weak<Protector>>> = OnceLock::new();
        let mut weak = CURRENT.get_or_init(|| Mutex::new(Weak::new())).lock().map_err(|_| MemoryError::Poisoned)?;
        if let Some(existing) = weak.upgrade() { return Ok(existing); }
        let mut page = LockedPage::allocate()?;
        rand::rngs::SysRng.try_fill_bytes(&mut page.bytes()[..KEY_LEN]).map_err(|_| MemoryError::Random)?;
        page.protect(false)?;
        let protector = Arc::new(Self { page: Mutex::new(page) });
        *weak = Arc::downgrade(&protector);
        Ok(protector)
    }

    fn capabilities(&self) -> Result<Capabilities> {
        if PAGE_OPEN.with(Cell::get) { return Err(MemoryError::NestedAccess); }
        let page = self.page.lock().map_err(|_| MemoryError::Poisoned)?;
        Ok(Capabilities { page_size: page.len, locked: true, idle_no_access: true,
            excluded_from_core: cfg!(any(target_os = "linux", target_os = "android")) })
    }

    fn access(&self) -> Result<PageAccess<'_>> {
        // A second operation in the same synchronous closure would otherwise
        // deadlock this non-reentrant mutex. Other threads still wait normally.
        if PAGE_OPEN.with(Cell::get) { return Err(MemoryError::NestedAccess); }
        let mut page = self.page.lock().map_err(|_| MemoryError::Poisoned)?;
        page.protect(true)?;
        PAGE_OPEN.with(|open| open.set(true));
        Ok(PageAccess { page, not_send: PhantomData })
    }
}

struct PageAccess<'a> {
    page: MutexGuard<'a, LockedPage>,
    not_send: PhantomData<Rc<()>>,
}

impl Drop for PageAccess<'_> {
    fn drop(&mut self) {
        self.page.bytes()[SCRATCH_OFFSET..SCRATCH_OFFSET + MAX_SECRET].zeroize();
        if self.page.protect(false).is_err() { std::process::abort(); }
        PAGE_OPEN.with(|open| open.set(false));
    }
}

/// A fixed-size, synchronous view. It cannot cross an await on a Send task.
///
/// ```compile_fail
/// fn require_send<T: Send>() {}
/// require_send::<gipny_libcore::memory::SecretGuard<'_, 32>>();
/// ```
pub struct SecretGuard<'a, const N: usize> { access: PageAccess<'a> }

impl<const N: usize> Deref for SecretGuard<'_, N> {
    type Target = [u8; N];
    fn deref(&self) -> &Self::Target {
        // No mutable alias exists while this guard owns the page mutex.
        unsafe { &*(self.access.page.ptr.as_ptr().add(SCRATCH_OFFSET).cast::<[u8; N]>()) }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SecretKind { Master = 1, Identity = 2, Ratchet = 3, PreKey = 4, I2p = 5 }

/// No Debug/Serialize implementation: exporting plaintext must be explicit at
/// the storage boundary, with a guard alive for the entire operation.
pub(crate) struct Sealed<const N: usize> {
    protector: Arc<Protector>,
    ciphertext: [u8; N],
    nonce: [u8; 24],
    tag: [u8; 16],
    aad: [u8; 64],
}

impl<const N: usize> Clone for Sealed<N> {
    fn clone(&self) -> Self {
        Self { protector: self.protector.clone(), ciphertext: self.ciphertext,
            nonce: self.nonce, tag: self.tag, aad: self.aad }
    }
}

impl<const N: usize> Sealed<N> {
    pub(crate) fn from_slice(kind: SecretKind, plaintext: &[u8]) -> Result<Self> {
        if plaintext.len() != N { return Err(MemoryError::Size); }
        Self::create(kind, |scratch| { scratch.copy_from_slice(plaintext); Ok(()) })
    }

    pub(crate) fn random(kind: SecretKind) -> Result<Self> {
        Self::create(kind, |scratch| rand::rngs::SysRng.try_fill_bytes(scratch).map_err(|_| MemoryError::Random))
    }

    fn create(kind: SecretKind, fill: impl FnOnce(&mut [u8]) -> Result<()>) -> Result<Self> {
        if N == 0 || N > MAX_SECRET { return Err(MemoryError::Size); }
        let protector = Protector::shared()?;
        let mut nonce = [0; 24];
        rand::rngs::SysRng.try_fill_bytes(&mut nonce).map_err(|_| MemoryError::Random)?;
        let mut aad = [0; 64];
        aad[..25].copy_from_slice(b"gipny/protected-memory/v1");
        aad[32] = kind as u8;
        aad[40..48].copy_from_slice(&(N as u64).to_le_bytes());
        rand::rngs::SysRng.try_fill_bytes(&mut aad[48..]).map_err(|_| MemoryError::Random)?;
        let mut ciphertext = [0; N];
        let tag;
        {
            let mut access = protector.access()?;
            let (key_region, scratch_region) = access.page.bytes().split_at_mut(SCRATCH_OFFSET);
            let key: &[u8; 32] = (&key_region[..KEY_LEN]).try_into().map_err(|_| MemoryError::Size)?;
            let scratch = &mut scratch_region[..N];
            fill(scratch)?;
            let cipher = XChaCha20Poly1305::new(key.into());
            tag = cipher.encrypt_inout_detached(&XNonce::from(nonce), &aad, (&mut *scratch).into())
                .map_err(|_| MemoryError::Authentication)?;
            ciphertext.copy_from_slice(scratch);
        }
        Ok(Self { protector, ciphertext, nonce, tag: tag.into(), aad })
    }

    pub(crate) fn open(&self) -> Result<SecretGuard<'_, N>> {
        let mut access = self.protector.access()?;
        {
            let (key_region, scratch_region) = access.page.bytes().split_at_mut(SCRATCH_OFFSET);
            let key: &[u8; 32] = (&key_region[..KEY_LEN]).try_into().map_err(|_| MemoryError::Size)?;
            let scratch = &mut scratch_region[..N];
            scratch.copy_from_slice(&self.ciphertext);
            XChaCha20Poly1305::new(key.into()).decrypt_inout_detached(
                &XNonce::from(self.nonce), &self.aad, scratch.into(), &Tag::from(self.tag),
            ).map_err(|_| MemoryError::Authentication)?;
        }
        Ok(SecretGuard { access })
    }
}

/// Bounded I2P private-key storage. The clone contains ciphertext only. It is
/// deliberately not Debug/Serialize; export must stay within a synchronous
/// guard scope, before any network await.
#[derive(Clone)]
pub struct SealedBlob { sealed: Sealed<MAX_SECRET>, len: usize }

impl SealedBlob {
    pub fn from_slice(plaintext: &[u8]) -> Result<Self> {
        if plaintext.is_empty() || plaintext.len() > MAX_SECRET { return Err(MemoryError::Size); }
        let sealed = Sealed::create(SecretKind::I2p, |scratch| {
            scratch.fill(0);
            scratch[..plaintext.len()].copy_from_slice(plaintext);
            Ok(())
        })?;
        Ok(Self { sealed, len: plaintext.len() })
    }

    pub fn expose(&self) -> Result<BlobGuard<'_>> {
        Ok(BlobGuard { guard: self.sealed.open()?, len: self.len })
    }
}

pub struct BlobGuard<'a> { guard: SecretGuard<'a, MAX_SECRET>, len: usize }

impl Deref for BlobGuard<'_> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target { &self.guard[..self.len] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealing_uses_fresh_nonces_and_authenticates_before_exposure() {
        let key = [0x5a; 32];
        let first = Sealed::<32>::from_slice(SecretKind::Master, &key).unwrap();
        let second = Sealed::<32>::from_slice(SecretKind::Master, &key).unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.aad, second.aad);
        assert_ne!(first.ciphertext, key);
        assert_eq!(&*first.open().unwrap(), &key);
        let mut tampered = first.clone();
        tampered.tag[0] ^= 1;
        assert!(matches!(tampered.open(), Err(MemoryError::Authentication)));
        let mut access = first.protector.access().unwrap();
        assert!(access.page.bytes()[SCRATCH_OFFSET..SCRATCH_OFFSET + MAX_SECRET].iter().all(|b| *b == 0));
    }

    #[test]
    fn last_owner_destroys_protector_and_clones_only_copy_ciphertext() {
        const CHILD: &str = "GIPNY_MEMORY_LAST_OWNER_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact").arg("memory::tests::last_owner_destroys_protector_and_clones_only_copy_ciphertext")
                .env(CHILD, "1").status().unwrap();
            assert!(status.success());
            return;
        }
        let secret = Sealed::<32>::random(SecretKind::Master).unwrap();
        let weak = Arc::downgrade(&secret.protector);
        let cloned = secret.clone();
        assert!(Arc::ptr_eq(&secret.protector, &cloned.protector));
        drop(secret);
        assert!(weak.upgrade().is_some());
        drop(cloned);
        assert!(weak.upgrade().is_none());
        let next = Sealed::<32>::random(SecretKind::Master).unwrap();
        assert!(!Weak::ptr_eq(&weak, &Arc::downgrade(&next.protector)));
    }

    #[cfg(unix)]
    #[test]
    fn idle_page_rejects_reads_in_a_child_process() {
        use std::os::unix::process::ExitStatusExt;
        const CHILD: &str = "GIPNY_MEMORY_NOACCESS_CHILD";
        if std::env::var_os(CHILD).is_some() {
            i2p_embed::process_security::harden_process().unwrap();
            let secret = Sealed::<32>::random(SecretKind::Master).unwrap();
            let ptr = { secret.protector.page.lock().unwrap().ptr.as_ptr() };
            unsafe { std::ptr::read_volatile(ptr); }
            panic!("NOACCESS page remained readable");
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact").arg("memory::tests::idle_page_rejects_reads_in_a_child_process")
            .env(CHILD, "1").status().unwrap();
        assert!(matches!(status.signal(), Some(signal) if signal == libc::SIGSEGV || signal == libc::SIGBUS));
    }

    #[cfg(windows)]
    #[test]
    fn idle_page_rejects_reads_on_windows_in_a_child_process() {
        const CHILD: &str = "GIPNY_MEMORY_NOACCESS_WINDOWS_CHILD";
        if std::env::var_os(CHILD).is_some() {
            i2p_embed::process_security::harden_process().unwrap();
            let secret = Sealed::<32>::random(SecretKind::Master).unwrap();
            let ptr = { secret.protector.page.lock().unwrap().ptr.as_ptr() };
            unsafe { std::ptr::read_volatile(ptr); }
            panic!("PAGE_NOACCESS page remained readable");
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact").arg("memory::tests::idle_page_rejects_reads_on_windows_in_a_child_process")
            .env(CHILD, "1").status().unwrap();
        // Windows access violation, distinct from an ordinary test panic.
        assert_eq!(status.code().map(|code| code as u32), Some(0xc0000005));
    }

    #[cfg(unix)]
    #[test]
    fn lock_failure_has_no_plaintext_fallback() {
        const CHILD: &str = "GIPNY_MEMORY_LOCK_FAILURE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            // A privileged runner may bypass RLIMIT_MEMLOCK. The isolated
            // child drops root credentials before testing the OS denial.
            if unsafe { libc::geteuid() } == 0 {
                assert_eq!(unsafe { libc::setgid(65534) }, 0);
                assert_eq!(unsafe { libc::setuid(65534) }, 0);
            }
            let limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &limit) }, 0);
            assert!(matches!(Sealed::<32>::random(SecretKind::Master),
                Err(MemoryError::System { operation: "mlock", .. })));
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact").arg("memory::tests::lock_failure_has_no_plaintext_fallback")
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
    }

    #[test]
    fn bounded_blob_exposes_exact_length_and_clones_ciphertext() {
        assert!(matches!(SealedBlob::from_slice(&[]), Err(MemoryError::Size)));
        assert!(matches!(SealedBlob::from_slice(&[0; MAX_SECRET + 1]), Err(MemoryError::Size)));
        let source = [0x3d; MAX_SECRET];
        let blob = SealedBlob::from_slice(&source).unwrap();
        let clone = blob.clone();
        drop(blob);
        assert_eq!(&*clone.expose().unwrap(), &source);
        let short = SealedBlob::from_slice(b"private-I2P-key").unwrap();
        assert_eq!(&*short.expose().unwrap(), b"private-I2P-key");
    }

    #[test]
    fn nested_access_fails_instead_of_deadlocking_and_recovers_after_drop() {
        let first = Sealed::<32>::random(SecretKind::Master).unwrap();
        let second = Sealed::<32>::random(SecretKind::Ratchet).unwrap();
        let guard = first.open().unwrap();
        assert!(matches!(second.open(), Err(MemoryError::NestedAccess)));
        drop(guard);
        assert!(second.open().is_ok());
    }

    #[test]
    fn panic_wipes_scratch_and_poison_does_not_skip_last_owner_destruction() {
        const CHILD: &str = "GIPNY_MEMORY_POISON_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact").arg("memory::tests::panic_wipes_scratch_and_poison_does_not_skip_last_owner_destruction")
                .env(CHILD, "1").status().unwrap();
            assert!(status.success());
            return;
        }
        let secret = Sealed::<32>::random(SecretKind::Master).unwrap();
        let weak = Arc::downgrade(&secret.protector);
        assert!(std::panic::catch_unwind(|| {
            let _guard = secret.open().unwrap();
            panic!("exercise guard unwinding");
        }).is_err());
        assert!(matches!(secret.open(), Err(MemoryError::Poisoned)));
        {
            let mut page = match secret.protector.page.lock() {
                Err(poison) => poison.into_inner(),
                Ok(_) => panic!("mutex should be poisoned after guard unwinding"),
            };
            page.protect(true).unwrap();
            assert!(page.bytes()[SCRATCH_OFFSET..SCRATCH_OFFSET + MAX_SECRET].iter().all(|byte| *byte == 0));
            page.protect(false).unwrap();
        }
        drop(secret);
        assert!(weak.upgrade().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_reports_locked_nondumpable_inaccessible_page() {
        let secret = Sealed::<32>::random(SecretKind::Master).unwrap();
        // Hold the mutex without opening the page, so concurrent tests cannot
        // change this mapping's permissions while /proc is inspected.
        let page = secret.protector.page.lock().unwrap();
        let address = page.ptr.as_ptr() as usize;
        let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
        let mut found = false;
        let mut flags = None;
        for line in smaps.lines() {
            if let Some((range, rest)) = line.split_once(' ') {
                if let Some((start, end)) = range.split_once('-') {
                    if let (Ok(start), Ok(end)) = (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16)) {
                        found = start <= address && address < end;
                        if found { assert!(rest.starts_with("---p ")); }
                        continue;
                    }
                }
            }
            if found && line.starts_with("VmFlags:") { flags = Some(line.to_owned()); break; }
        }
        let flags = flags.expect("dedicated mapping is present in smaps");
        assert!(flags.split_whitespace().any(|flag| flag == "lo"), "page is not locked");
        assert!(flags.split_whitespace().any(|flag| flag == "dd"), "page is dumpable");
    }
}
