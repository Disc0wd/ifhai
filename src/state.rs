use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `owner/repo` on GitHub.
    pub repo: String,
    /// Glob that matches this app's AppImage in every release.
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Previous {
    pub version: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub slug: String,
    pub version: String,
    /// The release tag the file came from, when it came from a release.
    pub tag: Option<String>,
    /// "fuse" (run the file) or "extract" (run the unpacked tree; needs no FUSE).
    pub mode: String,
    pub source: Option<Source>,
    /// SHA-256 of the installed file when it is known (a verified download); empty otherwise.
    pub sha256: String,
    pub electron: bool,
    /// GitHub repos named in the app's bundled metadata. Candidates only: one becomes the update
    /// source if and when its latest release is verified to contain this app's file.
    #[serde(default)]
    pub hints: Vec<String>,
    /// Already tried to turn the hints into an update source (each app costs at most one lookup).
    #[serde(default)]
    pub hints_tried: bool,
    /// The file's original name, e.g. `Foo-1.2.3-x86_64.AppImage`: the template for matching releases.
    #[serde(default)]
    pub origin_name: String,
    pub installed_at: u64,
    pub previous: Option<Previous>,
    // Everything ifhai created for this app. Removal deletes exactly this and nothing else.
    pub file: PathBuf,
    pub launcher: PathBuf,
    pub desktop: PathBuf,
    pub icon: Option<PathBuf>,
    pub support: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub apps: BTreeMap<String, App>,
}

impl State {
    /// Missing file = nothing installed. A damaged one is an error, never silently reset.
    pub fn load(path: &Path) -> Result<State> {
        match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| {
                Error::Config(format!(
                    "The install record {} is damaged:\n{e}\nFix or delete it; your installed apps are not touched.",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(Error::Io(format!("Cannot read {}: {e}", path.display()))),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".state.{}.tmp", std::process::id()));
        let text = toml::to_string_pretty(self).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(())
    }

    /// Find an installed app by slug, name (any case) or unambiguous slug prefix.
    pub fn find(&self, q: &str) -> Result<&App> {
        let ql = q.trim().to_lowercase();
        if let Some(a) = self.apps.get(&ql) {
            return Ok(a);
        }
        let by_name: Vec<&App> = self
            .apps
            .values()
            .filter(|a| a.name.to_lowercase() == ql)
            .collect();
        if let [one] = by_name.as_slice() {
            return Ok(one);
        }
        let by_prefix: Vec<&App> = self
            .apps
            .values()
            .filter(|a| a.slug.starts_with(&ql) || a.name.to_lowercase().starts_with(&ql))
            .collect();
        match by_prefix.as_slice() {
            [one] => Ok(one),
            [] => Err(Error::NotFound(format!(
                "'{q}' is not installed.\n\nSee what is installed:\n  ifhai list"
            ))),
            many => Err(Error::Ambiguous(format!(
                "'{q}' matches several apps:\n\n{}\n\nUse the full name.",
                many.iter()
                    .map(|a| format!("  {}", a.slug))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))),
        }
    }
}

/// Every directory ifhai writes to. All under the user's home; nothing needs root.
#[derive(Debug, Clone)]
pub struct Dirs {
    /// `~/Applications`: where the AppImages live, visible and browsable.
    pub apps: PathBuf,
    /// `~/Applications/.ifhai`: extracted trees, the previous version, temp files.
    pub support: PathBuf,
    pub bin: PathBuf,
    pub applications: PathBuf,
    pub icons: PathBuf,
    pub state_file: PathBuf,
    /// Disposable downloads (the catalogue).
    pub cache: PathBuf,
}

fn xdg(var: &str, fallback: &Path, home: &Path) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => PathBuf::from(v),
        _ => home.join(fallback),
    }
}

impl Dirs {
    pub fn from_home(home: &Path) -> Dirs {
        let data = xdg("XDG_DATA_HOME", Path::new(".local/share"), home);
        Dirs {
            apps: home.join("Applications"),
            support: home.join("Applications").join(".ifhai"),
            bin: home.join(".local").join("bin"),
            applications: data.join("applications"),
            icons: data.join("icons").join("hicolor"),
            state_file: data.join("ifhai").join("state.toml"),
            cache: xdg("XDG_CACHE_HOME", Path::new(".cache"), home).join("ifhai"),
        }
    }

    pub fn from_env() -> Result<Dirs> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::Config("HOME is not set; cannot decide where to install apps.".into())
            })?;
        Ok(Dirs::from_home(&home))
    }

    /// Only paths under these roots may be deleted on behalf of the state file, so a damaged or
    /// hand-edited record can never make `remove` delete something unrelated.
    pub fn may_delete(&self, p: &Path) -> bool {
        let under = |root: &Path| {
            p.starts_with(root) && p != root && !p.components().any(|c| c.as_os_str() == "..")
        };
        under(&self.apps) || under(&self.bin) || under(&self.applications) || under(&self.icons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn app(slug: &str, name: &str) -> App {
        App {
            name: name.into(),
            slug: slug.into(),
            version: "1.0".into(),
            tag: None,
            mode: "fuse".into(),
            source: None,
            sha256: "00".into(),
            electron: false,
            hints: vec![],
            hints_tried: false,
            origin_name: String::new(),
            installed_at: 1,
            previous: None,
            file: format!("/h/Applications/{slug}.AppImage").into(),
            launcher: format!("/h/.local/bin/{slug}").into(),
            desktop: format!("/h/.local/share/applications/ifhai-{slug}.desktop").into(),
            icon: None,
            support: format!("/h/Applications/.ifhai/{slug}").into(),
        }
    }

    #[test]
    fn round_trip_missing_and_damaged() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s/state.toml");
        assert!(State::load(&p).unwrap().apps.is_empty());
        let mut s = State::default();
        let mut a = app("obsidian", "Obsidian");
        a.source = Some(Source {
            repo: "o/r".into(),
            pattern: "O-*.AppImage".into(),
        });
        a.previous = Some(Previous {
            version: "0.9".into(),
            tag: Some("v0.9".into()),
        });
        s.apps.insert("obsidian".into(), a);
        s.save(&p).unwrap();
        assert_eq!(State::load(&p).unwrap().apps, s.apps);
        std::fs::write(&p, "apps = [[[").unwrap();
        let e = State::load(&p).unwrap_err().to_string();
        assert!(e.contains("damaged") && e.contains("state.toml"), "{e}");
    }

    #[test]
    fn finding_apps() {
        let mut s = State::default();
        for (slug, name) in [
            ("obsidian", "Obsidian"),
            ("kdenlive", "Kdenlive"),
            ("krita", "Krita"),
            ("krita-next", "Krita Next"),
        ] {
            s.apps.insert(slug.into(), app(slug, name));
        }
        assert_eq!(s.find("obsidian").unwrap().slug, "obsidian");
        assert_eq!(s.find("OBSIDIAN").unwrap().slug, "obsidian");
        assert_eq!(s.find("Krita").unwrap().slug, "krita", "exact beats prefix");
        assert_eq!(s.find("obs").unwrap().slug, "obsidian");
        assert!(matches!(s.find("k"), Err(Error::Ambiguous(_))));
        assert!(matches!(s.find("zzz"), Err(Error::NotFound(_))));
    }

    #[test]
    fn deletion_is_confined_to_ifhai_locations() {
        let d = Dirs::from_home(Path::new("/h"));
        assert!(d.may_delete(Path::new("/h/Applications/obsidian.AppImage")));
        assert!(d.may_delete(Path::new("/h/Applications/.ifhai/obsidian")));
        assert!(d.may_delete(Path::new("/h/.local/bin/obsidian")));
        for bad in [
            "/h/Documents/x",
            "/etc/passwd",
            "/h/Applications",
            "/h",
            "/h/Applications/../Documents/x",
            "/",
        ] {
            assert!(!d.may_delete(Path::new(bad)), "{bad}");
        }
    }
}
