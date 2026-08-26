//! GitHub release update check.
//!
//! Deliberately *only* a check: the client tells the user a newer version exists
//! and links to the release. Silent self-replacement would need code signing and
//! elevated writes on Windows, which is more risk than this project warrants.
//!
//! Uses a blocking HTTP call on purpose — it runs on a background thread at most
//! once per launch, and it keeps a second async TLS stack out of the binary.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// What the GitHub releases API gives us, narrowed to the fields we use.
#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub notes: String,
}

/// A semantic version, compared field by field.
///
/// Only `major.minor.patch` is understood; a pre-release suffix is ignored for
/// ordering, which is fine because pre-releases are filtered out earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32, u32);

impl Version {
    fn parse(text: &str) -> Result<Self> {
        let cleaned = text.trim().trim_start_matches(['v', 'V']);
        let core = cleaned.split(['-', '+']).next().unwrap_or(cleaned);
        let mut parts = core.split('.');
        let mut next = || -> Result<u32> {
            parts
                .next()
                .unwrap_or("0")
                .parse::<u32>()
                .with_context(|| format!("无法解析版本号: {text}"))
        };
        Ok(Version(next()?, next().unwrap_or(0), next().unwrap_or(0)))
    }
}

/// Compare `latest` against `current`, both as version strings.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Ok(l), Ok(c)) => l > c,
        // An unparsable tag is not worth nagging the user about.
        _ => false,
    }
}

/// Ask GitHub for the newest non-draft, non-prerelease release of `repo`
/// (`owner/name`). Returns `None` when the current build is up to date.
pub fn check(repo: &str, current_version: &str) -> Result<Option<UpdateInfo>> {
    if !repo.contains('/') {
        bail!("仓库格式应为 owner/repo: {repo}");
    }

    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = ureq::get(&url)
        .set("Accept", "application/vnd.github+json")
        // GitHub rejects requests without a User-Agent.
        .set("User-Agent", concat!("Synctus/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .context("请求 GitHub 更新信息失败")?;

    let release: GhRelease = response
        .into_json()
        .context("解析 GitHub 返回的更新信息失败")?;

    if release.draft || release.prerelease {
        return Ok(None);
    }
    if !is_newer(&release.tag_name, current_version) {
        return Ok(None);
    }

    Ok(Some(UpdateInfo {
        version: release.tag_name.trim_start_matches(['v', 'V']).to_string(),
        url: release.html_url,
        notes: release.body.chars().take(500).collect(),
    }))
}

/// Version of the running build.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn short_and_prefixed_tags_parse() {
        assert_eq!(Version::parse("v1").unwrap(), Version(1, 0, 0));
        assert_eq!(Version::parse("1.2").unwrap(), Version(1, 2, 0));
        assert_eq!(Version::parse("1.2.3-beta.1").unwrap(), Version(1, 2, 3));
    }

    #[test]
    fn garbage_tags_never_report_an_update() {
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
    }

    #[test]
    fn repo_must_look_like_owner_slash_name() {
        assert!(check("not-a-repo", "0.1.0").is_err());
    }
}
