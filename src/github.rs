//! GitHub release metadata and choosing the right AppImage asset.

use crate::appimage::glob;
use crate::error::{Error, Result};
use crate::fetch::Fetcher;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

pub fn parse_release(json: &str) -> Result<Release> {
    serde_json::from_str(json).map_err(|e| {
        Error::Network(format!(
            "GitHub sent a release description ifhai could not read: {e}"
        ))
    })
}

pub fn latest_release(fetcher: &dyn Fetcher, repo: &str) -> Result<Release> {
    match fetcher.text(&format!(
        "https://api.github.com/repos/{repo}/releases/latest"
    )) {
        Ok(j) => parse_release(&j),
        Err(Error::NotFound(_)) => Err(Error::NotFound(format!(
            "{repo} has no published releases (or does not exist)."
        ))),
        Err(e) => Err(e),
    }
}

const ARCHES: &[(&str, &[&str])] = &[
    ("x86_64", &["x86_64", "amd64", "x64"]),
    ("aarch64", &["aarch64", "arm64"]),
    ("arm", &["armhf", "armv7"]),
    ("x86", &["i386", "i686", "x86_32"]),
];

fn tokens(arch: &str) -> &'static [&'static str] {
    ARCHES
        .iter()
        .find(|(a, _)| *a == arch)
        .map(|(_, t)| *t)
        .unwrap_or(&[])
}

fn mentions(name: &str, toks: &[&str]) -> bool {
    let n = name.to_lowercase();
    toks.iter().any(|t| n.contains(t))
}

/// Pick the one AppImage for this machine. Several equally good candidates is an error that lists
/// them: guessing which build the user wants is how people end up with the wrong one.
pub fn select_asset<'a>(
    assets: &'a [Asset],
    arch: &str,
    pattern: Option<&str>,
) -> Result<&'a Asset> {
    let mut c: Vec<&Asset> = assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".appimage"))
        .collect();
    if let Some(p) = pattern {
        let narrowed: Vec<&Asset> = c.iter().copied().filter(|a| glob(p, &a.name)).collect();
        if !narrowed.is_empty() {
            c = narrowed;
        }
    }
    let host = tokens(arch);
    let any_arch: Vec<&str> = ARCHES.iter().flat_map(|(_, t)| t.iter().copied()).collect();
    let tagged: Vec<&Asset> = c
        .iter()
        .copied()
        .filter(|a| mentions(&a.name, host))
        .collect();
    let untagged: Vec<&Asset> = c
        .iter()
        .copied()
        .filter(|a| !mentions(&a.name, &any_arch))
        .collect();
    let pool = if !tagged.is_empty() { tagged } else { untagged };
    match pool.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::NotFound(format!(
            "This release has no AppImage for {arch}.{}",
            if c.is_empty() {
                String::new()
            } else {
                format!(
                    " It has: {}.",
                    c.iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ))),
        many => Err(Error::Ambiguous(format!(
            "This release has several AppImages that could be the right one:\n\n{}\n\nInstall the one you want by its download URL (right-click the file on the release page).",
            many.iter()
                .map(|a| format!("  {}", a.name))
                .collect::<Vec<_>>()
                .join("\n")
        ))),
    }
}

/// `https://github.com/o/r/releases/download/<tag>/<asset>.AppImage` -> (`o/r`, tag, asset).
/// Browsers record the download URL on the file; this turns it into an exact update source.
pub fn release_download_url(url: &str) -> Option<(String, String, String)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.split(['?', '#']).next()?;
    let p: Vec<&str> = rest.split('/').collect();
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && s != ".."
    };
    let [owner, repo, "releases", "download", tag, asset] = p.as_slice() else {
        return None;
    };
    let safe_asset = asset
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c));
    (ok(owner)
        && ok(repo)
        && crate::fetch::valid_tag(tag)
        && safe_asset
        && asset.to_lowercase().ends_with(".appimage"))
    .then(|| {
        (
            format!("{owner}/{repo}"),
            tag.to_string(),
            asset.to_string(),
        )
    })
}

/// `Foo-1.2.3-x86_64.AppImage` + tag `v1.2.3` -> `Foo-*-x86_64.AppImage`, so the same variant is
/// found in the next release.
pub fn pattern_from_asset(name: &str, tag: &str) -> String {
    let bare = tag.trim_start_matches(['v', 'V']);
    if bare.is_empty() || !name.contains(bare) {
        return name.to_string();
    }
    name.replacen(&format!("v{bare}"), "*", 1)
        .replacen(&format!("V{bare}"), "*", 1)
        .replacen(bare, "*", 1)
}

/// The SHA-256 GitHub or the publisher gives for `asset`, if any: the API `digest` first, then a
/// checksum file in the same release.
pub fn expected_sha256(
    fetcher: &dyn Fetcher,
    asset: &Asset,
    all: &[Asset],
) -> Result<Option<String>> {
    if let Some(h) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| is_sha256(h))
    {
        return Ok(Some(h.to_ascii_lowercase()));
    }
    let own = [
        format!("{}.sha256", asset.name),
        format!("{}.sha256sum", asset.name),
    ];
    let shared = |n: &str| {
        let l = n.to_lowercase();
        l.starts_with("sha256sum") || l.starts_with("checksums") || l == "sha256.txt"
    };
    let Some(side) = all
        .iter()
        .find(|a| own.contains(&a.name))
        .or_else(|| all.iter().find(|a| shared(&a.name)))
    else {
        return Ok(None);
    };
    let text = match fetcher.text(&side.browser_download_url) {
        Ok(t) => t,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(sum_for(&text, &asset.name))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// From `sha256sum`-style text: the digest on the line for `name`, or the only digest in a
/// one-line sidecar file.
pub fn sum_for(text: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    for l in &lines {
        let mut p = l.split_whitespace();
        if let (Some(h), Some(f)) = (p.next(), p.next())
            && is_sha256(h)
            && f.trim_start_matches('*') == name
        {
            return Some(h.to_ascii_lowercase());
        }
    }
    match lines.as_slice() {
        [one] => one
            .split_whitespace()
            .next()
            .filter(|h| is_sha256(h))
            .map(str::to_ascii_lowercase),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(name: &str) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: format!("https://github.com/o/r/releases/download/v1/{name}"),
            size: 1,
            digest: None,
        }
    }

    #[test]
    fn picks_the_appimage_for_this_architecture() {
        let assets = vec![
            a("App-1.0-x86_64.AppImage"),
            a("App-1.0-aarch64.AppImage"),
            a("App-1.0-x86_64.AppImage.zsync"),
            a("App-1.0.tar.gz"),
            a("SHA256SUMS"),
        ];
        assert_eq!(
            select_asset(&assets, "x86_64", None).unwrap().name,
            "App-1.0-x86_64.AppImage"
        );
        assert_eq!(
            select_asset(&assets, "aarch64", None).unwrap().name,
            "App-1.0-aarch64.AppImage"
        );
    }

    #[test]
    fn untagged_names_are_accepted_only_when_nothing_matches_the_arch() {
        assert_eq!(
            select_asset(&[a("Tool.AppImage")], "x86_64", None)
                .unwrap()
                .name,
            "Tool.AppImage"
        );
        let e = select_asset(&[a("Tool-aarch64.AppImage")], "x86_64", None).unwrap_err();
        assert!(
            matches!(e, Error::NotFound(_)) && e.to_string().contains("no AppImage for x86_64"),
            "{e}"
        );
        assert!(select_asset(&[a("notes.txt")], "x86_64", None).is_err());
    }

    #[test]
    fn several_candidates_are_never_guessed() {
        let assets = vec![a("App-x86_64.AppImage"), a("App-legacy-x86_64.AppImage")];
        let e = select_asset(&assets, "x86_64", None).unwrap_err();
        assert!(
            matches!(e, Error::Ambiguous(_)) && e.to_string().contains("legacy"),
            "{e}"
        );
        // ...but a remembered pattern resolves it.
        assert_eq!(
            select_asset(&assets, "x86_64", Some("App-legacy-*.AppImage"))
                .unwrap()
                .name,
            "App-legacy-x86_64.AppImage"
        );
    }

    #[test]
    fn pattern_memory_survives_version_changes() {
        assert_eq!(
            pattern_from_asset("Foo-1.2.3-x86_64.AppImage", "v1.2.3"),
            "Foo-*-x86_64.AppImage"
        );
        assert_eq!(
            pattern_from_asset("Foo-v1.2.3-x86_64.AppImage", "v1.2.3"),
            "Foo-*-x86_64.AppImage"
        );
        assert_eq!(
            pattern_from_asset("Foo-x86_64.AppImage", "v1.2.3"),
            "Foo-x86_64.AppImage"
        );
        let p = pattern_from_asset("Foo-1.2.3-x86_64.AppImage", "1.2.3");
        assert!(glob(&p, "Foo-9.9.9-x86_64.AppImage"));
    }

    #[test]
    fn checksum_text_formats() {
        let h = "a".repeat(64);
        let g = "b".repeat(64);
        assert_eq!(
            sum_for(
                &format!("{h}  other.AppImage\n{g} *App.AppImage\n"),
                "App.AppImage"
            ),
            Some(g.clone())
        );
        assert_eq!(
            sum_for(&format!("{h}  App.AppImage.sha256-target\n"), "x"),
            Some(h.clone()),
            "one-line sidecar"
        );
        assert_eq!(
            sum_for(&format!("{h}  a\n{g}  b\n"), "c"),
            None,
            "no guessing among several lines"
        );
        assert_eq!(sum_for("not a sum", "x"), None);
    }

    #[test]
    fn download_urls_become_exact_sources() {
        let u = "https://github.com/KDE/kdenlive/releases/download/v25.08.1/kdenlive-25.08.1-x86_64.AppImage?x=1";
        assert_eq!(
            release_download_url(u),
            Some((
                "KDE/kdenlive".into(),
                "v25.08.1".into(),
                "kdenlive-25.08.1-x86_64.AppImage".into()
            ))
        );
        for bad in [
            "http://github.com/a/b/releases/download/v1/x.AppImage",
            "https://example.org/a/b/releases/download/v1/x.AppImage",
            "https://github.com/a/b/releases/latest",
            "https://github.com/a/b/releases/download/v1/x.zip",
            "https://github.com/a/b/releases/download/v1/%2e%2e.AppImage",
            "https://github.com/../b/releases/download/v1/x.AppImage",
            "https://github.com/a/b/releases/download/-v1/x.AppImage",
        ] {
            assert_eq!(release_download_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn release_json_is_parsed_defensively() {
        let r = parse_release(r#"{"tag_name":"v1","assets":[{"name":"a.AppImage","browser_download_url":"https://x/a","size":5,"digest":"sha256:abc","extra":1}],"other":true}"#).unwrap();
        assert_eq!(r.tag_name, "v1");
        assert_eq!(r.assets[0].digest.as_deref(), Some("sha256:abc"));
        assert_eq!(
            parse_release(r#"{"tag_name":"v2"}"#).unwrap().assets.len(),
            0
        );
        assert!(parse_release("<html>").is_err());
    }
}
