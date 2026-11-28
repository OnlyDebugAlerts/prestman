use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use self_update::{Release, ReleaseAsset, ReleaseSource, backends::custom};
use semver::Version;

pub const VERSION: &str = env!("PRESTMAN_VERSION");
pub const AUTO_INSTALL: bool = option_env!("PRESTMAN_RELEASE_TAG").is_some();
pub const RELEASES_URL: &str = "https://github.com/OnlyDebugAlerts/prestman/releases";
const ASSETS_API: &str = "https://api.github.com/repos/OnlyDebugAlerts/prestman/releases/assets";

#[derive(Clone, Debug)]
pub struct AvailableUpdate {
    pub version: String,
    release: Release,
    asset: ReleaseAsset,
}

// Keep the same asset URL and digest from discovery through installation.
impl ReleaseSource for AvailableUpdate {
    fn get_releases(&self) -> self_update::Result<Vec<Release>> {
        Ok(vec![self.release.clone()])
    }
}

fn asset_name(target: &str) -> Result<&'static str> {
    Ok(match target {
        "x86_64-pc-windows-msvc" => "prestman-windows-x64.zip",
        "aarch64-pc-windows-msvc" => "prestman-windows-arm64.zip",
        "x86_64-unknown-linux-gnu" => "prestman-linux-x64.tar.gz",
        "aarch64-unknown-linux-gnu" => "prestman-linux-arm64.tar.gz",
        "x86_64-apple-darwin" => "prestman-macos-x64.zip",
        "aarch64-apple-darwin" => "prestman-macos-arm64.zip",
        _ => anyhow::bail!("Automatic updates are not available for {target}"),
    })
}

fn newer_stable(current: &str, candidate: &str) -> Result<bool> {
    let current = Version::parse(current.strip_prefix('v').unwrap_or(current))?;
    let candidate = Version::parse(candidate.strip_prefix('v').unwrap_or(candidate))?;
    Ok(candidate.pre.is_empty() && candidate.cmp_precedence(&current).is_gt())
}

fn select_release(
    release: Release,
    current: &str,
    target: &str,
) -> Result<Option<AvailableUpdate>> {
    if !newer_stable(current, release.version())? {
        return Ok(None);
    }
    let name = asset_name(target)?;
    let mut matches = release.assets().iter().filter(|asset| asset.name() == name);
    let asset = matches
        .next()
        .with_context(|| format!("Release is missing {name}"))?
        .clone();
    ensure!(
        matches.next().is_none(),
        "Release contains more than one {name}"
    );

    // The GitHub backend uses the asset API URL with Accept: application/octet-stream.
    let (assets_api, asset_id) = asset.download_url().rsplit_once('/').unwrap_or_default();
    ensure!(
        assets_api.eq_ignore_ascii_case(ASSETS_API)
            && !asset_id.is_empty()
            && asset_id.bytes().all(|b| b.is_ascii_digit()),
        "Unexpected release download URL"
    );
    let digest = asset
        .digest()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    ensure!(
        digest.is_some_and(
            |digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
        ),
        "Release asset has no valid SHA-256 digest"
    );
    Ok(Some(AvailableUpdate {
        version: release.version().to_owned(),
        release,
        asset,
    }))
}

pub fn check() -> Result<Option<AvailableUpdate>> {
    let target = self_update::get_target();
    asset_name(target)?;
    let checker = self_update::backends::github::Update::configure()
        .repo_owner("OnlyDebugAlerts")
        .repo_name("prestman")
        .bin_name("prestman")
        .current_version(VERSION)
        .timeout(Duration::from_secs(20))
        .show_output(false)
        .build()?;
    // GitHub's latest endpoint excludes drafts and prereleases, and returns 404
    // if the repository has no published stable release yet.
    let releases = match checker.get_latest_release() {
        Ok(releases) => releases,
        Err(self_update::Error::NotFound { .. }) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    releases
        .latest()
        .cloned()
        .map_or(Ok(None), |release| select_release(release, VERSION, target))
}

pub fn install(
    update: AvailableUpdate,
    before_replace: impl Fn() -> self_update::Result<()> + Send + Sync + 'static,
) -> Result<()> {
    ensure!(
        install_builder(update, VERSION, before_replace)
            .build()?
            .update()?
            .is_updated(),
        "No update was installed"
    );
    Ok(())
}

fn install_builder(
    update: AvailableUpdate,
    current: &str,
    before_replace: impl Fn() -> self_update::Result<()> + Send + Sync + 'static,
) -> custom::UpdateBuilder {
    let asset = update.asset.clone();
    let mut builder = custom::Update::configure();
    builder
        .source(update)
        .bin_name("prestman")
        .current_version(current)
        .asset_matcher(move |_| Some(asset.clone()))
        .verify_release_digest(true)
        .timeout(Duration::from_secs(15 * 60))
        .no_confirm(true)
        .show_output(false)
        .show_download_progress(false)
        .verify_binary(move |_staged| {
            #[cfg(target_os = "macos")]
            verify_bundle(_staged)?;
            before_replace()
        });

    #[cfg(target_os = "macos")]
    builder.bundle_path_in_archive("Prestman.app");
    #[cfg(target_os = "windows")]
    builder.bin_path_in_archive("prestman.exe");
    #[cfg(target_os = "linux")]
    builder.bin_path_in_archive("prestman");

    builder
}

#[cfg(target_os = "macos")]
fn verify_bundle(bundle: &std::path::Path) -> self_update::Result<()> {
    let status = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .status()?;
    if !status.success() {
        return Err(self_update::Error::verification_rejected(
            "The application bundle signature is invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
