/// Passwords shorter than this many characters get a warning.
pub const MIN_PASSWORD_CHARS: usize = 12;

/// Result of the password policy check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordCheck {
    /// No remark.
    Ok,
    /// Shorter than [`MIN_PASSWORD_CHARS`]. This is a warning only; the
    /// caller must still allow the password.
    TooShort,
}

impl PasswordCheck {
    /// The user-facing warning, if any.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            PasswordCheck::Ok => None,
            PasswordCheck::TooShort => {
                Some("This password is shorter than 12 characters. A longer password is safer.")
            }
        }
    }
}

/// Checks a password against the policy. It never blocks: a short password
/// only produces [`PasswordCheck::TooShort`].
pub fn check_password(password: &str) -> PasswordCheck {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        PasswordCheck::TooShort
    } else {
        PasswordCheck::Ok
    }
}
