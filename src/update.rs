//! Update check against the latest GitHub release, and installing its executable.
//! Pre-releases are never offered: GitHub's latest release is never one.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::install;

const LATEST_RELEASE: &str = "https://api.github.com/repos/trapplab/freisprech/releases/latest";

/// A release newer than the running version.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    /// Web page with the release notes.
    pub page: String,
    download: String,
    size: u64,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// The latest release, if it is newer than the running version.
pub async fn check() -> Result<Option<Release>> {
    let response = client()?
        .get(LATEST_RELEASE)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Failed to check for updates")?;
    let latest: GithubRelease = serde_json::from_slice(&response.bytes().await?)
        .context("Unexpected answer from GitHub")?;
    if !is_newer(&latest.tag_name, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let name = asset_name();
    let asset = latest
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .with_context(|| format!("Release {} has no {name} for this system", latest.tag_name))?;
    Ok(Some(Release {
        version: latest.tag_name,
        page: latest.html_url,
        download: asset.browser_download_url,
        size: asset.size,
    }))
}

/// Downloads the release and installs it over the installed copy. Returns the installed
/// path, to be started once this process has exited.
pub async fn install(release: Release) -> Result<PathBuf> {
    tracing::info!(version = release.version, "Downloading update");
    let data = client()?
        .get(&release.download)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Failed to download the update")?
        .bytes()
        .await
        .context("Failed to download the update")?;
    anyhow::ensure!(
        data.len() as u64 == release.size,
        "Download incomplete: {} of {} bytes",
        data.len(),
        release.size
    );
    tokio::task::spawn_blocking(move || install::install_update(&data, &release.version)).await?
}

fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        // GitHub's API rejects requests without one.
        .user_agent(concat!("freisprech/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()
}

/// As named by the release workflow, e.g. `freisprech-windows-x86_64.exe`.
fn asset_name() -> String {
    use std::env::consts::{ARCH, EXE_SUFFIX, OS};
    format!("freisprech-{OS}-{ARCH}{EXE_SUFFIX}")
}

/// Compares `major.minor.patch`; a suffix like `-rc1` is ignored.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(version: &str) -> Vec<u64> {
        let version = version.trim_start_matches('v');
        let core = version.split(['-', '+']).next().unwrap_or_default();
        core.split('.').map(|n| n.parse().unwrap_or(0)).collect()
    }
    parse(candidate) > parse(current)
}
