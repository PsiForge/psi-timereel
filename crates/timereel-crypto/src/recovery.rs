use bech32::{Bech32, Hrp};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::error::KeyError;

const HRP: &str = "trk";
const GROUP: usize = 4;

/// The recovery key: 32 random bytes shown once as a Bech32 string.
///
/// It is never written to disk. Its [`Debug`] output is redacted.
pub struct RecoveryKey {
    bytes: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryKey(<redacted>)")
    }
}

impl RecoveryKey {
    pub(crate) fn from_bytes(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self { bytes }
    }

    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// The string to show to the user once, e.g. `trk1-abcd-efgh-...`
    /// (lowercase Bech32 with a checksum, in groups of four characters).
    pub fn display(&self) -> SecretString {
        let encoded = bech32::encode::<Bech32>(Hrp::parse_unchecked(HRP), &self.bytes[..])
            .unwrap_or_default();
        let encoded = Zeroizing::new(encoded);
        let mut out = String::with_capacity(encoded.len() + encoded.len() / GROUP);
        for (i, c) in encoded.chars().enumerate() {
            if i > 0 && i % GROUP == 0 {
                out.push('-');
            }
            out.push(c);
        }
        SecretString::from(out)
    }

    /// Parses what the user typed: groups, spaces, dashes and case are
    /// ignored. A typo fails the Bech32 checksum and gives
    /// [`KeyError::WrongSecret`] (the same message as a wrong password).
    pub fn parse(text: &SecretString) -> Result<Self, KeyError> {
        let cleaned: Zeroizing<String> = Zeroizing::new(
            text.expose_secret()
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .collect::<String>()
                .to_ascii_lowercase(),
        );
        let (hrp, data) = bech32::decode(&cleaned).map_err(|_| KeyError::WrongSecret)?;
        let data = Zeroizing::new(data);
        if hrp.as_str() != HRP {
            return Err(KeyError::WrongSecret);
        }
        let bytes: [u8; 32] = data
            .as_slice()
            .try_into()
            .map_err(|_| KeyError::WrongSecret)?;
        Ok(Self::from_bytes(Zeroizing::new(bytes)))
    }
}
