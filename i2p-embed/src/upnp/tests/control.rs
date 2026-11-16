use super::*;

#[test]
fn host_notifications_are_debounced() {
    let control = Control::default();
    control.report(true);
    control.report(true);
    assert_eq!(
        control.flags.lock().unwrap().revision,
        0,
        "online→online is no news"
    );
    assert_eq!(control.wait(0, Duration::from_millis(1)), Woke::Timeout);
    control.report(false);
    control.report(true);
    assert_eq!(control.wait(0, Duration::from_millis(1)), Woke::Network);
    assert_eq!(control.settle(Duration::from_millis(20)), Some((false, 2)));
    control.request_stop();
    assert_eq!(control.settle(Duration::from_millis(20)), None);
    assert_eq!(control.wait(2, Duration::from_secs(5)), Woke::Stop);
    assert!(control.stopping());
}
