//! TimeReel `crypto` crate. Placeholder until its ticket is implemented.

/// Name of this crate, used by the placeholder test.
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(super::crate_name(), "timereel-crypto");
    }
}
