//! Reading an AppImage's identity from the file alone: nothing here executes it.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Type1,
    Type2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub kind: Kind,
    /// Where the runtime ends and the filesystem image starts (end of the ELF section table).
    /// ELF `e_machine`: the processor the runtime (and so the app) is built for.
    pub machine: u16,
    pub fs_offset: u64,
    /// Raw `.upd_info` section contents (type 2), trimmed.
    pub update_info: Option<String>,
    /// The runtime is dynamically linked against FUSE 2 and will not start without it.
    pub runtime_needs_fuse2: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    TooSmall,
    NotElf,
    NotAppImage,
    Unsupported(&'static str),
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Invalid::TooSmall => f.write_str("is too small to be an AppImage"),
            Invalid::NotElf => {
                f.write_str("is not an executable (no ELF header); AppImages are ELF files")
            }
            Invalid::NotAppImage => {
                f.write_str("is an executable but not an AppImage (no AppImage marker)")
            }
            Invalid::Unsupported(why) => {
                write!(f, "is an AppImage this version cannot read ({why})")
            }
        }
    }
}

fn u16le(b: &[u8], o: usize) -> Option<u64> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as u64)
}
fn u32le(b: &[u8], o: usize) -> Option<u64> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as u64)
}
fn u64le(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// ELF section table geometry: (e_shoff, e_shentsize, e_shnum, e_shstrndx, is64).
fn sections(b: &[u8]) -> Option<(u64, u64, u64, u64, bool)> {
    let is64 = match *b.get(4)? {
        1 => false,
        2 => true,
        _ => return None,
    };
    if *b.get(5)? != 1 {
        return None; // big-endian AppImages do not exist in practice
    }
    Some(if is64 {
        (
            u64le(b, 0x28)?,
            u16le(b, 0x3A)?,
            u16le(b, 0x3C)?,
            u16le(b, 0x3E)?,
            true,
        )
    } else {
        (
            u32le(b, 0x20)?,
            u16le(b, 0x2E)?,
            u16le(b, 0x30)?,
            u16le(b, 0x32)?,
            false,
        )
    })
}

/// (offset, size) of a section by name, with every read bounds-checked against `file`.
fn section(file: &[u8], name: &str) -> Option<(u64, u64)> {
    let (shoff, shentsize, shnum, shstrndx, is64) = sections(file)?;
    let entry = |i: u64| -> Option<(u64, u64, u64)> {
        let base = shoff.checked_add(i.checked_mul(shentsize)?)? as usize;
        let name_off = u32le(file, base)?;
        let (off, size) = if is64 {
            (u64le(file, base + 0x18)?, u64le(file, base + 0x20)?)
        } else {
            (u32le(file, base + 0x10)?, u32le(file, base + 0x14)?)
        };
        Some((name_off, off, size))
    };
    let (_, stroff, strsize) = entry(shstrndx)?;
    let strtab = file.get(stroff as usize..stroff.checked_add(strsize)? as usize)?;
    for i in 0..shnum {
        let (n, off, size) = entry(i)?;
        let s = strtab.get(n as usize..)?;
        let end = s.iter().position(|&c| c == 0)?;
        if &s[..end] == name.as_bytes() {
            return Some((off, size));
        }
    }
    None
}

/// Inspect the bytes of a file. `data` must contain at least the whole runtime (everything before
/// the filesystem image); reading the first few MiB of a file is enough.
pub fn inspect(data: &[u8]) -> Result<Info, Invalid> {
    if data.len() < 64 {
        return Err(Invalid::TooSmall);
    }
    if &data[..4] != b"\x7fELF" {
        return Err(Invalid::NotElf);
    }
    let kind = match data.get(8..11) {
        Some([0x41, 0x49, 0x02]) => Kind::Type2,
        Some([0x41, 0x49, 0x01]) => Kind::Type1,
        _ => return Err(Invalid::NotAppImage),
    };
    let (shoff, shentsize, shnum, _, _) =
        sections(data).ok_or(Invalid::Unsupported("unusual ELF layout"))?;
    let fs_offset = shnum
        .checked_mul(shentsize)
        .and_then(|t| t.checked_add(shoff))
        .ok_or(Invalid::Unsupported("corrupt section table"))?;
    let update_info = section(data, ".upd_info")
        .and_then(|(o, s)| data.get(o as usize..o.checked_add(s)? as usize))
        .map(|b| {
            String::from_utf8_lossy(b)
                .trim_matches(['\0', ' ', '\n'])
                .to_string()
        })
        .filter(|s| !s.is_empty());
    let runtime = &data[..(fs_offset as usize).min(data.len())];
    let runtime_needs_fuse2 = runtime.windows(12).any(|w| w == b"libfuse.so.2");
    Ok(Info {
        kind,
        machine: u16le(data, 0x12).unwrap_or(0) as u16,
        fs_offset,
        update_info,
        runtime_needs_fuse2,
    })
}

/// How many leading bytes `inspect` needs: the runtime plus its section table.
pub const HEAD_BYTES: usize = 8 * 1024 * 1024;

pub fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// Where updates come from, as far as the file itself says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubSource {
    pub repo: String,
    /// Glob for the release asset, e.g. `Foo-*-x86_64.AppImage`.
    pub pattern: String,
}

/// `gh-releases-zsync|user|repo|latest|Foo-*x86_64.AppImage.zsync`. Other transports are ignored.
pub fn github_source(update_info: &str) -> Option<GithubSource> {
    let mut f = update_info.split('|');
    if f.next()? != "gh-releases-zsync" {
        return None;
    }
    let (user, repo, tag, file) = (f.next()?, f.next()?, f.next()?, f.next()?);
    if !matches!(tag, "latest") {
        return None; // pinned tags, pre-release channels: not what a plain "update" means
    }
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && s != ".."
    };
    if !ok(user) || !ok(repo) {
        return None;
    }
    let pattern = file.strip_suffix(".zsync").unwrap_or(file).to_string();
    Some(GithubSource {
        repo: format!("{user}/{repo}"),
        pattern,
    })
}

/// Shell-style `*` glob, case-insensitive. Only `*` is special.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.to_lowercase(), text.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !t.starts_with(first) || !t.ends_with(last) || t.len() < first.len() + last.len() {
        return false;
    }
    let mut pos = first.len();
    let end = t.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match t[pos..end].find(mid) {
            Some(i) => pos += i + mid.len(),
            None => return false,
        }
    }
    true
}

/// Best-effort application name and version from a file name such as
/// `Obsidian-1.8.9-x86_64.AppImage` -> ("Obsidian", Some("1.8.9")).
pub fn name_and_version(file_name: &str) -> (String, Option<String>) {
    let stem = file_name
        .strip_suffix(".AppImage")
        .or_else(|| file_name.strip_suffix(".appimage"))
        .unwrap_or(file_name);
    let arch = [
        "x86_64", "amd64", "x64", "aarch64", "arm64", "armhf", "i386", "i686", "ia32", "linux",
    ];
    // `x86_64` contains the separator we split on, so neutralise it first.
    let stem = stem
        .replace("x86_64", "-")
        .replace("X86_64", "-")
        .replace("x86-64", "-");
    let tokens: Vec<&str> = stem
        .split(['-', '_', ' '])
        .filter(|t| !t.is_empty())
        .collect();
    let is_version = |t: &str| {
        let t = t.trim_start_matches(['v', 'V']);
        t.chars().next().is_some_and(|c| c.is_ascii_digit())
            && t.contains(|c: char| c == '.' || c.is_ascii_digit())
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '+')
    };
    let mut name_parts = Vec::new();
    let mut version = None;
    for t in tokens {
        if arch.contains(&t.to_lowercase().as_str()) {
            continue;
        }
        if version.is_none() && !name_parts.is_empty() && is_version(t) {
            version = Some(t.trim_start_matches(['v', 'V']).to_string());
            continue;
        }
        if version.is_none() {
            name_parts.push(t);
        }
    }
    let name = if name_parts.is_empty() {
        stem.to_string()
    } else {
        name_parts.join(" ")
    };
    (name, version)
}

/// A short, file-name-safe identity: lowercase letters, digits, single dashes.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() { "app".into() } else { s }
}

/// Builds fake AppImages for tests. Not part of the supported API.
#[doc(hidden)]
pub mod testutil {
    /// Build a minimal valid type-2 AppImage (ELF64 + `.upd_info` + padding "filesystem").
    pub fn fake(update_info: Option<&str>, needs_fuse2: bool) -> Vec<u8> {
        let shstr = b"\0.shstrtab\0.upd_info\0";
        let upd = update_info.map(|s| s.as_bytes().to_vec());
        let mut body: Vec<u8> = Vec::new();
        let ehsize = 64usize;
        let mut runtime = vec![0u8; 16];
        if needs_fuse2 {
            runtime.extend_from_slice(b"libfuse.so.2\0");
        }
        let str_off = ehsize + runtime.len();
        body.extend_from_slice(&runtime);
        body.extend_from_slice(shstr);
        let upd_off = ehsize + body.len();
        if let Some(u) = &upd {
            body.extend_from_slice(u);
        }
        let shoff = ehsize + body.len();
        let shnum = if upd.is_some() { 3 } else { 2 };
        let mut h = vec![0u8; ehsize];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[5] = 1;
        h[8..11].copy_from_slice(&[0x41, 0x49, 0x02]);
        h[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        h[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
        h[0x3C..0x3E].copy_from_slice(&(shnum as u16).to_le_bytes());
        h[0x3E..0x40].copy_from_slice(&1u16.to_le_bytes());
        let sh = |name: u32, off: usize, size: usize| {
            let mut e = vec![0u8; 64];
            e[0..4].copy_from_slice(&name.to_le_bytes());
            e[0x18..0x20].copy_from_slice(&(off as u64).to_le_bytes());
            e[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
            e
        };
        let mut table = sh(0, 0, 0);
        table.extend(sh(1, str_off, shstr.len()));
        if let Some(u) = &upd {
            table.extend(sh(11, upd_off, u.len()));
        }
        let mut out = h;
        out.extend(body);
        out.extend(table);
        out.extend_from_slice(b"hsqs-filesystem-image");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::fake;
    use super::*;

    #[test]
    fn reads_a_valid_type2_image_without_executing_it() {
        let f = fake(
            Some("gh-releases-zsync|o|r|latest|App-*x86_64.AppImage.zsync"),
            false,
        );
        let i = inspect(&f).unwrap();
        assert_eq!(i.kind, Kind::Type2);
        assert_eq!(
            i.update_info.as_deref(),
            Some("gh-releases-zsync|o|r|latest|App-*x86_64.AppImage.zsync")
        );
        assert!(!i.runtime_needs_fuse2);
        assert_eq!(
            &f[i.fs_offset as usize..],
            b"hsqs-filesystem-image",
            "offset points at the filesystem"
        );
    }

    #[test]
    fn detects_a_runtime_that_needs_fuse2() {
        assert!(inspect(&fake(None, true)).unwrap().runtime_needs_fuse2);
        assert!(!inspect(&fake(None, false)).unwrap().runtime_needs_fuse2);
        assert_eq!(inspect(&fake(None, false)).unwrap().update_info, None);
    }

    #[test]
    fn rejects_things_that_are_not_appimages() {
        assert_eq!(inspect(b"hi"), Err(Invalid::TooSmall));
        assert_eq!(inspect(&[b'x'; 200]), Err(Invalid::NotElf));
        let mut plain_elf = fake(None, false);
        plain_elf[8..11].copy_from_slice(&[0, 0, 0]);
        assert_eq!(inspect(&plain_elf), Err(Invalid::NotAppImage));
    }

    #[test]
    fn hostile_headers_never_panic() {
        let good = fake(Some("x"), false);
        for cut in [64, 70, 100, good.len() - 30] {
            let _ = inspect(&good[..cut.min(good.len())]);
        }
        let mut evil = good.clone();
        evil[0x28..0x30].copy_from_slice(&u64::MAX.to_le_bytes()); // e_shoff at the end of the world
        let _ = inspect(&evil);
        let mut evil = good.clone();
        evil[0x3C..0x3E].copy_from_slice(&u16::MAX.to_le_bytes());
        evil[0x3A..0x3C].copy_from_slice(&u16::MAX.to_le_bytes());
        let _ = inspect(&evil);
        let mut evil = good;
        evil[0x3E..0x40].copy_from_slice(&999u16.to_le_bytes()); // string table index out of range
        assert!(inspect(&evil).unwrap().update_info.is_none());
    }

    #[test]
    fn github_update_info() {
        let s = github_source(
            "gh-releases-zsync|probono|Subsurface|latest|Subsurface-*x86_64.AppImage.zsync",
        )
        .unwrap();
        assert_eq!(
            s,
            GithubSource {
                repo: "probono/Subsurface".into(),
                pattern: "Subsurface-*x86_64.AppImage".into()
            }
        );
        assert_eq!(github_source("zsync|https://x/y.zsync"), None);
        assert_eq!(github_source("gh-releases-zsync|a|b|v1.0|f.zsync"), None);
        assert_eq!(
            github_source("gh-releases-zsync|../x|b|latest|f.zsync"),
            None
        );
        assert_eq!(github_source("gh-releases-zsync|a|b"), None);
    }

    #[test]
    fn globbing() {
        assert!(glob("App-*-x86_64.AppImage", "App-1.2.3-x86_64.AppImage"));
        assert!(glob("app-*", "APP-1.AppImage"));
        assert!(!glob("App-*-x86_64.AppImage", "App-1.2.3-aarch64.AppImage"));
        assert!(glob("*", "anything"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXcYYb"));
        assert!(glob("exact", "EXACT"));
        assert!(!glob("ab*ba", "aba"));
    }

    #[test]
    fn names_and_versions_from_file_names() {
        let n = |s: &str| name_and_version(s);
        assert_eq!(
            n("Obsidian-1.8.9.AppImage"),
            ("Obsidian".into(), Some("1.8.9".into()))
        );
        assert_eq!(
            n("Kdenlive-25.08.1-x86_64.AppImage"),
            ("Kdenlive".into(), Some("25.08.1".into()))
        );
        assert_eq!(
            n("My_Cool_App-v2.0.0-linux-x86_64.AppImage"),
            ("My Cool App".into(), Some("2.0.0".into()))
        );
        assert_eq!(
            n("appimagetool-x86_64.AppImage"),
            ("appimagetool".into(), None)
        );
        assert_eq!(
            n("7zip-24.09.AppImage"),
            ("7zip".into(), Some("24.09".into()))
        );
        assert_eq!(n("weird.AppImage"), ("weird".into(), None));
        assert_eq!(n("Bitwarden-2025.1.0-x86_64.AppImage").0, "Bitwarden");
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(slugify("Visual Studio Code"), "visual-studio-code");
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert_eq!(slugify("  --  "), "app");
        assert_eq!(slugify("Ünïcode & Co."), "n-code-co");
        assert!(
            slugify("a/b\\c\0d")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
    }
}
