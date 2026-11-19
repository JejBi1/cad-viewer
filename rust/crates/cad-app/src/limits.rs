//! Resource limits of the thumbnail renderer (`main.limit_thumbnail_resources`).
//!
//! The renderer parses untrusted files automatically (file manager, indexers), so its memory (address space) and CPU
//! time are bounded even when it is started by something other than our `.thumbnailer`, which also wraps it in
//! `timeout` and `prlimit`. Limits are only ever lowered. On Windows the sandbox of the shell extension
//! (`shellext/Sandbox.cs`: job object, low integrity) does this from outside.

/// Address space limit, 4 GiB.
#[cfg_attr(not(unix), allow(dead_code))]
pub const ADDRESS_SPACE: u64 = 4 * 1024 * 1024 * 1024;
/// CPU seconds (soft); the hard limit is 5 s later (SIGXCPU, then SIGKILL).
#[cfg_attr(not(unix), allow(dead_code))]
pub const CPU_SECONDS: u64 = 120;

/// The `(soft, hard)` pair to set for a resource, or `None` when the current soft limit is already at or below
/// `want`. `RLIM_INFINITY` is passed as `u64::MAX`.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn lowered(soft: u64, hard: u64, want: u64, hard_want: u64) -> Option<(u64, u64)> {
    if soft != u64::MAX && soft <= want {
        return None;
    }
    let new_hard = if hard == u64::MAX { hard_want } else { hard.min(hard_want) };
    Some((want.min(new_hard), new_hard))
}

#[cfg(unix)]
pub fn limit_thumbnail_resources() {
    // a macro, because the type of the resource constants differs between C libraries
    macro_rules! apply {
        ($resource:expr, $want:expr, $hard_want:expr) => {{
            let mut cur = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: plain syscalls on a local struct
            unsafe {
                if libc::getrlimit($resource, &mut cur) == 0 {
                    let inf = libc::RLIM_INFINITY as u64;
                    let norm = |v: u64| if v == inf { u64::MAX } else { v };
                    if let Some((soft, hard)) =
                        lowered(norm(cur.rlim_cur as u64), norm(cur.rlim_max as u64), $want, $hard_want)
                    {
                        let new = libc::rlimit { rlim_cur: soft as _, rlim_max: hard as _ };
                        let _ = libc::setrlimit($resource, &new);
                    }
                }
            }
        }};
    }
    apply!(libc::RLIMIT_AS, ADDRESS_SPACE, ADDRESS_SPACE);
    apply!(libc::RLIMIT_CPU, CPU_SECONDS, CPU_SECONDS + 5);
}

#[cfg(not(unix))]
pub fn limit_thumbnail_resources() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_only_lowered() {
        assert_eq!(lowered(u64::MAX, u64::MAX, 10, 20), Some((10, 20)));
        assert_eq!(lowered(5, u64::MAX, 10, 20), None);
        assert_eq!(lowered(10, 50, 10, 20), None);
        assert_eq!(lowered(100, 50, 10, 20), Some((10, 20)));
        assert_eq!(lowered(100, 15, 10, 20), Some((10, 15)));
        // soft never above the hard limit
        assert_eq!(lowered(u64::MAX, 8, 10, 20), Some((8, 8)));
    }
}
