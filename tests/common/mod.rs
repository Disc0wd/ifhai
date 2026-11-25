#![allow(dead_code)]
use ifhai::appimage::testutil::fake;
use ifhai::commands::{self, Env, Io};
use ifhai::error::{Error, Result};
use ifhai::fetch::Fetcher;
use ifhai::state::{Dirs, State};
use ifhai::system::{Host, System};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct TestIo {
    pub out: String,
    pub err: String,
    pub answers: VecDeque<String>,
    pub asked: Vec<String>,
    pub interactive: bool,
    pub tty: bool,
    pub color: bool,
    pub err_tty: bool,
    pub utf8: bool,
}

impl TestIo {
    pub fn non_interactive() -> Self {
        TestIo {
            out: String::new(),
            err: String::new(),
            answers: VecDeque::new(),
            asked: vec![],
            interactive: false,
            tty: false,
            color: false,
            err_tty: false,
            utf8: true,
        }
    }
    pub fn answering(a: &[&str]) -> Self {
        TestIo {
            interactive: true,
            answers: a.iter().map(|s| s.to_string()).collect(),
            ..Self::non_interactive()
        }
    }
    pub fn term(a: &[&str]) -> Self {
        TestIo {
            tty: true,
            err_tty: true,
            color: true,
            ..Self::answering(a)
        }
    }
}

impl Io for TestIo {
    fn out(&mut self, s: &str) {
        self.out.push_str(s);
    }
    fn err(&mut self, s: &str) {
        self.err.push_str(s);
    }
    fn ask(&mut self, p: &str) -> Option<String> {
        self.asked.push(p.to_string());
        self.answers.pop_front()
    }
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn stdout_is_tty(&self) -> bool {
        self.tty
    }
    fn color_allowed(&self) -> bool {
        self.tty && self.color
    }
    fn stderr_is_tty(&self) -> bool {
        self.err_tty
    }
    fn stderr_color_allowed(&self) -> bool {
        self.err_tty && self.color
    }
    fn unicode(&self) -> bool {
        self.utf8
    }
}

/// What a fake AppImage contains when "extracted".
#[derive(Clone)]
pub struct Spec {
    pub file_name: String,
    pub desktop: String,
    pub icon: Option<Vec<u8>>,
    pub app_run: bool,
    pub electron: bool,
    pub update_info: Option<String>,
    pub needs_fuse2: bool,
}

pub fn png(width: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&13u32.to_be_bytes());
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    b
}

impl Spec {
    pub fn new(file_name: &str, name: &str) -> Spec {
        Spec {
            file_name: file_name.into(),
            desktop: format!(
                "[Desktop Entry]\nType=Application\nName={name}\nExec=AppRun %U\nIcon={}\nCategories=Utility;\n",
                name.to_lowercase()
            ),
            icon: Some(png(256)),
            app_run: true,
            electron: false,
            update_info: None,
            needs_fuse2: false,
        }
    }
}

#[derive(Default)]
pub struct MockSystem {
    pub specs: Mutex<HashMap<String, Spec>>,
    pub fail_extract: Mutex<bool>,
    pub refreshes: Mutex<usize>,
    pub launched: Mutex<Vec<(PathBuf, Vec<String>)>>,
    pub exit_code: Mutex<i32>,
    pub extracted: Mutex<Vec<String>>,
    /// file name -> the URL a browser recorded as its origin
    pub origins: Mutex<HashMap<String, String>>,
}

impl System for MockSystem {
    fn extract(&self, file: &Path, into: &Path, _off: u64) -> Result<()> {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        self.extracted.lock().unwrap().push(name.clone());
        if *self.fail_extract.lock().unwrap() {
            return Err(Error::Failed(
                "Could not unpack this AppImage (mock).".into(),
            ));
        }
        let spec = self
            .specs
            .lock()
            .unwrap()
            .get(&name)
            .cloned()
            .unwrap_or_else(|| Spec::new(&name, "Unknown"));
        let root = into.join("squashfs-root");
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("app.desktop"), &spec.desktop)?;
        if let Some(icon) = &spec.icon {
            let icon_name = spec
                .desktop
                .lines()
                .find_map(|l| l.strip_prefix("Icon="))
                .unwrap_or("icon");
            std::fs::write(root.join(format!("{icon_name}.png")), icon)?;
        }
        if spec.app_run {
            std::fs::write(root.join("AppRun"), "#!/bin/sh\n")?;
        }
        if spec.electron {
            std::fs::create_dir_all(root.join("opt/app"))?;
            std::fs::write(root.join("opt/app/chrome-sandbox"), "x")?;
        }
        Ok(())
    }
    fn refresh_desktop(&self, _d: &Path) {
        *self.refreshes.lock().unwrap() += 1;
    }
    fn origin_url(&self, file: &Path) -> Option<String> {
        self.origins
            .lock()
            .unwrap()
            .get(file.file_name()?.to_str()?)
            .cloned()
    }
    fn launch(&self, launcher: &Path, args: &[String]) -> Result<i32> {
        self.launched
            .lock()
            .unwrap()
            .push((launcher.to_path_buf(), args.to_vec()));
        Ok(*self.exit_code.lock().unwrap())
    }
}

#[derive(Default)]
pub struct MockFetcher {
    pub texts: Mutex<HashMap<String, String>>,
    pub files: Mutex<HashMap<String, Vec<u8>>>,
    pub tags: Mutex<HashMap<String, String>>,
    pub down: Mutex<Vec<String>>,
    pub calls: Mutex<Vec<String>>,
}

impl MockFetcher {
    fn is_down(&self, what: &str) -> bool {
        self.down
            .lock()
            .unwrap()
            .iter()
            .any(|d| what.contains(d.as_str()))
    }
    pub fn fetches(&self, needle: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains(needle))
            .count()
    }
}

impl Fetcher for MockFetcher {
    fn text(&self, url: &str) -> Result<String> {
        self.calls.lock().unwrap().push(format!("text {url}"));
        if self.is_down(url) {
            return Err(Error::Network(format!(
                "Could not reach {url}.\nmock: network down"
            )));
        }
        self.texts
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Not found: {url}")))
    }
    fn file(&self, url: &str, dest: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(format!("file {url}"));
        if self.is_down(url) {
            return Err(Error::Network(format!(
                "Could not reach {url}.\nmock: network down"
            )));
        }
        let bytes = self
            .files
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Not found: {url}")))?;
        std::fs::write(dest, bytes)?;
        Ok(())
    }
    fn latest_tag(&self, repo: &str) -> Result<Option<String>> {
        self.calls.lock().unwrap().push(format!("tag {repo}"));
        if self.is_down(repo) {
            return Err(Error::Network(
                "Could not reach github.com.\nmock: network down".into(),
            ));
        }
        Ok(self.tags.lock().unwrap().get(repo).cloned())
    }
}

pub fn sha256(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub system: MockSystem,
    pub fetcher: MockFetcher,
    pub dirs: Dirs,
    pub host: Host,
}

impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path().join("home");
        std::fs::create_dir_all(h.join("Downloads")).unwrap();
        let dirs = Dirs {
            apps: h.join("Applications"),
            support: h.join("Applications/.ifhai"),
            bin: h.join(".local/bin"),
            applications: h.join(".local/share/applications"),
            icons: h.join(".local/share/icons/hicolor"),
            state_file: h.join(".local/share/ifhai/state.toml"),
        };
        let host = Host {
            fuse2: true,
            fusermount: true,
            userns_restricted: false,
            arch: "x86_64".into(),
            path_var: Some(format!("/usr/bin:{}", dirs.bin.display())),
        };
        Fixture {
            dir,
            system: MockSystem::default(),
            fetcher: MockFetcher::default(),
            dirs,
            host,
        }
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    pub fn downloads(&self) -> PathBuf {
        self.home().join("Downloads")
    }

    /// A fresh "download": a valid AppImage that is **not** executable, like a browser leaves it.
    pub fn download(&self, spec: &Spec) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = self.downloads().join(&spec.file_name);
        let bytes = fake(spec.update_info.as_deref(), spec.needs_fuse2);
        // Make the content unique per spec so different versions have different checksums.
        let mut b = bytes;
        b.extend_from_slice(spec.file_name.as_bytes());
        std::fs::write(&p, b).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        self.system
            .specs
            .lock()
            .unwrap()
            .insert(spec.file_name.clone(), spec.clone());
        p
    }

    pub fn run(&self, io: &mut TestIo, args: &[&str]) -> i32 {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut env = Env {
            fetcher: &self.fetcher,
            system: &self.system,
            dirs: &self.dirs,
            host: &self.host,
            io,
            self_exe: "/opt/ifhai/bin/ifhai".into(),
        };
        commands::run(&args, &mut env)
    }

    pub fn yes(&self, args: &[&str]) -> (i32, TestIo) {
        let mut io = TestIo::non_interactive();
        let mut a = args.to_vec();
        a.push("--yes");
        let code = self.run(&mut io, &a);
        (code, io)
    }

    pub fn state(&self) -> State {
        State::load(&self.dirs.state_file).unwrap()
    }

    /// Publish a GitHub release. Each asset is (name, bytes); `digest` adds the API sha256.
    pub fn release(&self, repo: &str, tag: &str, assets: &[(&str, Vec<u8>)], digest: bool) {
        let mut js = Vec::new();
        for (name, bytes) in assets {
            let url = format!("https://github.com/{repo}/releases/download/{tag}/{name}");
            self.fetcher
                .files
                .lock()
                .unwrap()
                .insert(url.clone(), bytes.clone());
            let d = if digest && name.to_lowercase().ends_with(".appimage") {
                format!(",\"digest\":\"sha256:{}\"", sha256(bytes))
            } else {
                String::new()
            };
            js.push(format!(
                "{{\"name\":\"{name}\",\"browser_download_url\":\"{url}\",\"size\":{}{d}}}",
                bytes.len()
            ));
        }
        let body = format!("{{\"tag_name\":\"{tag}\",\"assets\":[{}]}}", js.join(","));
        self.fetcher.texts.lock().unwrap().insert(
            format!("https://api.github.com/repos/{repo}/releases/latest"),
            body,
        );
        self.fetcher
            .tags
            .lock()
            .unwrap()
            .insert(repo.into(), tag.into());
    }

    /// The bytes `download()` would write for this spec, for publishing as a release asset.
    pub fn bytes_of(&self, spec: &Spec) -> Vec<u8> {
        let mut b = fake(spec.update_info.as_deref(), spec.needs_fuse2);
        b.extend_from_slice(spec.file_name.as_bytes());
        self.system
            .specs
            .lock()
            .unwrap()
            .insert(spec.file_name.clone(), spec.clone());
        b
    }
}
