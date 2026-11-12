//! Session-type detection (architecture section 5).

/// The kind of graphical session the agent runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// An X11 session with a usable `DISPLAY`.
    X11,
    /// A Wayland session.
    Wayland,
    /// No usable graphical session was found.
    None,
}

/// Detects the session type from the process environment. See
/// [`detect_session_with`] for the rules.
pub fn detect_session() -> Session {
    detect_session_with(|name| std::env::var(name).ok())
}

/// Detects the session type with `get` as the source of environment
/// variables, so tests need not touch the real environment.
///
/// `XDG_SESSION_TYPE` is checked first: `wayland` gives [`Session::Wayland`],
/// and `x11` gives [`Session::X11`] if `DISPLAY` is set (without it nothing
/// can be captured, so the answer is [`Session::None`]). Any other value, or
/// none, falls back to `WAYLAND_DISPLAY` and then `DISPLAY`. Empty values
/// count as unset.
pub fn detect_session_with(get: impl Fn(&str) -> Option<String>) -> Session {
    let set = |name: &str| get(name).is_some_and(|v| !v.trim().is_empty());
    let session_type = get("XDG_SESSION_TYPE").map(|v| v.trim().to_ascii_lowercase());
    match session_type.as_deref() {
        Some("wayland") => return Session::Wayland,
        Some("x11") => {
            return if set("DISPLAY") {
                Session::X11
            } else {
                Session::None
            }
        }
        _ => {}
    }
    if set("WAYLAND_DISPLAY") {
        Session::Wayland
    } else if set("DISPLAY") {
        Session::X11
    } else {
        Session::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(vars: &[(&str, &str)]) -> Session {
        detect_session_with(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        })
    }

    #[test]
    fn x11() {
        assert_eq!(
            detect(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")]),
            Session::X11
        );
        assert_eq!(detect(&[("DISPLAY", ":1")]), Session::X11);
        // An unknown session type falls back to the display variables.
        assert_eq!(
            detect(&[("XDG_SESSION_TYPE", "tty"), ("DISPLAY", ":0")]),
            Session::X11
        );
    }

    #[test]
    fn wayland() {
        assert_eq!(
            detect(&[
                ("XDG_SESSION_TYPE", "wayland"),
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("DISPLAY", ":0"),
            ]),
            Session::Wayland
        );
        assert_eq!(
            detect(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]),
            Session::Wayland
        );
        assert_eq!(detect(&[("XDG_SESSION_TYPE", "Wayland")]), Session::Wayland);
    }

    #[test]
    fn none() {
        assert_eq!(detect(&[]), Session::None);
        assert_eq!(
            detect(&[("DISPLAY", ""), ("WAYLAND_DISPLAY", " ")]),
            Session::None
        );
        // An X11 session without DISPLAY cannot be captured.
        assert_eq!(detect(&[("XDG_SESSION_TYPE", "x11")]), Session::None);
        assert_eq!(detect(&[("XDG_SESSION_TYPE", "tty")]), Session::None);
    }
}
