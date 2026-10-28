//! Generated bindings for KWin's privileged protocols (vendored XML in
//! `protocols/`, LGPL-2.1-or-later).

#![allow(non_upper_case_globals, non_camel_case_types, clippy::all)]

pub mod screencast {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/zkde-screencast-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/zkde-screencast-unstable-v1.xml");
}

pub mod window_management {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/plasma-window-management.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/plasma-window-management.xml");
}
