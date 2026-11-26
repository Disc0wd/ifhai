//! Network access, all through `curl` (on practically every Linux system), so ifhai needs no TLS
//! stack and inherits the system certificates and proxy settings.

use crate::error::{Error, Result};
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Release descriptions, checksums and the catalogue (about 3 MB) fit well within this.
const TEXT_MAX: u64 = 32 << 20;
/// Larger than any real AppImage; a server that sends more is stopped and the file removed.
const FILE_MAX: u64 = 8 << 30;

pub trait Fetcher: Sync {
    /// GET a small text resource. 404 is `Error::NotFound`; anything else that fails is `Network`.
    fn text(&self, url: &str) -> Result<String>;
    /// Download to `dest`. Same error mapping; a partial file is removed on failure.
    fn file(&self, url: &str, dest: &Path) -> Result<()>;
    /// Newest release tag of a GitHub repo, or None if it has no releases. No API call involved.
    fn latest_tag(&self, repo: &str) -> Result<Option<String>>;
    /// Build `dest` from the previous version `seed` plus only the changed blocks, using `zsync`
    /// when it is installed. Any error means "do a normal download instead".
    fn delta(&self, _zsync_url: &str, _seed: &Path, _dest: &Path) -> Result<()> {
        Err(Error::NotFound("delta updates are not available".into()))
    }
}

pub struct Curl;

const BASE: &[&str] = &[
    "--show-error",
    "--location",
    "--proto",
    "=https",
    "--proto-redir",
    "=https",
    "--tlsv1.2",
    "--max-time",
    "1800",
];

const API: &str = "https://api.github.com/";

/// A tag we are willing to put into a URL and a file name.
pub fn valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 100
        && !t.starts_with(['-', '.'])
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

fn read_capped(mut r: impl Read, cap: u64, over: &AtomicBool) -> Vec<u8> {
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 8192]);
    loop {
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) if (buf.len() + n) as u64 > cap => {
                over.store(true, Ordering::Relaxed);
                break;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    buf
}

struct Ran {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    ok: bool,
    too_big: bool,
}

/// Wait for `child`, killing it as soon as it prints more than `text_cap` bytes or the `file` it is
/// writing passes `file_cap`. curl's own limit only works when the server announces a length.
fn supervise(mut child: Child, text_cap: u64, file: Option<&Path>, file_cap: u64) -> Ran {
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let over = AtomicBool::new(false);
    std::thread::scope(|s| {
        let ho = s.spawn(|| {
            out.map(|o| read_capped(o, text_cap, &over))
                .unwrap_or_default()
        });
        let he = s.spawn(|| {
            let mut v = Vec::new();
            if let Some(mut e) = err {
                let _ = (&mut e).take(64 * 1024).read_to_end(&mut v);
                let _ = std::io::copy(&mut e, &mut std::io::sink());
            }
            v
        });
        let grown = || file.is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > file_cap));
        let (mut ok, mut too_big) = (false, false);
        loop {
            match child.try_wait() {
                Ok(Some(st)) => {
                    ok = st.success();
                    break;
                }
                Ok(None) => {}
                Err(_) => break,
            }
            if over.load(Ordering::Relaxed) || grown() {
                let _ = child.kill();
                let _ = child.wait();
                too_big = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let stdout = ho.join().unwrap_or_default();
        Ran {
            too_big: too_big || over.load(Ordering::Relaxed) || grown(),
            ok,
            stdout,
            stderr: he.join().unwrap_or_default(),
        }
    })
}

/// The tag in the address `/releases/latest` redirected to. A renamed or transferred repository
/// redirects too, and a freed name can be claimed by someone else, so a different repository is
/// refused rather than followed.
fn tag_from(repo: &str, effective: &str) -> Result<Option<String>> {
    let Some((front, tag)) = effective.split_once("/releases/tag/") else {
        return Ok(None);
    };
    let landed = front.strip_prefix("https://github.com/").unwrap_or(front);
    if !landed.eq_ignore_ascii_case(repo) {
        return Err(Error::Verify(format!(
            "{repo} now redirects to a different repository ({}), so ifhai will not follow it.\nIf the project really moved, install it again from the new address.",
            landed.escape_debug()
        )));
    }
    Ok(Some(tag.trim().to_string()).filter(|t| valid_tag(t)))
}

impl Curl {
    fn run(&self, extra: &[&str], url: &str) -> Result<Vec<u8>> {
        self.run_with(extra, url, true, None)
    }

    /// `quiet = false` lets curl draw its progress bar on stderr (used for big downloads on a terminal).
    fn run_with(
        &self,
        extra: &[&str],
        url: &str,
        quiet: bool,
        file: Option<&Path>,
    ) -> Result<Vec<u8>> {
        let mut cmd = Command::new("curl");
        cmd.arg(if quiet { "--silent" } else { "--progress-bar" });
        cmd.args(BASE)
            .args(["--user-agent", concat!("ifhai/", env!("CARGO_PKG_VERSION"))])
            .args(extra);
        let api = url.starts_with(API);
        if api {
            cmd.args(["--header", "Accept: application/vnd.github+json"]);
        }
        // The token goes through curl's stdin config, never the argument list (`ps` would show it).
        let token = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|t| api && !t.is_empty() && !t.contains(['"', '\n']));
        if token.is_some() {
            cmd.args(["--config", "-"]);
        }
        cmd.arg(url).stdin(if token.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(if quiet {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::Network(
                        "curl was not found. Install curl (ifhai uses it for downloads).".into(),
                    )
                } else {
                    Error::Network(format!("could not run curl: {e}"))
                }
            })?;
        if let (Some(t), Some(mut stdin)) = (token, child.stdin.take()) {
            let _ = writeln!(stdin, "header = \"Authorization: Bearer {t}\"");
        }
        let host = url.split('/').nth(2).unwrap_or(url);
        // A status code comes back on stdout for downloads, a whole body for text.
        let cap = if file.is_some() { 64 * 1024 } else { TEXT_MAX };
        let ran = supervise(child, cap, file, FILE_MAX);
        if ran.too_big {
            return Err(Error::Network(format!(
                "The response from {host} is larger than ifhai accepts, so it was stopped."
            )));
        }
        if !ran.ok {
            return Err(Error::Network(format!(
                "Could not reach {host}.\n{}",
                String::from_utf8_lossy(&ran.stderr).trim()
            )));
        }
        Ok(ran.stdout)
    }

    fn status(code: &str, url: &str) -> Result<()> {
        match code.trim() {
            "200" => Ok(()),
            "404" => Err(Error::NotFound(format!("Not found: {url}"))),
            "403" | "429" if url.starts_with(API) => Err(Error::Network(
                "GitHub's rate limit was reached (60 requests an hour without a token).\nSet GITHUB_TOKEN to raise it, or try again later."
                    .into(),
            )),
            c => Err(Error::Network(format!("The server answered HTTP {c} for {url}"))),
        }
    }
}

impl Fetcher for Curl {
    fn text(&self, url: &str) -> Result<String> {
        let out = self.run(&["--write-out", "\n%{http_code}"], url)?;
        let s = String::from_utf8_lossy(&out).into_owned();
        let (body, code) = s.rsplit_once('\n').unwrap_or(("", &s));
        Self::status(code, url)?;
        Ok(body.to_string())
    }

    fn file(&self, url: &str, dest: &Path) -> Result<()> {
        let d = dest.to_string_lossy();
        // Show curl's progress bar when someone is watching: a 120 MB download must not look hung.
        let quiet = !std::io::stderr().is_terminal();
        let result = self
            .run_with(
                &["--write-out", "%{http_code}", "--output", &d],
                url,
                quiet,
                Some(dest),
            )
            .and_then(|out| Self::status(&String::from_utf8_lossy(&out), url));
        if result.is_err() {
            let _ = std::fs::remove_file(dest);
        }
        result
    }

    fn delta(&self, zsync_url: &str, seed: &Path, dest: &Path) -> Result<()> {
        if !zsync_url.starts_with("https://") {
            return Err(Error::NotFound(
                "delta updates need an https:// link".into(),
            ));
        }
        let ok = Command::new("zsync")
            .args(["-q", "-i"])
            .arg(seed)
            .arg("-o")
            .arg(dest)
            .arg(zsync_url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok && dest.exists() {
            Ok(())
        } else {
            let _ = std::fs::remove_file(dest);
            Err(Error::NotFound(
                "zsync is not installed or could not build the update".into(),
            ))
        }
    }

    fn latest_tag(&self, repo: &str) -> Result<Option<String>> {
        let url = format!("https://github.com/{repo}/releases/latest");
        let out = self.run(
            &[
                "--output",
                "/dev/null",
                "--write-out",
                "%{http_code} %{url_effective}",
            ],
            &url,
        )?;
        let s = String::from_utf8_lossy(&out).into_owned();
        let (code, effective) = s.split_once(' ').unwrap_or((&s, ""));
        if code == "404" {
            return Ok(None);
        }
        tag_from(repo, effective)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_to_another_repository_is_refused() {
        let at = |r: &str| format!("https://github.com/{r}/releases/tag/v1.0");
        assert_eq!(tag_from("o/r", &at("O/R")).unwrap(), Some("v1.0".into()));
        assert!(tag_from("o/r", &at("thief/r")).is_err());
        assert_eq!(
            tag_from("o/r", "https://github.com/o/r/releases").unwrap(),
            None
        );
    }

    #[test]
    fn oversized_responses_are_stopped() {
        let sh = |script: &str| {
            Command::new("sh")
                .args(["-c", script])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        };
        let r = supervise(sh("printf hi"), 1000, None, 1000);
        assert!(r.ok && !r.too_big && r.stdout == b"hi");
        let r = supervise(sh("head -c 200000 /dev/zero; sleep 30"), 1000, None, 1000);
        assert!(r.too_big);
        let f = std::env::temp_dir().join(format!("ifhai-cap-test-{}", std::process::id()));
        let script = format!("head -c 200000 /dev/zero > '{}'; sleep 30", f.display());
        let r = supervise(sh(&script), 1000, Some(&f), 1000);
        let _ = std::fs::remove_file(&f);
        assert!(r.too_big);
    }

    #[test]
    fn tags_that_are_safe_to_use_in_urls() {
        for ok in ["v0.1.0", "1.2.3", "v1.0.0-rc.1", "nightly_2026"] {
            assert!(valid_tag(ok), "{ok}");
        }
        for bad in ["", "-v1", ".hidden", "v1/../x", "v1 0", "v1;rm", "v1\n"] {
            assert!(!valid_tag(bad), "{bad:?}");
        }
    }
}
