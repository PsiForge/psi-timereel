//! `timereel-tray` binary. Currently only prints its version.

/// Returns the version line printed by this binary.
fn version_line() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

fn main() {
    println!("{}", version_line());
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_line_has_name_and_version() {
        let line = super::version_line();
        assert!(line.starts_with("timereel-tray "));
        assert!(line.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
