use super::*;

fn release(version: &str, name: &str) -> Release {
    Release::builder()
        .version(version)
        .asset(
            ReleaseAsset::new(name, format!("{ASSETS_API}/123"))
                .with_digest(format!("sha256:{}", "ab".repeat(32))),
        )
        .build()
        .unwrap()
}

#[test]
fn compares_semver_without_downgrades_prereleases_or_build_only_changes() {
    for (current, next, expected) in [
        ("0.1.9", "v0.1.10", true),
        ("0.1.0", "1.0.0", true),
        ("0.2.0", "0.1.99", false),
        ("0.2.0", "v0.2.0", false),
        ("0.2.0", "0.3.0-beta.1", false),
        ("0.2.0-beta.1", "0.2.0", true),
        ("0.2.0+old", "0.2.0+new", false),
    ] {
        assert_eq!(
            newer_stable(current, next).unwrap(),
            expected,
            "{current} -> {next}"
        );
    }
    assert!(newer_stable("0.1.0", "nightly").is_err());
}

#[test]
fn all_release_targets_have_distinct_exact_assets() {
    let targets = [
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ];
    let names = targets.map(|target| asset_name(target).unwrap());
    assert_eq!(
        names
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        6
    );
    for (target, name) in targets.into_iter().zip(names) {
        assert!(
            select_release(release("0.2.0", name), "0.1.0", target)
                .unwrap()
                .is_some()
        );
    }
    assert!(asset_name("x86_64-unknown-linux-musl").is_err());
}

#[test]
fn wrong_architecture_missing_checksum_and_duplicate_assets_are_rejected() {
    let target = "x86_64-pc-windows-msvc";
    let name = asset_name(target).unwrap();
    assert!(
        select_release(
            release("0.2.0", "prestman-windows-arm64.zip"),
            "0.1.0",
            target
        )
        .is_err()
    );
    let valid = release("0.2.0", name);
    for assets in [
        vec![ReleaseAsset::new(name, valid.assets()[0].download_url())],
        vec![valid.assets()[0].clone(), valid.assets()[0].clone()],
    ] {
        let bad = Release::builder()
            .version("0.2.0")
            .assets(assets)
            .build()
            .unwrap();
        assert!(select_release(bad, "0.1.0", target).is_err());
    }
}

#[test]
fn current_release_does_not_require_update_assets() {
    let release = Release::builder().version("0.1.0").build().unwrap();
    assert!(
        select_release(release, "0.1.0", "x86_64-pc-windows-msvc")
            .unwrap()
            .is_none()
    );
}

#[test]
fn unexpected_download_hosts_and_invalid_digests_are_rejected() {
    let name = "prestman-windows-x64.zip";
    let valid = release("0.2.0", name);
    for (url, digest) in [
        (
            "http://api.github.com/repos/OnlyDebugAlerts/prestman/releases/assets/123",
            format!("sha256:{}", "ab".repeat(32)),
        ),
        (
            "https://example.com/file",
            format!("sha256:{}", "ab".repeat(32)),
        ),
        (
            "https://api.github.com/repos/another/repo/releases/assets/123",
            format!("sha256:{}", "ab".repeat(32)),
        ),
        (valid.assets()[0].download_url(), "sha256:1234".into()),
        (
            valid.assets()[0].download_url(),
            format!("sha256:{}", "zz".repeat(32)),
        ),
    ] {
        let bad = Release::builder()
            .version("0.2.0")
            .asset(ReleaseAsset::new(name, url).with_digest(digest))
            .build()
            .unwrap();
        assert!(select_release(bad, "0.1.0", "x86_64-pc-windows-msvc").is_err());
    }
}

#[test]
fn accepted_selection_keeps_its_version_url_and_digest() {
    let selected = select_release(
        release("0.2.0", "prestman-windows-x64.zip"),
        "0.1.0",
        "x86_64-pc-windows-msvc",
    )
    .unwrap()
    .unwrap();
    let frozen = selected.get_releases().unwrap();
    assert_eq!(frozen.len(), 1);
    assert_eq!(frozen[0].version(), "0.2.0");
    assert_eq!(
        frozen[0].assets()[0].download_url(),
        selected.asset.download_url()
    );
    assert_eq!(frozen[0].assets()[0].digest(), selected.asset.digest());
}

#[cfg(target_os = "windows")]
#[test]
fn windows_update_checks_digest_before_replacing_the_file() -> Result<()> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use self_update::{
        http::{HeaderMap, header::ACCEPT},
        http_client::{HttpClient, HttpResponse},
    };
    use std::{fs, io::Cursor, sync::Arc};

    struct Download(Vec<u8>);
    struct Response(HeaderMap, Vec<u8>);
    impl HttpClient for Download {
        fn get(
            &self,
            url: &str,
            headers: &HeaderMap,
            _: Option<Duration>,
        ) -> self_update::Result<Box<dyn HttpResponse>> {
            assert_eq!(url, format!("{ASSETS_API}/123"));
            assert_eq!(headers[ACCEPT], "application/octet-stream");
            Ok(Box::new(Response(HeaderMap::new(), self.0.clone())))
        }
    }
    impl HttpResponse for Response {
        fn headers(&self) -> &HeaderMap {
            &self.0
        }

        fn body(self: Box<Self>) -> Box<dyn std::io::Read> {
            Box::new(Cursor::new(self.1))
        }
    }

    // A deflated ZIP with one entry, prestman.exe.
    let zip = STANDARD.decode(concat!(
        "UEsDBBQAAAAIAIqReF03UTG7FQAAABMAAAAMAAAAcHJlc3RtYW4uZXhlKy1IL0pMSU1RSK1ITS4tSUzKSQUA",
        "UEsBAhQAFAAAAAgAipF4XTdRMbsVAAAAEwAAAAwAAAAAAAAAAAAAAIABAAAAAHByZXN0bWFuLmV4ZVBLBQYAAAAAAQABADoAAAA/AAAAAAA=",
    ))?;
    let target = self_update::get_target();
    let name = asset_name(target)?;
    let directory = tempfile::tempdir_in(std::env::current_dir()?)?;
    let installed = directory.path().join("prestman.exe");
    fs::write(&installed, b"previous executable")?;
    let download = Arc::new(Download(zip));

    let run_update = |digest: &str| -> Result<_> {
        let release = Release::builder()
            .version("0.2.0")
            .asset(ReleaseAsset::new(name, format!("{ASSETS_API}/123")).with_digest(digest))
            .build()?;
        let update = select_release(release, "0.1.0", target)?.context("No update selected")?;
        Ok(install_builder(update, "0.1.0")
            .bin_install_path(&installed)
            .http_client(download.clone())
            .build()?
            .update())
    };
    assert!(matches!(
        run_update(&format!("sha256:{}", "00".repeat(32)))?,
        Err(self_update::Error::ChecksumMismatch { .. })
    ));
    assert_eq!(fs::read(&installed)?, b"previous executable");

    let digest = "sha256:f8916f3b4258c9593cdb1fa3b83bbd332627ec882d4155db3687a78599669a23";
    assert!(run_update(digest)??.is_updated());
    assert_eq!(fs::read(installed)?, b"upgraded executable");
    Ok(())
}
