use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayBackend {
    Wayland,
    X11,
    Unsupported,
}

pub(crate) fn display_backend(window: &impl IsA<gtk::Window>) -> DisplayBackend {
    classify_display_type(window.as_ref().display().type_().name())
}

fn classify_display_type(type_name: &str) -> DisplayBackend {
    match type_name {
        "GdkWaylandDisplay" => DisplayBackend::Wayland,
        "GdkX11Display" => DisplayBackend::X11,
        _ => DisplayBackend::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdk_backend_detection_is_type_exact() {
        assert_eq!(
            classify_display_type("GdkWaylandDisplay"),
            DisplayBackend::Wayland
        );
        assert_eq!(classify_display_type("GdkX11Display"), DisplayBackend::X11);
        assert_eq!(
            classify_display_type("wayland-0"),
            DisplayBackend::Unsupported
        );
        assert_eq!(classify_display_type(""), DisplayBackend::Unsupported);
    }
}
