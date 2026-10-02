//! Opt-in RAILGUN shielded balances for Vaughan.
//!
//! Wraps the pinned upstream `railgun-rust` crates (`railgun-wallet`,
//! `broadcaster-core`) behind the `enabled` feature. Without that feature the
//! crate is an empty stub, so the default workspace build never compiles the
//! RAILGUN stack. Frontends turn it on through their own `railgun` feature.
//!
//! Plan, threat model and pin-bump procedure: `docs/railgun-go-no-go.md`.

/// Whether this build includes RAILGUN support (the `enabled` feature).
#[must_use]
pub const fn is_enabled() -> bool {
    cfg!(feature = "enabled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_flag_matches_feature() {
        assert_eq!(is_enabled(), cfg!(feature = "enabled"));
    }
}
