//! Network access, all through `curl` (on practically every Linux system), so ifhai needs no TLS
//! stack and inherits the system certificates and proxy settings.

use crate::error::{Error, Result};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

pub trait Fetcher: Sync {
    /// GET a small text resource. 404 is `Error::NotFound`; anything else that fails is `Network`.
    fn text(&self, url: &str) -> Result<String>;
    /// Download to `dest`. Same error mapping; a partial file is removed on failure.
    fn file(&self, url: &str, dest: &Path) -> Result<()>;
    /// Newest release tag of a GitHub repo, or None if it has no releases. No API call involved.
    fn latest_tag(&self, repo: &str) -> Result<Option<String>>;
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

impl Curl {
    fn run(&self, extra: &[&str], url: &str) -> Result<Vec<u8>> {
        self.run_with(extra, url, true)
    }

    /// `quiet = false` lets curl draw its progress bar on stderr (used for big downloads on a terminal).
    fn run_with(&self, extra: &[&str], url: &str, quiet: bool) -> Result<Vec<u8>> {
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
        let out = child
            .wait_with_output()
            .map_err(|e| Error::Network(format!("curl failed: {e}")))?;
        if !out.status.success() {
            let host = url.split('/').nth(2).unwrap_or(url);
            return Err(Error::Network(format!(
                "Could not reach {host}.\n{}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(out.stdout)
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
            .run_with(&["--write-out", "%{http_code}", "--output", &d], url, quiet)
            .and_then(|out| Self::status(&String::from_utf8_lossy(&out), url));
        if result.is_err() {
            let _ = std::fs::remove_file(dest);
        }
        result
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
        Ok(effective
            .split("/releases/tag/")
            .nth(1)
            .map(|t| t.trim().to_string())
            .filter(|t| valid_tag(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::valid_tag;

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
