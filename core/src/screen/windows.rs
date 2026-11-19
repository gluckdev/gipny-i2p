use std::{ffi::c_void, ptr};
use windows_sys::Win32::{
    System::{
        Power::{
            PowerRegisterSuspendResumeNotification, PowerSettingRegisterNotification,
            DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, POWERBROADCAST_SETTING,
        },
        SystemServices::GUID_SESSION_DISPLAY_STATUS,
    },
    UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMSUSPEND, PBT_POWERSETTINGCHANGE},
};

pub(super) fn install(_: &tauri::App) {
    // Keep the callback descriptor valid for both process-lifetime subscriptions.
    let subscriber = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(on_power),
        Context: ptr::null_mut(),
    }));
    let recipient = ptr::from_mut(subscriber).cast();
    unsafe {
        let mut handle = ptr::null_mut();
        PowerSettingRegisterNotification(
            &GUID_SESSION_DISPLAY_STATUS,
            DEVICE_NOTIFY_CALLBACK,
            recipient,
            &mut handle,
        );
        PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, recipient, &mut handle);
    }
}

unsafe extern "system" fn on_power(_: *const c_void, kind: u32, setting: *const c_void) -> u32 {
    if kind == PBT_APMSUSPEND {
        super::cover();
    } else if kind == PBT_POWERSETTINGCHANGE && !setting.is_null() {
        let data = unsafe { &*setting.cast::<POWERBROADCAST_SETTING>() };
        if data.DataLength == 4 && unsafe { data.Data.as_ptr().cast::<u32>().read_unaligned() } == 0
        {
            super::cover();
        }
    }
    0
}

use ::windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};

pub(super) async fn available() -> ::windows::core::Result<bool> {
    Ok(UserConsentVerifier::CheckAvailabilityAsync()?.await?
        == UserConsentVerifierAvailability::Available)
}

pub(super) async fn verify(reason: &str) -> ::windows::core::Result<bool> {
    use ::windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use tauri::Manager;
    let app = super::APP.get().ok_or_else(::windows::core::Error::empty)?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(::windows::core::Error::empty)?;
    let hwnd = window.hwnd().map_err(|_| ::windows::core::Error::empty())?;
    let operation: windows_future::IAsyncOperation<UserConsentVerificationResult> = {
        let factory =
            ::windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
        unsafe { factory.RequestVerificationForWindowAsync(hwnd, &reason.into())? }
    };
    Ok(operation.await? == UserConsentVerificationResult::Verified)
}
