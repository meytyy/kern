//! Update check against the GitHub Releases of meytyy/kern. Nothing is downloaded
//! or installed here: the result is a direct link that the user opens.
use serde::Deserialize;
use std::cmp::Ordering;
use std::process::Command;
use std::time::Duration;

pub const REPO: &str = "meytyy/kern";
const RELEASES_API: &str = "https://api.github.com/repos/meytyy/kern/releases?per_page=30";

/// Release file names, shared with scripts/bundle.sh, scripts/build-windows.ps1,
/// the release workflow and the README download links. The first match wins.
pub fn asset_names(os: &str, arch: &str) -> &'static [&'static str] {
    match (os, arch) {
        ("macos", "aarch64") => &["KERN-macos-arm64.zip"],
        ("windows", "x86_64") => &["KERN-windows-x64-setup.exe", "KERN-windows-x64.zip"],
        _ => &[],
    }
}

/// Human name of the current platform's build, for messages.
pub fn platform_label(os: &str, arch: &str) -> String {
    match (os, arch) {
        ("macos", "aarch64") => "macOS Apple Silicon".into(),
        ("macos", _) => format!("macOS {arch}"),
        ("windows", "x86_64") => "Windows x64".into(),
        _ => format!("{os} {arch}"),
    }
}

/// A SemVer version: `1.2.3`, `v1.2.3`, `1.2.3-beta.1`. Build metadata is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, p.split('.').map(str::to_string).collect()),
            Some(_) => return None,
            None => (s, Vec::new()),
        };
        let mut parts = core.split('.');
        let num = |p: Option<&str>| -> Option<u64> {
            let p = p?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let v = Version {
            major: num(parts.next())?,
            minor: num(parts.next())?,
            patch: num(parts.next())?,
            pre,
        };
        parts.next().is_none().then_some(v)
    }

    pub fn is_stable(&self) -> bool {
        self.pre.is_empty()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => cmp_pre(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// SemVer §11: numeric identifiers compare as numbers and sort before text ones.
fn cmp_pre(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(m), Ok(n)) => m.cmp(&n),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Newer stable release with a file for this platform.
    Available {
        version: Version,
        download_url: String,
        page_url: String,
    },
    /// Newer stable release, but no file for this platform.
    NoBuild {
        version: Version,
        page_url: String,
    },
    UpToDate {
        latest: Option<Version>,
    },
}

/// Pick the newest stable release from the API's JSON and compare it with `current`.
pub fn evaluate(json: &str, current: &Version, os: &str, arch: &str) -> Result<Outcome, String> {
    let releases: Vec<ApiRelease> =
        serde_json::from_str(json).map_err(|e| format!("unexpected response from GitHub: {e}"))?;
    let newest = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, _)| v.is_stable())
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((version, release)) = newest else {
        return Ok(Outcome::UpToDate { latest: None });
    };
    if version <= *current {
        return Ok(Outcome::UpToDate {
            latest: Some(version),
        });
    }
    let page_url = if release.html_url.is_empty() {
        format!(
            "https://github.com/{REPO}/releases/tag/{}",
            release.tag_name
        )
    } else {
        release.html_url
    };
    let wanted = asset_names(os, arch);
    let asset = wanted
        .iter()
        .find_map(|name| release.assets.iter().find(|a| a.name == *name));
    Ok(match asset {
        Some(a) => Outcome::Available {
            version,
            download_url: a.browser_download_url.clone(),
            page_url,
        },
        None => Outcome::NoBuild { version, page_url },
    })
}

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is SemVer")
}

/// The endpoint; `KERN_UPDATE_URL` replaces it for testing (a file:// fixture works).
fn releases_url() -> String {
    std::env::var("KERN_UPDATE_URL").unwrap_or_else(|_| RELEASES_API.to_string())
}

fn curl() -> Command {
    // The system curl: /usr/bin/curl on macOS, System32\curl.exe on Windows 10/11.
    #[cfg(windows)]
    let program = std::env::var_os("SystemRoot")
        .map(|r| {
            std::path::PathBuf::from(r)
                .join("System32")
                .join("curl.exe")
        })
        .filter(|p| p.is_file())
        .unwrap_or_else(|| "curl.exe".into());
    #[cfg(not(windows))]
    let program = if std::path::Path::new("/usr/bin/curl").is_file() {
        std::path::PathBuf::from("/usr/bin/curl")
    } else {
        std::path::PathBuf::from("curl")
    };
    let mut cmd = Command::new(program);
    crate::platform::no_console(&mut cmd);
    cmd
}

/// Fetch the release list. Blocking: call it off the UI thread.
pub fn fetch(timeout: Duration) -> Result<String, String> {
    let agent = format!("KERN/{}", env!("CARGO_PKG_VERSION"));
    let out = curl()
        .args(["-sS", "-L", "--fail", "--proto", "=https,file"])
        .args(["--max-time", &timeout.as_secs().max(1).to_string()])
        .args(["-H", "Accept: application/vnd.github+json"])
        .args(["-A", &agent])
        .arg(releases_url())
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim().trim_start_matches("curl: ").trim();
        return Err(if err.is_empty() {
            format!("curl exited with {}", out.status)
        } else {
            err.chars().take(200).collect()
        });
    }
    String::from_utf8(out.stdout).map_err(|_| "response is not UTF-8".into())
}

/// Fetch and evaluate for the running build.
pub fn check(timeout: Duration) -> Result<Outcome, String> {
    let json = fetch(timeout)?;
    evaluate(
        &json,
        &current_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn semver_order() {
        assert!(v("v0.10.0") > v("v0.9.0"));
        assert!(v("0.9.10") > v("0.9.9"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha.1"));
        assert_eq!(v("V2.0.0+build.5"), v("2.0.0"));
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("01x.2.3").is_none());
        assert!(Version::parse("nightly").is_none());
    }

    fn release(tag: &str, draft: bool, pre: bool, assets: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag, "draft": draft, "prerelease": pre,
            "html_url": format!("https://github.com/meytyy/kern/releases/tag/{tag}"),
            "assets": assets.iter().map(|a| serde_json::json!({
                "name": a,
                "browser_download_url": format!("https://github.com/meytyy/kern/releases/download/{tag}/{a}"),
            })).collect::<Vec<_>>(),
        })
    }

    const BOTH: &[&str] = &["KERN-macos-arm64.zip", "KERN-windows-x64-setup.exe"];

    #[test]
    fn newer_stable_release_with_platform_file() {
        let json = serde_json::json!([
            release("v0.9.0", false, false, BOTH),
            release("v0.10.0", false, false, BOTH),
            release("v0.11.0-beta.1", false, true, BOTH),
            release("v0.12.0", true, false, BOTH),
            release("v0.13.0-rc.1", false, false, BOTH),
        ])
        .to_string();
        let mac = evaluate(&json, &v("0.9.0"), "macos", "aarch64").unwrap();
        assert_eq!(
            mac,
            Outcome::Available {
                version: v("0.10.0"),
                download_url:
                    "https://github.com/meytyy/kern/releases/download/v0.10.0/KERN-macos-arm64.zip"
                        .into(),
                page_url: "https://github.com/meytyy/kern/releases/tag/v0.10.0".into(),
            }
        );
        let Outcome::Available { download_url, .. } =
            evaluate(&json, &v("0.9.0"), "windows", "x86_64").unwrap()
        else {
            panic!()
        };
        assert!(download_url.ends_with("/v0.10.0/KERN-windows-x64-setup.exe"));
    }

    #[test]
    fn up_to_date_and_ahead() {
        let json = serde_json::json!([release("v0.1.0", false, false, BOTH)]).to_string();
        let latest = Some(v("0.1.0"));
        assert_eq!(
            evaluate(&json, &v("0.1.0"), "macos", "aarch64").unwrap(),
            Outcome::UpToDate {
                latest: latest.clone()
            }
        );
        assert_eq!(
            evaluate(&json, &v("0.2.0"), "macos", "aarch64").unwrap(),
            Outcome::UpToDate { latest }
        );
        assert_eq!(
            evaluate("[]", &v("0.1.0"), "macos", "aarch64").unwrap(),
            Outcome::UpToDate { latest: None }
        );
    }

    #[test]
    fn missing_platform_file() {
        let json = serde_json::json!([release("v0.2.0", false, false, &["KERN-macos-arm64.zip"])])
            .to_string();
        assert!(matches!(
            evaluate(&json, &v("0.1.0"), "windows", "x86_64").unwrap(),
            Outcome::NoBuild { .. }
        ));
        assert!(matches!(
            evaluate(&json, &v("0.1.0"), "macos", "x86_64").unwrap(),
            Outcome::NoBuild { .. }
        ));
        // The portable zip is accepted when the installer is missing.
        let json = serde_json::json!([release("v0.2.0", false, false, &["KERN-windows-x64.zip"])])
            .to_string();
        assert!(matches!(
            evaluate(&json, &v("0.1.0"), "windows", "x86_64").unwrap(),
            Outcome::Available { .. }
        ));
    }

    #[test]
    fn bad_response_is_an_error() {
        assert!(
            evaluate(
                "{\"message\":\"API rate limit exceeded\"}",
                &v("0.1.0"),
                "macos",
                "aarch64"
            )
            .is_err()
        );
        assert!(evaluate("<html>", &v("0.1.0"), "macos", "aarch64").is_err());
    }

    /// The app, the Windows installer and the exe's version resource carry one
    /// version, which is also the release tag (vX.Y.Z).
    #[test]
    fn version_agrees_everywhere() {
        let v = env!("CARGO_PKG_VERSION");
        let iss = include_str!("../../installer/KERN.iss");
        assert!(
            iss.contains(&format!("#define AppVersion \"{v}\"")),
            "installer/KERN.iss"
        );
        let rc = include_str!("../../assets/windows/kern.rc");
        let commas = format!("{},0", v.replace('.', ","));
        assert!(
            rc.contains(&format!("FILEVERSION {commas}")),
            "kern.rc FILEVERSION"
        );
        assert!(
            rc.contains(&format!("PRODUCTVERSION {commas}")),
            "kern.rc PRODUCTVERSION"
        );
        assert!(
            rc.contains(&format!("\"ProductVersion\", \"{v}\"")),
            "kern.rc ProductVersion"
        );
        let readme = include_str!("../../README.md");
        for file in ["KERN-windows-x64-setup.exe", "KERN-macos-arm64.zip"] {
            let link = format!("https://github.com/meytyy/kern/releases/download/v{v}/{file}");
            assert!(readme.contains(&link), "README link {link}");
        }
        // The README links are the files the update check looks for.
        assert_eq!(
            asset_names("windows", "x86_64")[0],
            "KERN-windows-x64-setup.exe"
        );
        assert_eq!(asset_names("macos", "aarch64")[0], "KERN-macos-arm64.zip");
    }

    /// After a release: `curl` the GitHub release list into a file and run
    ///   KERN_LIVE_RELEASES=releases.json KERN_LIVE_PREVIOUS=0.1.1 cargo test live_release
    /// It checks that the previous version is offered this version's file for
    /// each platform. Without the variable it does nothing.
    #[test]
    fn live_release_is_offered_to_the_previous_version() {
        let Ok(path) = std::env::var("KERN_LIVE_RELEASES") else {
            return;
        };
        let json = std::fs::read_to_string(path).unwrap();
        let previous = v(&std::env::var("KERN_LIVE_PREVIOUS").unwrap());
        let now = env!("CARGO_PKG_VERSION");
        for (os, arch, file) in [
            ("windows", "x86_64", "KERN-windows-x64-setup.exe"),
            ("macos", "aarch64", "KERN-macos-arm64.zip"),
        ] {
            match evaluate(&json, &previous, os, arch).unwrap() {
                Outcome::Available {
                    version,
                    download_url,
                    ..
                } => {
                    assert_eq!(version, v(now), "{os}");
                    assert_eq!(
                        download_url,
                        format!("https://github.com/meytyy/kern/releases/download/v{now}/{file}")
                    );
                    println!("{os} {arch}: v{version} -> {download_url}");
                }
                other => panic!("{os}: {other:?}"),
            }
        }
    }
}
