//! Native test fixture entrypoint. Keeping the FFI reference in this library
//! links the C++ archive emitted by this package's build script into the bin.
use std::ffi::{c_char, c_int, CString};

extern "C" {
    fn gipny_browser_fixture_start(argc: c_int, argv: *const *const c_char) -> c_int;
}

pub fn start(arguments: &[CString]) -> bool {
    let Ok(count) = c_int::try_from(arguments.len()) else {
        return false;
    };
    let pointers: Vec<_> = arguments.iter().map(|value| value.as_ptr()).collect();
    // SAFETY: argument storage remains alive throughout init/start; the C++
    // adapter copies it and sets the loopback policy before spawning threads.
    unsafe { gipny_browser_fixture_start(count, pointers.as_ptr()) == 1 }
}
