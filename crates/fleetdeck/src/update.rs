//! `fleetdeck update`: replace the running binary with the latest GitHub Release.
//!
//! The release workflow publishes `fleetdeck-<version>-<target>.tar.gz` for each target and a
//! `SHA256SUMS` file. The update checks the archive against `SHA256SUMS` before it extracts it,
//! and `self_update` swaps the binary in with a rename, so a failed update keeps the old binary.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use self_update::backends::github::Update;
use self_update::{ReleaseAsset, VersionStatus};

const OWNER: &str = "jayjongcheolpark";
const REPO: &str = "fleetdeck";
const BIN: &str = "fleetdeck";
const SUMS: &str = "SHA256SUMS";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The archive name that `.github/workflows/release.yml` publishes.
pub fn archive_name(version: &str, target: &str) -> String {
    format!("{BIN}-{version}-{target}.tar.gz")
}

/// Picks the archive for `target` from the asset names of one release.
/// The name must match exactly, so `x86_64-apple-darwin` never matches another target.
pub fn select_asset<'a>(names: &[&'a str], version: &str, target: &str) -> Option<&'a str> {
    let want = archive_name(version, target);
    names.iter().copied().find(|n| *n == want)
}

/// True when `latest` (a tag such as `v0.2.0` or a bare version) is newer than `current`.
pub fn is_newer(current: &str, latest: &str) -> Result<bool> {
    let latest = latest.strip_prefix('v').unwrap_or(latest);
    self_update::version::bump_is_greater(current, latest)
        .with_context(|| format!("cannot compare versions {current} and {latest}"))
}

/// Picks the archive for `target` from the assets of one release.
/// The matcher does not get the release version, so it reads it from the archive names.
fn match_archive(assets: &[ReleaseAsset], target: &str) -> Option<ReleaseAsset> {
    let names: Vec<&str> = assets.iter().map(|a| a.name()).collect();
    let prefix = format!("{BIN}-");
    let suffix = format!("-{target}.tar.gz");
    let version = names
        .iter()
        .find_map(|n| n.strip_prefix(&prefix)?.strip_suffix(&suffix))?;
    let name = select_asset(&names, version, target)?;
    assets.iter().find(|a| a.name() == name).cloned()
}

fn updater(target: &'static str) -> Result<Update> {
    let built = Update::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .bin_name(BIN)
        .target(target)
        .current_version(CURRENT)
        .bin_path_in_archive("{{ bin }}-{{ version }}-{{ target }}/{{ bin }}")
        .asset_matcher(move |assets: &[ReleaseAsset]| match_archive(assets, target))
        .checksum_from_asset(SUMS)
        .verify_binary(|new_exe: &Path| check_new_binary(new_exe))
        .show_output(false)
        .show_download_progress(false)
        .no_confirm(true)
        .build()?;
    Ok(built)
}

/// Runs the downloaded binary once before it replaces the installed one.
fn check_new_binary(new_exe: &Path) -> self_update::errors::Result<()> {
    let out = Command::new(new_exe)
        .arg("--version")
        .output()
        .map_err(|e| self_update::errors::Error::verification_rejected(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && text.starts_with("fleetdeck ") {
        Ok(())
    } else {
        Err(self_update::errors::Error::verification_rejected(format!(
            "the new binary did not print its version: {}",
            text.trim()
        )))
    }
}

/// `fleetdeck update [--check]`.
pub fn run(check: bool) -> Result<()> {
    let target = self_update::get_target();
    let updater = updater(target)?;
    let latest = updater
        .get_latest_release()
        .context("cannot read the latest GitHub Release")?;
    let Some(release) = latest.all().first() else {
        bail!("{OWNER}/{REPO} has no releases");
    };
    let version = release.version().to_owned();
    if !is_newer(CURRENT, &version)? {
        println!("fleetdeck {CURRENT} is the latest version.");
        return Ok(());
    }
    if check {
        println!("fleetdeck {version} is available (you have {CURRENT}).");
        println!("Run `fleetdeck update` to install it.");
        return Ok(());
    }
    println!("Updating fleetdeck {CURRENT} to {version} for {target}.");
    match updater
        .update()
        .context("update failed; the installed binary is unchanged")?
    {
        VersionStatus::Updated(v) => println!("fleetdeck is now {v}."),
        other => println!("fleetdeck {} is the latest version.", other.version()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use self_update::Checksum;

    const TARGETS: [&str; 4] = [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ];

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.0", "v0.2.0").unwrap());
        assert!(is_newer("0.1.0", "0.1.1").unwrap());
        assert!(!is_newer("0.2.0", "v0.2.0").unwrap());
        assert!(!is_newer("0.2.0", "v0.1.9").unwrap());
        assert!(is_newer("0.9.0", "0.10.0").unwrap());
        assert!(is_newer("0.1.0", "not-a-version").is_err());
    }

    #[test]
    fn selects_the_archive_for_each_target() {
        let names: Vec<String> = TARGETS
            .iter()
            .map(|t| archive_name("0.2.0", t))
            .chain([SUMS.to_string()])
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        for t in TARGETS {
            assert_eq!(
                select_asset(&names, "0.2.0", t),
                Some(format!("fleetdeck-0.2.0-{t}.tar.gz").as_str())
            );
        }
        assert_eq!(
            select_asset(&names, "0.2.0", "x86_64-unknown-linux-musl"),
            None
        );
        assert_eq!(select_asset(&names, "0.1.0", TARGETS[0]), None);
    }

    #[test]
    fn matcher_picks_the_running_target() {
        let assets: Vec<ReleaseAsset> = TARGETS
            .iter()
            .map(|t| archive_name("0.2.0", t))
            .chain([SUMS.to_string()])
            .map(|n| ReleaseAsset::new(n.clone(), format!("https://example.invalid/{n}")))
            .collect();
        for t in TARGETS {
            let chosen = match_archive(&assets, t).expect("an archive matches");
            assert_eq!(chosen.name(), format!("fleetdeck-0.2.0-{t}.tar.gz"));
        }
        assert!(match_archive(&assets, "x86_64-unknown-linux-musl").is_none());
    }

    fn hex_sha256(body: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The `SHA256SUMS` shape that `sha256sum -- *.tar.gz` writes in the release workflow.
    fn sums_for(name: &str, body: &[u8]) -> String {
        let hex = hex_sha256(body);
        format!(
            "{}  fleetdeck-0.2.0-x86_64-unknown-linux-gnu.tar.gz\n{hex}  {name}\n",
            "0".repeat(64)
        )
    }

    #[test]
    fn verifies_the_archive_against_sha256sums() {
        let name = archive_name("0.2.0", "aarch64-apple-darwin");

        let sums = sums_for(&name, b"archive bytes");
        let Checksum::Sha256(want) = Checksum::from_sums_file(&sums, &name).unwrap() else {
            panic!("expected a SHA-256 entry");
        };
        assert_eq!(want, hex_sha256(b"archive bytes"));
        assert_ne!(want, hex_sha256(b"tampered bytes"));

        let missing = archive_name("0.2.0", "x86_64-apple-darwin");
        assert!(Checksum::from_sums_file(&sums, &missing).is_err());
    }
}
