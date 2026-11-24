fn main() {
    println!("cargo:rerun-if-env-changed=PRESTMAN_RELEASE_TAG");
    let tag = std::env::var("PRESTMAN_RELEASE_TAG")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    let version = tag.strip_prefix('v').unwrap_or(&tag);
    semver::Version::parse(version).expect("PRESTMAN_RELEASE_TAG must be a SemVer version");
    println!("cargo:rustc-env=PRESTMAN_VERSION={version}");
}
