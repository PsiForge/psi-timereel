use std::io;

/// Errors of the key file operations.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The password or recovery key is not correct. This one error is used
    /// for every unlock failure, so nothing is revealed about the cause.
    #[error("Password or recovery key is not correct")]
    WrongSecret,
    /// The key file is malformed (bad JSON, unknown version, bad lengths or
    /// out-of-range parameters).
    #[error("The key file is damaged: {0}")]
    Corrupt(String),
    /// Reading or writing the key file failed.
    #[error("Cannot access the key file: {0}")]
    Io(#[from] io::Error),
    /// The system could not provide random bytes or key derivation failed to
    /// run (for example not enough memory for the Argon2 parameters).
    #[error("Cannot create keys: {0}")]
    Crypto(String),
}
