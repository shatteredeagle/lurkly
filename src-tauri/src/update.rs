//! "Is there a newer release?" via the GitHub releases API. Notify-only: package-managed installs
//! (.deb, .rpm, Arch) should be updated by the package manager, so the app never replaces itself.

use serde_json::Value;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub struct Release {
    pub version: String,
    pub url: String,
}

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

pub fn is_newer(current: &str, latest: &str) -> bool {
    let (mut a, mut b) = (parts(current), parts(latest));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    b > a
}

/// The latest published (non-draft, non-prerelease) release, or None when there are none yet.
pub async fn latest(http: &reqwest::Client) -> Result<Option<Release>, String> {
    let api = env!("CARGO_PKG_REPOSITORY").replace("https://github.com/", "https://api.github.com/repos/") + "/releases/latest";
    let resp = http
        .get(api)
        .header("User-Agent", concat!("lurkly/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "Can't reach GitHub".to_string())?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let v: Value = resp.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("Unexpected GitHub response")?;
    Ok(Some(Release { version: tag.trim_start_matches('v').to_string(), url: v["html_url"].as_str().unwrap_or("").to_string() }))
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "v0.2.1"));
        assert!(is_newer("0.2.9", "0.10.0"));
        assert!(is_newer("0.2", "0.2.1"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.3.0", "v0.2.9"));
        assert!(!is_newer("0.2.0", "0.2.0-rc1"));
    }
}
