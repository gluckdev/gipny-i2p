use gio::{BusType, DBusSignalFlags};

thread_local! {
    static BUSES: std::cell::RefCell<Vec<gio::DBusConnection>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn install() {
    gio::bus_get(BusType::Session, gio::Cancellable::NONE, |result| {
        let Ok(bus) = result else { return };
        for (name, path) in [
            ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver"),
            ("org.freedesktop.ScreenSaver", "/ScreenSaver"),
            (
                "org.freedesktop.ScreenSaver",
                "/org/freedesktop/ScreenSaver",
            ),
            ("org.mate.ScreenSaver", "/org/mate/ScreenSaver"),
            ("org.xfce.ScreenSaver", "/org/xfce/ScreenSaver"),
        ] {
            bus.signal_subscribe(
                Some(name),
                Some(name),
                Some("ActiveChanged"),
                Some(path),
                None,
                DBusSignalFlags::NONE,
                |_, _, _, _, _, value| {
                    if value.get::<(bool,)>() == Some((true,)) {
                        super::cover();
                    }
                },
            );
        }
        // GNOME can power off its screens before the screen saver locks.
        bus.signal_subscribe(
            Some("org.gnome.Mutter.DisplayConfig"),
            Some("org.freedesktop.DBus.Properties"),
            Some("PropertiesChanged"),
            Some("/org/gnome/Mutter/DisplayConfig"),
            Some("org.gnome.Mutter.DisplayConfig"),
            DBusSignalFlags::NONE,
            |_, _, _, _, _, params| {
                let values = gio::glib::VariantDict::new(Some(&params.child_value(1)));
                if matches!(values.lookup::<i32>("PowerSaveMode"), Ok(Some(1..=3))) {
                    super::cover();
                }
            },
        );
        BUSES.with(|buses| buses.borrow_mut().push(bus));
    });
    gio::bus_get(BusType::System, gio::Cancellable::NONE, |result| {
        let Ok(bus) = result else { return };
        bus.signal_subscribe(
            Some("org.freedesktop.login1"),
            Some("org.freedesktop.login1.Manager"),
            Some("PrepareForSleep"),
            Some("/org/freedesktop/login1"),
            None,
            DBusSignalFlags::NONE,
            |_, _, _, _, _, value| {
                if value.get::<(bool,)>() == Some((true,)) {
                    super::cover();
                }
            },
        );
        BUSES.with(|buses| buses.borrow_mut().push(bus));
    });
}
