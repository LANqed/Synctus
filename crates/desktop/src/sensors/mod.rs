//! Reading local state from the OS.
//!
//! Every platform module exposes the same four functions and returns `None` when
//! the information is unavailable, so the caller never has to branch on the
//! platform. Nothing here is allowed to panic or block for long: the sampler
//! runs on the UI thread's timer.

use synctus_core::model::{Battery, ForegroundApp, NowPlaying};

/// One sample of everything the OS can tell us.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub foreground: Option<ForegroundApp>,
    pub battery: Option<Battery>,
    pub music: Option<NowPlaying>,
    pub idle_secs: Option<u32>,
}

#[cfg(windows)]
mod platform {
    pub use super::windows_impl::*;
}

#[cfg(target_os = "linux")]
mod platform {
    pub use super::linux_impl::*;
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::*;
    pub fn foreground() -> Option<ForegroundApp> {
        None
    }
    pub fn battery() -> Option<Battery> {
        None
    }
    pub fn music() -> Option<NowPlaying> {
        None
    }
    pub fn idle_secs() -> Option<u32> {
        None
    }
}

#[cfg(windows)]
mod windows_impl;

#[cfg(target_os = "linux")]
mod linux_impl;

/// Sample everything. Individual failures degrade to `None`.
pub fn sample() -> Sample {
    Sample {
        foreground: platform::foreground(),
        battery: platform::battery(),
        music: platform::music(),
        idle_secs: platform::idle_secs(),
    }
}

/// Trim a window title for display and publishing.
///
/// Long titles are the common case (browsers append the site name), and the
/// overlay only has room for a line, so cut early rather than shipping kilobytes
/// in every snapshot.
pub fn trim_title(title: &str) -> Option<String> {
    let t = title.trim();
    if t.is_empty() {
        return None;
    }
    let mut out: String = t.chars().take(120).collect();
    if t.chars().count() > 120 {
        out.push('…');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_never_panics() {
        // The values depend on the machine; the point is that it returns.
        let _ = sample();
    }

    #[test]
    fn titles_are_trimmed_and_truncated() {
        assert_eq!(trim_title("  hello  "), Some("hello".to_string()));
        assert_eq!(trim_title("   "), None);

        let long = "x".repeat(200);
        let out = trim_title(&long).unwrap();
        assert_eq!(out.chars().count(), 121, "120 chars plus the ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_titles_are_cut_on_char_boundaries() {
        let long = "中".repeat(200);
        let out = trim_title(&long).unwrap();
        assert!(out.starts_with('中'));
        assert_eq!(out.chars().count(), 121);
    }
}
