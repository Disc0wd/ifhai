//! The AppImageHub catalogue: a list of AppImage projects on GitHub, used to find an app by name.
//! It is cached on disk and refreshed automatically. The feed carries no download links or
//! versions, so every candidate is checked against GitHub Releases before it is offered.

use crate::error::{Error, Result};
use crate::fetch::Fetcher;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FEED_URL: &str = "https://appimage.github.io/feed.json";
/// A week: the catalogue changes slowly and a stale copy is better than none.
pub const MAX_AGE_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub description: String,
    /// `owner/repo` on GitHub.
    pub repo: String,
}

/// Bump when the reduced entries change shape or cleaning, so old caches are refetched.
const CACHE_VERSION: u32 = 3;

#[derive(Serialize, Deserialize)]
struct Cache {
    v: u32,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Feed {
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    name: Option<String>,
    description: Option<String>,
    links: Option<Vec<Link>>,
}

#[derive(Deserialize)]
struct Link {
    #[serde(rename = "type")]
    kind: Option<String>,
    url: Option<String>,
}

/// Some descriptions carry HTML (`<p>...</p>`); show plain text.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let words = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // This text is shown on the user's terminal: no escape sequences.
    words.chars().filter(|c| !c.is_control()).collect()
}

fn repo_ok(s: &str) -> bool {
    let mut p = s.split('/');
    let part = |x: &str| {
        !x.is_empty()
            && x.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && x != ".."
    };
    matches!((p.next(), p.next(), p.next()), (Some(a), Some(b), None) if part(a) && part(b))
}

/// Keep entries that have a name and a GitHub repo; skip the rest. Unknown fields are ignored.
pub fn parse_feed(json: &str) -> Result<Vec<Entry>> {
    let feed: Feed = serde_json::from_str(json).map_err(|e| {
        Error::Network(format!(
            "The AppImage catalogue was not in the expected format: {e}"
        ))
    })?;
    let mut out = Vec::new();
    for it in feed.items {
        let (Some(name), Some(links)) = (it.name, it.links) else {
            continue;
        };
        let repo = links
            .iter()
            .find(|l| l.kind.as_deref() == Some("GitHub"))
            .and_then(|l| l.url.clone());
        if let Some(repo) = repo.filter(|r| repo_ok(r)) {
            let name = plain(&name);
            if !name.is_empty() {
                out.push(Entry {
                    name,
                    description: plain(&it.description.unwrap_or_default()),
                    repo,
                });
            }
        }
    }
    Ok(out)
}

fn age_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()
        .map(|d| d.as_secs())
}

/// Entries plus an optional warning for the user. A fresh cache is used as is; otherwise the feed is
/// fetched; if that fails a stale cache is used (with a warning); with neither, an error.
pub fn load(fetcher: &dyn Fetcher, cache_dir: &Path) -> Result<(Vec<Entry>, Option<String>)> {
    let file = cache_dir.join("catalogue.json");
    let cached: Option<Vec<Entry>> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Cache>(&t).ok())
        .filter(|c| c.v == CACHE_VERSION)
        .map(|c| c.entries);
    if let Some(c) = &cached
        && age_secs(&file).is_some_and(|a| a < MAX_AGE_SECS)
    {
        return Ok((c.clone(), None));
    }
    let fetched = fetcher.text(FEED_URL).and_then(|t| parse_feed(&t));
    match fetched {
        Ok(entries) => {
            if std::fs::create_dir_all(cache_dir).is_ok()
                && let Ok(text) = serde_json::to_string(&Cache {
                    v: CACHE_VERSION,
                    entries: entries.clone(),
                })
            {
                let tmp = cache_dir.join(format!(".catalogue.{}.tmp", std::process::id()));
                if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &file).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
            Ok((entries, None))
        }
        Err(e) => match cached {
            Some(c) => Ok((
                c,
                Some(format!(
                    "Could not refresh the app catalogue ({}); using the copy from last time.",
                    e.to_string().lines().next().unwrap_or("")
                )),
            )),
            None => Err(Error::Network(format!(
                "Could not load the app catalogue, so apps cannot be found by name.\n{e}\n\nYou can still install a file, an owner/repo or a link."
            ))),
        },
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Score 0..=100; 0 means no match. Exact name or repo name 100, prefix 70, all words in the name
/// 60, substring 45, all words in the description 20.
pub fn score(e: &Entry, query: &str) -> u32 {
    let q = norm(query);
    if q.is_empty() {
        return 0;
    }
    let name = norm(&e.name);
    let repo = norm(e.repo.split('/').nth(1).unwrap_or(""));
    if name == q || repo == q || name.replace(' ', "") == q.replace(' ', "") {
        return 100;
    }
    if name.starts_with(&q) {
        return 70;
    }
    let words: Vec<&str> = q.split(' ').collect();
    let name_words: Vec<&str> = name.split(' ').collect();
    if words.iter().all(|w| name_words.contains(w)) {
        return 60;
    }
    if name.contains(&q) {
        return 45;
    }
    if words
        .iter()
        .all(|w| norm(&e.description).split(' ').any(|d| d == *w))
    {
        return 20;
    }
    0
}

/// Matches best first; ties break on name then repo, so results are deterministic.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<(u32, &'a Entry)> {
    let mut v: Vec<(u32, &Entry)> = entries
        .iter()
        .map(|e| (score(e, query), e))
        .filter(|(s, _)| *s > 0)
        .collect();
    v.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
            .then_with(|| a.1.repo.cmp(&b.1.repo))
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_text_cannot_carry_terminal_escapes() {
        let json = r#"{"items":[{"name":"Ev\u001b[2Jil","description":"a\u0007b <p>c</p>",
            "links":[{"type":"GitHub","url":"o/r"}]}]}"#;
        let e = &parse_feed(json).unwrap()[0];
        assert_eq!(
            (e.name.as_str(), e.description.as_str()),
            ("Ev[2Jil", "ab c")
        );
    }

    fn e(name: &str, repo: &str, desc: &str) -> Entry {
        Entry {
            name: name.into(),
            description: desc.into(),
            repo: repo.into(),
        }
    }

    #[test]
    fn feed_parsing_is_defensive() {
        let json = r#"{"version":"x","items":[
            {"name":"Good","description":"d","links":[{"type":"GitHub","url":"o/good"},{"type":"Download","url":"https://x"}],"extra":1},
            {"name":"NoLinks","description":null,"links":null},
            {"name":"NoGithub","links":[{"type":"Download","url":"https://x"}]},
            {"name":"Bad repo","links":[{"type":"GitHub","url":"../evil"}]},
            {"links":[{"type":"GitHub","url":"o/noname"}]},
            {"name":"  ","links":[{"type":"GitHub","url":"o/blank"}]},
            {"name":"NullDesc","description":null,"links":[{"type":"GitHub","url":"o/nulldesc"}]}
        ]}"#;
        let v = parse_feed(json).unwrap();
        assert_eq!(
            v,
            vec![e("Good", "o/good", "d"), e("NullDesc", "o/nulldesc", "")]
        );
        assert!(parse_feed("<html>captive portal</html>").is_err());
        assert!(parse_feed(r#"{"nope":1}"#).is_err());
    }

    #[test]
    fn html_in_descriptions_is_stripped() {
        let json = r#"{"items":[{"name":"A","description":"<p>A small <b>Apple</b> Notes\n for Linux</p>","links":[{"type":"GitHub","url":"o/a"}]}]}"#;
        assert_eq!(
            parse_feed(json).unwrap()[0].description,
            "A small Apple Notes for Linux"
        );
        assert_eq!(
            plain("a < b and c > d"),
            "a d",
            "an unclosed tag swallows the rest, never panics"
        );
    }

    #[test]
    fn ranking() {
        let es = vec![
            e("Krita", "KDE/krita", "Digital painting"),
            e("Krita Next", "KDE/krita-next", "Nightly"),
            e("Pinta", "o/pinta", "Image editor like Krita"),
            e("Obsidian", "obsidianmd/obsidian-releases", "Notes"),
            e("OBS Studio", "obsproject/obs-studio", "Streaming"),
        ];
        let r: Vec<&str> = search(&es, "krita")
            .iter()
            .map(|(_, e)| e.name.as_str())
            .collect();
        assert_eq!(
            r,
            vec!["Krita", "Krita Next", "Pinta"],
            "exact, then prefix, then description"
        );
        assert_eq!(search(&es, "obsidian")[0].1.name, "Obsidian");
        assert_eq!(
            search(&es, "OBS-Studio")[0].1.name,
            "OBS Studio",
            "punctuation and case are ignored"
        );
        assert_eq!(search(&es, "obs studio")[0].0, 100);
        assert!(search(&es, "zzz").is_empty() && search(&es, "  ").is_empty());
        assert_eq!(search(&es, "krita"), search(&es, "krita"), "deterministic");
    }

    #[test]
    fn an_exact_repo_name_beats_a_fuzzy_name() {
        let es = [
            e("Super Cool Editor", "o/sce", ""),
            e("Editor", "someone/sce", ""),
        ];
        assert_eq!(score(&es[0], "sce"), 100);
    }
}
