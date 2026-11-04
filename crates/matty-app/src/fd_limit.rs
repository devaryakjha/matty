//! The open file limit (#206). Every saved server runs its own sync engine,
//! each holding its database, sockets and media files, about 17 descriptors
//! apiece, and macOS starts an app with a soft limit of 256. At launch
//! Matty raises its soft limit as far as [`WANTED`], within the hard limit,
//! and never lowers it.

/// The soft limit Matty asks for: room for dozens of servers with fonts,
/// images and logs open alongside.
pub const WANTED: u64 = 4096;

/// The soft limit to set, given the current soft and hard limits (`None`
/// is unlimited); `None` when the current one is high enough already.
pub fn raised(current: Option<u64>, maximum: Option<u64>) -> Option<u64> {
    let target = maximum.map_or(WANTED, |maximum| maximum.min(WANTED));
    match current {
        Some(current) if current < target => Some(target),
        _ => None,
    }
}

/// Raises the soft open file limit and logs the outcome.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn raise() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let limit = getrlimit(Resource::Nofile);
    let Some(soft) = raised(limit.current, limit.maximum) else {
        tracing::info!(
            soft = ?limit.current,
            hard = ?limit.maximum,
            "the open file limit is high enough"
        );
        return;
    };
    let wanted = Rlimit {
        current: Some(soft),
        maximum: limit.maximum,
    };
    match setrlimit(Resource::Nofile, wanted) {
        Ok(()) => tracing::info!(
            from = ?limit.current,
            to = soft,
            hard = ?limit.maximum,
            "raised the open file limit"
        ),
        Err(error) => tracing::warn!(
            %error,
            soft = ?limit.current,
            hard = ?limit.maximum,
            "could not raise the open file limit"
        ),
    }
}

/// Other systems keep their limit: Windows has no such small default.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_family = "wasm")))]
pub fn raise() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raises_a_low_limit_within_the_hard_one() {
        assert_eq!(raised(Some(256), None), Some(WANTED));
        assert_eq!(raised(Some(256), Some(10_240)), Some(WANTED));
        assert_eq!(raised(Some(256), Some(1024)), Some(1024));
        assert_eq!(raised(Some(1023), Some(1024)), Some(1024));
    }

    #[test]
    fn never_lowers_the_limit() {
        for current in [
            None,
            Some(0),
            Some(256),
            Some(1024),
            Some(4096),
            Some(65_536),
        ] {
            for maximum in [
                None,
                Some(0),
                Some(256),
                Some(1024),
                Some(4096),
                Some(524_288),
            ] {
                if let Some(soft) = raised(current, maximum) {
                    let current = current.unwrap_or(u64::MAX);
                    assert!(soft > current, "{current:?} -> {soft} under {maximum:?}");
                    assert!(maximum.is_none_or(|maximum| soft <= maximum));
                }
            }
        }
        assert_eq!(raised(Some(65_536), None), None);
        assert_eq!(raised(None, None), None);
        assert_eq!(raised(Some(WANTED), Some(524_288)), None);
    }

    /// The real limit is never below where it started.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn raise_keeps_or_raises_the_real_limit() {
        use rustix::process::{Resource, getrlimit};
        let before = getrlimit(Resource::Nofile).current.unwrap_or(u64::MAX);
        raise();
        let after = getrlimit(Resource::Nofile).current.unwrap_or(u64::MAX);
        assert!(after >= before);
        assert!(
            after
                >= before
                    .min(WANTED)
                    .min(getrlimit(Resource::Nofile).maximum.unwrap_or(u64::MAX))
        );
    }
}
