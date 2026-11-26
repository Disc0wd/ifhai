//! Make one AppImage file a real, working application. Everything is staged first and committed
//! last, with an undo path, so any failure leaves the previous state exactly as it was.

use crate::appimage::{self, Info, Kind};
use crate::desktop;
use crate::error::{Error, Result};
use crate::state::{App, Dirs, Previous, Source, State};
use crate::system::{Host, System};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Ctx<'a> {
    pub system: &'a dyn System,
    pub dirs: &'a Dirs,
    pub host: &'a Host,
}

#[derive(Default, Clone)]
pub struct Origin {
    pub source: Option<Source>,
    pub tag: Option<String>,
    pub version_hint: Option<String>,
    /// SHA-256 the caller already computed and verified, so the file is not read twice.
    pub known_sha256: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    Upgraded { from: String },
    Unchanged,
}

pub struct Done {
    pub app: App,
    pub outcome: Outcome,
    pub notes: Vec<String>,
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn temp_dir(support: &Path, tag: &str) -> PathBuf {
    support.join(format!(
        ".{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn chmod(p: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let tmp = path.with_extension(format!("ifhai-tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    chmod(&tmp, mode)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Rename, or copy-then-delete across filesystems (a download in /tmp, an app on another disk).
fn move_file(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(18) => {
            std::fs::copy(from, to)?;
            if sha256_file(from)? != sha256_file(to)? {
                let _ = std::fs::remove_file(to);
                return Err(Error::Io(
                    "The copy did not match the original, so nothing was moved.".into(),
                ));
            }
            std::fs::remove_file(from)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// A path inside the extracted tree, only if it really is inside (a hostile AppImage can ship
/// `.DirIcon -> /etc/passwd`).
fn inside(root: &Path, p: &Path) -> Option<PathBuf> {
    let (r, c) = (root.canonicalize().ok()?, p.canonicalize().ok()?);
    (c.starts_with(&r) && c.is_file()).then_some(c)
}

/// The app's menu file: normally at the top level, but some packages only ship it under `usr/`.
fn find_desktop(root: &Path) -> Option<PathBuf> {
    [root.to_path_buf(), root.join("usr/share/applications")]
        .iter()
        .find_map(|dir| {
            let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "desktop"))
                .collect();
            names.sort();
            names.into_iter().find_map(|p| inside(root, &p))
        })
}

fn png_width(p: &Path) -> Option<u32> {
    let mut b = [0u8; 24];
    std::fs::File::open(p).ok()?.read_exact(&mut b).ok()?;
    (&b[1..4] == b"PNG").then(|| u32::from_be_bytes([b[16], b[17], b[18], b[19]]))
}

/// Add `p` if it really is an image. Judged by content: packages ship empty files, broken links and
/// files with the wrong extension.
fn consider(root: &Path, p: PathBuf, found: &mut Vec<(u32, PathBuf, String)>) {
    let Some(real) = inside(root, &p) else { return };
    if let Some(width) = png_width(&real) {
        found.push((width, real, "png".into()));
    } else if is_svg(&real) {
        found.push((100_000, real, "svg".into()));
    }
}

fn is_svg(p: &Path) -> bool {
    let mut head = Vec::new();
    std::fs::File::open(p).is_ok_and(|f| f.take(64 * 1024).read_to_end(&mut head).is_ok())
        && head.windows(4).any(|w| w == b"<svg")
}

/// The best real image for the app, looking where packages put icons (properly or not).
fn find_icon(root: &Path, name: Option<&str>) -> Option<(PathBuf, String)> {
    let mut found: Vec<(u32, PathBuf, String)> = Vec::new();
    // `Icon=` should be a bare name; packages also write `app.png` or a path.
    let name = name.map(|n| {
        let file = n.rsplit('/').next().unwrap_or(n);
        [".png", ".svg", ".xpm"]
            .iter()
            .find_map(|e| file.strip_suffix(e))
            .unwrap_or(file)
    });
    if let Some(n) = name {
        let mut dirs = vec![root.to_path_buf(), root.join("usr/share/pixmaps")];
        if let Ok(sizes) = std::fs::read_dir(root.join("usr/share/icons/hicolor")) {
            dirs.extend(sizes.flatten().map(|s| s.path().join("apps")));
        }
        for dir in dirs {
            for ext in ["svg", "png"] {
                consider(root, dir.join(format!("{n}.{ext}")), &mut found);
            }
        }
    }
    if found.is_empty() {
        // `.DirIcon` is usually an extension-less symlink.
        consider(root, root.join(".DirIcon"), &mut found);
    }
    found.sort_by_key(|(w, _, _)| *w);
    found.pop().map(|(_, p, e)| (p, e))
}

/// GitHub repos mentioned in the app's AppStream metadata (homepage, vcs-browser...). Hints only.
fn find_hints(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for dir in ["usr/share/metainfo", "usr/share/appdata"] {
        let Ok(rd) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "xml") {
                continue;
            }
            let Some(real) = inside(root, &p) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(real) else {
                continue;
            };
            for chunk in text.split("https://github.com/").skip(1).take(40) {
                let path: String = chunk
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || "-_./".contains(*c))
                    .collect();
                let mut seg = path.split('/');
                if let (Some(o), Some(r)) = (seg.next(), seg.next()) {
                    let r = r.trim_end_matches(".git");
                    let ok = |s: &str| !s.is_empty() && s != "." && s != ".." && s.len() <= 100;
                    let repo = format!("{o}/{r}");
                    if ok(o) && ok(r) && !found.contains(&repo) {
                        found.push(repo);
                    }
                }
            }
        }
    }
    found.truncate(5);
    found
}

/// Does the app bundle Electron's sandbox helper? (Looked for near the top of the tree.)
fn is_electron(root: &Path) -> bool {
    fn walk(d: &Path, depth: u32) -> bool {
        let Ok(rd) = std::fs::read_dir(d) else {
            return false;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == "chrome-sandbox") {
                return true;
            }
            if depth > 0 && e.file_type().is_ok_and(|t| t.is_dir()) && walk(&p, depth - 1) {
                return true;
            }
        }
        false
    }
    walk(root, 3)
}

/// Is `src` byte-identical to the installed file? Different sizes cannot be, so the (slow) hashes
/// are only computed when the sizes match.
fn same_content(old: &App, src: &Path, src_sha: &str) -> bool {
    let (Ok(a), Ok(b)) = (std::fs::metadata(&old.file), std::fs::metadata(src)) else {
        return false;
    };
    if a.len() != b.len() {
        return false;
    }
    let s = if src_sha.is_empty() {
        sha256_file(src).ok()
    } else {
        Some(src_sha.to_string())
    };
    let o = if old.sha256.is_empty() {
        sha256_file(&old.file).ok()
    } else {
        Some(old.sha256.clone())
    };
    s.is_some() && s == o
}

/// Run the file directly (FUSE) unless the host cannot: an old runtime needs FUSE 2, and any
/// runtime needs a `fusermount` to mount itself. Otherwise unpack once and run from the tree.
pub fn choose_mode(info: &Info, host: &Host) -> &'static str {
    let fuse2 = if info.machine == EM_386 && host.arch == "x86_64" {
        host.fuse2_32
    } else {
        host.fuse2
    };
    if (info.runtime_needs_fuse2 && !fuse2) || !host.fusermount {
        "extract"
    } else {
        "fuse"
    }
}

/// A command called `name` provided by something else on this system.
fn other_command(ctx: &Ctx, name: &str) -> Option<PathBuf> {
    const SYSTEM: [&str; 6] = [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ];
    let path = ctx.host.path_var.clone().unwrap_or_default();
    std::env::split_paths(&path)
        .chain(SYSTEM.iter().map(PathBuf::from))
        .filter(|d| d.is_absolute() && d != &ctx.dirs.bin)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Where a new app's command goes. The name comes from inside the AppImage, so it must never
/// replace a file ifhai did not write, nor take over a command that already exists (`git`, `sudo`).
/// In either case the command is called `ifhai-<name>` instead.
fn new_launcher(ctx: &Ctx, slug: &str) -> Result<(PathBuf, Option<String>)> {
    let ours = |p: &Path| {
        std::fs::read_to_string(p).is_ok_and(|t| t.starts_with("#!/bin/sh\n# Managed by ifhai."))
    };
    let free = |p: &Path| std::fs::symlink_metadata(p).is_err() || ours(p);
    let plain = ctx.dirs.bin.join(slug);
    let other = other_command(ctx, slug);
    if other.is_none() && free(&plain) {
        return Ok((plain, None));
    }
    let renamed = ctx.dirs.bin.join(format!("ifhai-{slug}"));
    if !free(&renamed) {
        return Err(Error::Failed(format!(
            "{} already exists and ifhai did not create it, so it was left alone.\nMove it, then try again.",
            renamed.display()
        )));
    }
    let taken = other.unwrap_or(plain);
    Ok((
        renamed,
        Some(format!(
            "{} already exists, so this app's command is ifhai-{slug} instead.",
            taken.display()
        )),
    ))
}

const EM_386: u16 = 3;
const EM_ARM: u16 = 40;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;

/// Why this computer cannot run the file at all, if it cannot. Unknown processors are let through.
fn wrong_processor(info: &Info, host: &Host) -> Option<String> {
    let built = match info.machine {
        EM_386 => "32-bit x86 (i386)",
        EM_ARM => "32-bit ARM",
        EM_X86_64 => "x86_64",
        EM_AARCH64 => "64-bit ARM (aarch64)",
        _ => return None,
    };
    let (native, smaller) = match host.arch.as_str() {
        "x86_64" => (EM_X86_64, Some(EM_386)),
        "aarch64" => (EM_AARCH64, Some(EM_ARM)),
        "x86" => (EM_386, None),
        "arm" => (EM_ARM, None),
        _ => return None,
    };
    if info.machine == native || (Some(info.machine) == smaller && host.compat32) {
        return None;
    }
    let why = if Some(info.machine) == smaller {
        "and this system has no 32-bit support"
    } else {
        "and this computer cannot run it"
    };
    Some(format!(
        "is built for {built}, {why}. Installing FUSE will not help.\nDownload the {} version of the app instead.",
        host.arch
    ))
}

#[derive(Default)]
struct Undo {
    created: Vec<PathBuf>,
    /// (original place, where it is now): put it back.
    moved: Vec<(PathBuf, PathBuf)>,
}

impl Undo {
    fn run(self) {
        for p in self.created.iter().rev() {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
        for (orig, now) in self.moved.iter().rev() {
            let _ = std::fs::rename(now, orig);
        }
    }
}

/// Install (or upgrade) one AppImage file. `keep` copies instead of moving the source.
pub fn install_file(
    ctx: &Ctx,
    state: &mut State,
    src: &Path,
    origin: Origin,
    keep: bool,
) -> Result<Done> {
    let head = appimage::read_head(src)
        .map_err(|e| Error::Failed(format!("Cannot read {}: {e}", src.display())))?;
    let info: Info = appimage::inspect(&head)
        .map_err(|why| Error::Failed(format!("{} {why}.", src.display())))?;
    // Checked before anything runs: a program for another processor fails with a baffling error.
    if let Some(why) = wrong_processor(&info, ctx.host) {
        let file = src.file_name().unwrap_or(src.as_os_str()).to_string_lossy();
        return Err(Error::Failed(format!("{file} {why}")));
    }
    // Only a verified download arrives with a hash. Hashing a 120 MB file just to install it would
    // cost a second for nothing, so it is computed lazily and only when it can matter.
    let sha = origin.known_sha256.clone().unwrap_or_default();
    let d = ctx.dirs;

    std::fs::create_dir_all(&d.support)?;
    let tmp = temp_dir(&d.support, "tmp");
    std::fs::create_dir_all(&tmp)?;
    let cleanup_tmp = |t: &Path| {
        let _ = std::fs::remove_dir_all(t);
    };

    // Unpack to read the app's own description. This runs the AppImage's runtime, not the app.
    if let Err(e) = ctx.system.extract(src, &tmp, info.fs_offset) {
        cleanup_tmp(&tmp);
        return Err(e);
    }
    let root = tmp.join("squashfs-root");
    let desk_path = find_desktop(&root);
    let desk = desk_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| desktop::parse(&t))
        .unwrap_or_default();
    let file_name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (fname_name, fname_version) = appimage::name_and_version(&file_name);
    let name = desk.get("Name").map(str::to_string).unwrap_or(fname_name);
    let slug = appimage::slugify(&name);
    let version = origin
        .version_hint
        .clone()
        .or_else(|| desk.get("X-AppImage-Version").map(str::to_string))
        .or(fname_version)
        .unwrap_or_else(|| "unknown".into());
    let existing = state.apps.get(&slug).cloned();

    if let Some(old) = existing.as_ref().filter(|o| same_content(o, src, &sha)) {
        cleanup_tmp(&tmp);
        // Byte-identical to what is installed, so a redundant download can go (as promised: moved).
        if !keep && std::fs::canonicalize(src).ok() != std::fs::canonicalize(&old.file).ok() {
            let _ = std::fs::remove_file(src);
        }
        return Ok(Done {
            app: old.clone(),
            outcome: Outcome::Unchanged,
            notes: vec![],
        });
    }

    let mode = choose_mode(&info, ctx.host);
    // Every AppImage starts through AppRun, mounted or unpacked.
    if !root.join("AppRun").exists() {
        cleanup_tmp(&tmp);
        return Err(Error::Failed(format!(
            "{name} is missing its start file (AppRun), so it cannot start. The package is broken; report it to the people who publish it."
        )));
    }
    let mut notes = Vec::new();
    if mode == "extract" {
        notes.push(if ctx.host.fusermount {
            format!("{name} needs FUSE 2, which this system lacks. It was set up to run unpacked instead, so it works.")
        } else {
            format!("This system has no FUSE (no fusermount), which AppImages need to run directly. {name} was set up to run unpacked instead, so it works.")
        });
    }
    if info.kind == Kind::Type1 && mode == "fuse" {
        notes.push("This is an old (type 1) AppImage; if it does not start, it needs FUSE.".into());
    }

    let file = d.apps.join(format!("{slug}.AppImage"));
    // Never replace an app file ifhai did not put there (one saved here by hand, say).
    let same_file = std::fs::canonicalize(src)
        .ok()
        .is_some_and(|c| Some(c) == std::fs::canonicalize(&file).ok());
    if existing.is_none() && !same_file && std::fs::symlink_metadata(&file).is_ok() {
        cleanup_tmp(&tmp);
        return Err(Error::Failed(format!(
            "{} already exists and ifhai did not install it, so it was left alone.\nTo hand it over to ifhai, run: ifhai install {}",
            file.display(),
            file.display()
        )));
    }
    let launcher = match &existing {
        Some(o) => o.launcher.clone(),
        None => match new_launcher(ctx, &slug) {
            Ok((path, note)) => {
                notes.extend(note);
                path
            }
            Err(e) => {
                cleanup_tmp(&tmp);
                return Err(e);
            }
        },
    };
    let app = App {
        name: name.clone(),
        slug: slug.clone(),
        version: version.clone(),
        tag: origin.tag.clone(),
        mode: mode.into(),
        source: origin
            .source
            .clone()
            .or_else(|| {
                info.update_info
                    .as_deref()
                    .and_then(appimage::github_source)
                    .map(|g| Source {
                        repo: g.repo,
                        pattern: g.pattern,
                    })
            })
            // a newer file of an app we already follow keeps following the same place
            .or_else(|| existing.as_ref().and_then(|o| o.source.clone())),
        sha256: sha.clone(),
        electron: is_electron(&root),
        hints: find_hints(&root),
        hints_tried: false,
        origin_name: file_name.clone(),
        installed_at: now_secs(),
        previous: existing.as_ref().map(|o| Previous {
            version: o.version.clone(),
            tag: o.tag.clone(),
        }),
        file,
        launcher,
        desktop: d.applications.join(format!("ifhai-{slug}.desktop")),
        icon: None,
        support: d.support.join(&slug),
    };

    // Read the icon now: in unpacked mode the tree is moved before the icon is written.
    if desk.entry().is_none() {
        notes.push(format!(
            "{name} ships no usable menu file, so ifhai made a basic menu entry."
        ));
    }
    let icon_pick = find_icon(&root, desk.get("Icon")).and_then(|(path, ext)| {
        let width = png_width(&path);
        std::fs::read(&path).ok().map(|bytes| (bytes, ext, width))
    });
    if icon_pick.is_none() {
        notes.push(format!(
            "{name} ships no usable icon, so it gets a generic one."
        ));
    }
    let mut app = app;
    // The file may already sit exactly where it belongs (dropped into ~/Applications by hand).
    // Copying a file onto itself would truncate it, so in that case nothing is moved at all.
    let in_place = std::fs::canonicalize(src)
        .ok()
        .is_some_and(|c| Some(c) == std::fs::canonicalize(&app.file).ok());
    if in_place {
        app.previous = existing.as_ref().and_then(|o| o.previous.clone());
    }
    let mut undo = Undo::default();
    let result = (|| -> Result<()> {
        for dir in [&d.apps, &d.bin, &d.applications, &app.support] {
            std::fs::create_dir_all(dir)?;
        }
        if existing.is_some() && !in_place {
            let prev = app.support.join("previous.AppImage");
            if app.file.exists() {
                let _ = std::fs::remove_file(&prev);
                std::fs::rename(&app.file, &prev)?;
                undo.moved.push((app.file.clone(), prev));
            }
            let old_root = app.support.join("root");
            if old_root.exists() {
                let aside = app.support.join("root.old");
                let _ = std::fs::remove_dir_all(&aside);
                std::fs::rename(&old_root, &aside)?;
                undo.moved.push((old_root, aside));
            }
        }
        if in_place {
            // already there
        } else if keep {
            std::fs::copy(src, &app.file)?;
            undo.created.push(app.file.clone());
        } else {
            move_file(src, &app.file)?;
            undo.moved.push((src.to_path_buf(), app.file.clone()));
        }
        chmod(&app.file, 0o755)?;
        if mode == "extract" {
            let dest = app.support.join("root");
            std::fs::rename(&root, &dest)?;
            undo.created.push(dest);
        }
        let icon_name = match &icon_pick {
            Some((bytes, ext, width)) => {
                let (dir, fname) = if ext == "svg" {
                    ("scalable".to_string(), format!("ifhai-{slug}.svg"))
                } else {
                    let w = width.filter(|w| (16..=1024).contains(w)).unwrap_or(256);
                    (format!("{w}x{w}"), format!("ifhai-{slug}.png"))
                };
                let dest = d.icons.join(dir).join("apps").join(fname);
                std::fs::create_dir_all(dest.parent().unwrap_or(&d.icons))?;
                write_atomic(&dest, bytes, 0o644)?;
                undo.created.push(dest.clone());
                app.icon = Some(dest);
                format!("ifhai-{slug}")
            }
            None => "application-x-executable".into(),
        };
        let launcher_body = if mode == "extract" {
            format!(
                "#!/bin/sh\n# Managed by ifhai. Change it with: ifhai update / ifhai remove {slug}\nAPPDIR={}\nexport APPDIR APPIMAGE={}\nexec \"$APPDIR/AppRun\" \"$@\"\n",
                sq(&app.support.join("root").to_string_lossy()),
                sq(&app.file.to_string_lossy())
            )
        } else {
            format!(
                "#!/bin/sh\n# Managed by ifhai. Change it with: ifhai update / ifhai remove {slug}\nexec {} \"$@\"\n",
                sq(&app.file.to_string_lossy())
            )
        };
        write_atomic(&app.launcher, launcher_body.as_bytes(), 0o755)?;
        undo.created.push(app.launcher.clone());
        write_atomic(
            &app.desktop,
            desktop::render(&desk, &slug, &app.name, &app.launcher, &icon_name).as_bytes(),
            0o644,
        )?;
        undo.created.push(app.desktop.clone());
        Ok(())
    })();
    if let Err(e) = result {
        undo.run();
        cleanup_tmp(&tmp);
        return Err(e);
    }
    cleanup_tmp(&tmp);
    let _ = std::fs::remove_dir_all(app.support.join("root.old"));
    ctx.system.refresh_desktop(&d.applications);
    let outcome = match &existing {
        Some(o) => Outcome::Upgraded {
            from: o.version.clone(),
        },
        None => Outcome::Installed,
    };
    state.apps.insert(slug, app.clone());
    Ok(Done {
        app,
        outcome,
        notes,
    })
}

/// Delete everything recorded for `app`. Only paths ifhai may own are touched.
pub fn remove_files(dirs: &Dirs, app: &App) -> Result<()> {
    let mut paths: Vec<&Path> = vec![&app.file, &app.launcher, &app.desktop, &app.support];
    if let Some(i) = &app.icon {
        paths.push(i);
    }
    for p in &paths {
        if !dirs.may_delete(p) {
            return Err(Error::Failed(format!(
                "The install record lists {} for {}, which is not somewhere ifhai manages. Nothing was deleted.",
                p.display(),
                app.name
            )));
        }
    }
    for p in paths {
        let r = if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        match r {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(format!("Cannot delete {}: {e}", p.display()))),
        }
    }
    Ok(())
}

/// Swap the current version with the retained previous one.
pub fn rollback(ctx: &Ctx, state: &mut State, slug: &str) -> Result<(String, String)> {
    let app = state
        .apps
        .get(slug)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("'{slug}' is not installed.")))?;
    let prev_file = app.support.join("previous.AppImage");
    let Some(prev) = app.previous.clone().filter(|_| prev_file.exists()) else {
        return Err(Error::NotFound(format!(
            "{} has no earlier version to go back to.",
            app.name
        )));
    };
    let head = appimage::read_head(&prev_file)?;
    let info = appimage::inspect(&head)
        .map_err(|w| Error::Failed(format!("The saved earlier version {w}.")))?;
    let mode = choose_mode(&info, ctx.host);
    let swap = app.support.join("swap.AppImage");
    std::fs::rename(&app.file, &swap)?;
    if let Err(e) = std::fs::rename(&prev_file, &app.file) {
        let _ = std::fs::rename(&swap, &app.file);
        return Err(e.into());
    }
    let _ = std::fs::rename(&swap, &prev_file);
    let mut new = app.clone();
    new.mode = mode.into();
    new.version = prev.version.clone();
    new.tag = prev.tag.clone();
    new.sha256 = String::new(); // unknown: computed only if it is ever needed
    new.previous = Some(Previous {
        version: app.version.clone(),
        tag: app.tag.clone(),
    });
    if mode == "extract" {
        let t = temp_dir(&ctx.dirs.support, "rb");
        std::fs::create_dir_all(&t)?;
        ctx.system.extract(&new.file, &t, info.fs_offset)?;
        let _ = std::fs::remove_dir_all(new.support.join("root"));
        std::fs::rename(t.join("squashfs-root"), new.support.join("root"))?;
        let _ = std::fs::remove_dir_all(&t);
    }
    if mode != app.mode {
        // Same launcher path, different way of running: rewrite it.
        let body = if mode == "extract" {
            format!(
                "#!/bin/sh\n# Managed by ifhai. Change it with: ifhai update / ifhai remove {slug}\nAPPDIR={}\nexport APPDIR APPIMAGE={}\nexec \"$APPDIR/AppRun\" \"$@\"\n",
                sq(&new.support.join("root").to_string_lossy()),
                sq(&new.file.to_string_lossy())
            )
        } else {
            format!(
                "#!/bin/sh\n# Managed by ifhai. Change it with: ifhai update / ifhai remove {slug}\nexec {} \"$@\"\n",
                sq(&new.file.to_string_lossy())
            )
        };
        write_atomic(&new.launcher, body.as_bytes(), 0o755)?;
    }
    let (from, to) = (app.version.clone(), new.version.clone());
    state.apps.insert(slug.to_string(), new);
    Ok((from, to))
}
