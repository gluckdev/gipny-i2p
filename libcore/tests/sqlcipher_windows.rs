//! Isolated reproduction of sqlcipher/sqlcipher#619. Never enlarge the stack
//! or disable cipher_memory_security to make this regression pass.
#![cfg(windows)]

use std::ffi::c_void;
use std::process::Command;
use rusqlite::Connection;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn SetProcessWorkingSetSize(process: *mut c_void, minimum: usize, maximum: usize) -> i32;
    fn GetProcessWorkingSetSize(process: *mut c_void, minimum: *mut usize, maximum: *mut usize) -> i32;
    fn VirtualAlloc(address: *const c_void, size: usize, allocation: u32, protection: u32) -> *mut c_void;
    fn VirtualLock(address: *const c_void, size: usize) -> i32;
    fn VirtualUnlock(address: *const c_void, size: usize) -> i32;
    fn VirtualFree(address: *mut c_void, size: usize, operation: u32) -> i32;
}

const PAGE: usize = 4096;
const CHILD: &str = "GIPNY_TEST_SQLCIPHER_LOW_LOCK_CHILD";

// Only the child process changes its own quota. Retain successfully locked
// pages until after SQLCipher closes, so its logger cannot allocate on a
// newly locked page by chance and conceal the old recursion.
struct LockedQuota(Vec<*mut c_void>);
impl LockedQuota {
    fn exhaust() -> Self {
        // SAFETY: a current-process pseudo-handle needs no closing. These
        // limits affect this disposable test child, never the parent/suite.
        assert_ne!(unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), 128 * 1024, 4 * 1024 * 1024) }, 0,
            "cannot provision the Windows low-lock-quota regression");
        let mut quota = Self(Vec::new());
        for _ in 0..256 {
            // SAFETY: allocate one independent committed read/write page.
            let page = unsafe { VirtualAlloc(std::ptr::null(), PAGE, 0x1000 | 0x2000, 0x04) };
            assert!(!page.is_null(), "VirtualAlloc: {}", std::io::Error::last_os_error());
            if unsafe { VirtualLock(page, PAGE) } == 0 {
                let error = std::io::Error::last_os_error();
                unsafe { VirtualFree(page, 0, 0x8000); }
                assert_eq!(error.raw_os_error(), Some(1453), "expected working-set quota exhaustion");
                return quota;
            }
            quota.0.push(page);
        }
        panic!("Windows did not enforce the requested low lock quota");
    }
}
impl Drop for LockedQuota {
    fn drop(&mut self) {
        for page in self.0.drain(..) {
            // SAFETY: these are precisely the live allocations owned above.
            unsafe { VirtualUnlock(page, PAGE); VirtualFree(page, 0, 0x8000); }
        }
    }
}

fn working_set_limits() -> (usize, usize) {
    let (mut minimum, mut maximum) = (0, 0);
    assert_ne!(unsafe { GetProcessWorkingSetSize(GetCurrentProcess(), &mut minimum, &mut maximum) }, 0);
    (minimum, maximum)
}

#[test]
fn protected_keys_recover_from_lock_quota_but_stop_at_the_process_budget() {
    const KEY_CHILD: &str = "GIPNY_TEST_WINDOWS_KEY_LOCK_CHILD";
    if std::env::var_os(KEY_CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "protected_keys_recover_from_lock_quota_but_stop_at_the_process_budget", "--nocapture", "--test-threads=1"])
            .env(KEY_CHILD, "1").output().unwrap();
        assert!(output.status.success(), "isolated protected-key quota child failed: {}\n{}\n{}",
            output.status, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }

    use gipny_libcore::memory::{self, MemoryError, SealedBlob};
    let mut total_increase = 0;
    let mut recovered = false;
    for _ in 0..=16 {
        let quota = LockedQuota::exhaust();
        let before = working_set_limits();
        match SealedBlob::from_slice(b"synthetic protected-key regression") {
            Ok(secret) => {
                recovered = true;
                let after = working_set_limits();
                assert!(after.0 > before.0 && after.1 >= before.1);
                total_increase += after.0 - before.0;
                assert!(total_increase <= 16 * 1024 * 1024, "quota increased beyond the process budget");
                assert_eq!(&*secret.expose().unwrap(), b"synthetic protected-key regression");
                let capabilities = memory::capabilities().unwrap();
                assert!(capabilities.locked && capabilities.idle_no_access);
                drop(secret);
            }
            Err(MemoryError::System { operation: "VirtualLock", source }) => {
                assert!(recovered, "the bounded quota policy never recovered a protected page");
                assert_eq!(source.raw_os_error(), Some(1453));
                assert_eq!(working_set_limits(), before, "failed admission still changed OS policy");
                return; // No unlocked key was returned after the budget ran out.
            }
            Err(error) => panic!("unexpected protected-key admission failure: {error}"),
        }
        drop(quota);
    }
    panic!("repeated lock pressure bypassed the process-wide quota budget");
}

#[test]
fn memory_security_survives_windows_lock_failure() {
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "memory_security_survives_windows_lock_failure", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "isolated SQLCipher lock-quota child failed: {}\n{}\n{}",
            output.status, String::from_utf8_lossy(&output.stdout), stderr);
        // SQLCipher 4.19 reports this at INFO with the allocation's pointer and
        // size. Require the actual API failure and quota code on one line.
        assert!(stderr.lines().any(|line| line.contains("MEMORY sqlcipher_mlock: VirtualLock(")
                && line.ends_with(") returned 0 LastError=1453")),
            "regression never reached SQLCipher's failing-lock log path: {stderr}");
        println!("Observed SQLCipher VirtualLock ERROR_WORKING_SET_QUOTA; protected SQL allocations and free completed");
        return;
    }

    let db = Connection::open_in_memory().unwrap();
    let version: String = db.query_row("PRAGMA cipher_version", [], |row| row.get(0)).unwrap();
    let numbers: Vec<u32> = version.split('.').take(2).map(|part| part.parse().unwrap()).collect();
    assert!(numbers.as_slice() >= [4, 18].as_slice(), "SQLCipher {version} predates the Windows logger fix");
    db.execute_batch("PRAGMA cipher_log = stderr; PRAGMA cipher_log_level = INFO; PRAGMA cipher_log_source = MEMORY;").unwrap();
    let quota = LockedQuota::exhaust();
    db.execute_batch("PRAGMA cipher_memory_security = ON;").unwrap();
    let enabled: String = db.query_row("PRAGMA cipher_memory_security", [], |row| row.get(0)).unwrap();
    assert_eq!(enabled, "1", "the regression must retain memory protection");
    db.execute_batch("CREATE TEMP TABLE pressure(value BLOB);
        WITH RECURSIVE count(n) AS (VALUES(1) UNION ALL SELECT n + 1 FROM count WHERE n < 100)
        INSERT INTO pressure SELECT randomblob(2048) FROM count;").unwrap();
    let bytes: i64 = db.query_row("SELECT sum(length(value)) FROM pressure", [], |row| row.get(0)).unwrap();
    assert_eq!(bytes, 100 * 2048);
    drop(db); // Also exercise failed VirtualUnlock logging while quota is full.
    drop(quota);
}
