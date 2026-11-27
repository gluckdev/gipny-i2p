use super::*;
use crate::test_support::Scratch;
use std::fs;

fn unexpected_start(_: c_int, _: *const *const c_char, _: *const c_char) -> bool {
    panic!("startup should have stopped before native initialization")
}

#[test]
fn invalid_options_and_log_paths_can_be_corrected_before_native_init() {
    let mut state = StartupState::Ready;
    let invalid = ["--bandwidth=L\0".into()];
    assert!(matches!(
        state.start(&invalid, None, unexpected_start),
        Err(Error::InvalidRouterArgument)
    ));
    assert!(matches!(
        state.start(&[], Some("bad\0path"), unexpected_start),
        Err(Error::InvalidRouterArgument)
    ));
    state.start(&[], None, |_, _, _| true).unwrap();
    assert!(matches!(
        state.start(&[], None, unexpected_start),
        Err(Error::AlreadyStarted)
    ));
}

#[test]
fn partial_native_init_failure_requires_process_restart() {
    let mut state = StartupState::Ready;
    assert!(matches!(
        state.start(&[], None, |_, _, _| false),
        Err(Error::InitFailed)
    ));
    assert!(matches!(
        state.start(&[], None, unexpected_start),
        Err(Error::InitFailed)
    ));
}

#[test]
fn stale_certificate_removal_must_succeed_before_native_init() {
    let scratch = Scratch::new();
    let options = [format!("--datadir={}", scratch.0.display())];
    let certificates = scratch.0.join("certificates");
    let stale = certificates.join("reseed/retired.crt");
    // remove_file fails for a directory even when tests run as root.
    fs::create_dir_all(&stale).unwrap();
    let mut state = StartupState::Ready;
    assert!(matches!(
        state.start(&options, None, unexpected_start),
        Err(Error::Certificates(_))
    ));
    fs::remove_dir(&stale).unwrap();
    fs::write(&stale, b"retired").unwrap();
    let (name, expected) = certs::CERTIFICATES
        .iter()
        .find(|(n, _)| n.starts_with("reseed/"))
        .unwrap();
    let current = certificates.join(name);
    fs::write(&current, b"out of date").unwrap();
    state
        .start(&options, None, |_, _, _| {
            assert!(!stale.exists());
            assert_eq!(fs::read(current).unwrap(), *expected);
            true
        })
        .unwrap();
    assert_eq!(write_certificates(&certificates).unwrap(), 0);
}

#[test]
fn inaccessible_certificate_directory_fails_reconciliation() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("family"), b"not a directory").unwrap();
    assert!(write_certificates(&scratch.0).is_err());
}

#[cfg(gipny_native_tests)]
#[test]
fn native_transit_settings_match_the_selected_options() {
    // Each case owns fresh i2pd globals. InitI2P sets up RouterInfo locally,
    // and we can inspect the getters before starting transports or reseeding.
    const CHILD: &str = "GIPNY_TEST_TRANSIT_CASE";
    let cases = [
        // class/rate, share %, requested cap, no transit, expected native values
        ("L", 0, 0, true, (b'L', 48, 0, 2, false)),
        ("L", 15, 32, false, (b'L', 48, 7, 32, true)),
        ("O", 50, 256, false, (b'O', 256, 128, 256, true)),
        ("P", 80, 2048, false, (b'P', 2048, 1638, 2048, true)),
        ("777", 25, 8, false, (b'P', 777, 194, 8, true)),
        ("2mb", 50, 10, false, (b'P', 2000, 1000, 10, true)),
    ];
    let Ok(index) = std::env::var(CHILD) else {
        for index in 0..cases.len() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "startup_tests::native_transit_settings_match_the_selected_options",
                    "--nocapture",
                ])
                .env(CHILD, index.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "case {index}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    let (bandwidth, share, limit, disabled, expected) = cases[index.parse::<usize>().unwrap()];
    let scratch = Scratch::new();
    let mut options = vec![
        format!("--datadir={}", scratch.0.display()),
        format!("--bandwidth={bandwidth}"),
        format!("--share={share}"),
        format!("--limits.transittunnels={limit}"),
    ];
    if disabled {
        options.push("--notransit".into());
    }
    extern "C" {
        fn gipny_test_transit_snapshot(
            class: *mut c_char,
            shared: *mut u64,
            cap: *mut u32,
            accepts: *mut c_int,
        ) -> u64;
    }
    StartupState::Ready
        .start(&options, None, |argc, argv, _| unsafe {
            ffi::gipny_router_init(argc, argv) != 0
        })
        .unwrap();
    let (mut class, mut shared, mut cap, mut accepts) = (0, 0, 0, 0);
    let rate =
        unsafe { gipny_test_transit_snapshot(&mut class, &mut shared, &mut cap, &mut accepts) };
    assert_eq!((class as u8, rate, shared, cap, accepts != 0), expected);
}
