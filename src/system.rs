//! Everything that touches the machine beyond our own files: unpacking an AppImage, launching an
//! app, refreshing desktop caches, and asking what the host supports. Faked in tests.

use crate::error::{Error, Result};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Host {
    /// FUSE 2 is usable: libfuse.so.2 and a `fusermount`.
    pub fuse2: bool,
    /// The kernel/AppArmor forbids unprivileged user namespaces (breaks Electron sandboxes).
    pub userns_restricted: bool,
    /// `x86_64`, `aarch64`...
    pub arch: String,
    pub path_var: Option<String>,
}

pub trait System {
    /// Unpack the AppImage's filesystem into `into/squashfs-root`.
    fn extract(&self, file: &Path, into: &Path, fs_offset: u64) -> Result<()>;
    /// Tell the desktop its application list changed. Best effort, never fatal.
    fn refresh_desktop(&self, applications_dir: &Path);
    /// Run a launcher and wait; returns its exit code.
    fn launch(&self, launcher: &Path, args: &[String]) -> Result<i32>;
}

pub struct RealSystem;

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

impl RealSystem {
    pub fn host() -> Host {
        let libs = [
            "/usr/lib64/libfuse.so.2",
            "/usr/lib/libfuse.so.2",
            "/lib64/libfuse.so.2",
            "/lib/libfuse.so.2",
            "/usr/lib/x86_64-linux-gnu/libfuse.so.2",
            "/lib/x86_64-linux-gnu/libfuse.so.2",
            "/usr/lib/aarch64-linux-gnu/libfuse.so.2",
            "/lib/aarch64-linux-gnu/libfuse.so.2",
        ];
        let lib = libs.iter().any(|p| Path::new(p).exists());
        let fusermount = on_path("fusermount")
            || Path::new("/usr/bin/fusermount").exists()
            || Path::new("/bin/fusermount").exists();
        let read = |p: &str| {
            std::fs::read_to_string(p)
                .ok()
                .map(|s| s.trim().to_string())
        };
        let userns_restricted =
            read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1")
                || read("/proc/sys/user/max_user_namespaces").as_deref() == Some("0");
        Host {
            fuse2: lib && fusermount,
            userns_restricted,
            arch: std::env::consts::ARCH.to_string(),
            path_var: std::env::var("PATH").ok(),
        }
    }
}

fn make_executable(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

impl System for RealSystem {
    /// Runs the AppImage's own `--appimage-extract`; that executes its runtime (not the app), the
    /// same thing every AppImage manager does. If the runtime cannot start (an old runtime on a
    /// host without FUSE 2), fall back to `unsquashfs` when it is installed.
    fn extract(&self, file: &Path, into: &Path, fs_offset: u64) -> Result<()> {
        make_executable(file)?;
        let out = Command::new(file)
            .arg("--appimage-extract")
            .current_dir(into)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        let root = into.join("squashfs-root");
        if matches!(&out, Ok(o) if o.status.success()) && root.is_dir() {
            return Ok(());
        }
        let _ = std::fs::remove_dir_all(&root);
        if on_path("unsquashfs") {
            let ok = Command::new("unsquashfs")
                .args([
                    "-no-progress",
                    "-quiet",
                    "-offset",
                    &fs_offset.to_string(),
                    "-dest",
                ])
                .arg(&root)
                .arg(file)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok && root.is_dir() {
                return Ok(());
            }
        }
        let why = match out {
            Ok(o) => String::from_utf8_lossy(&o.stderr)
                .lines()
                .next()
                .unwrap_or("it exited with an error")
                .to_string(),
            Err(e) => e.to_string(),
        };
        Err(Error::Failed(format!(
            "Could not unpack this AppImage ({why}).\n\nIts runtime probably needs FUSE 2, which this system lacks. Install fuse-libs (Fedora) or libfuse2 (Debian/Ubuntu), or squashfs-tools so ifhai can unpack it itself, then try again."
        )))
    }

    fn refresh_desktop(&self, applications_dir: &Path) {
        if on_path("update-desktop-database") {
            let _ = Command::new("update-desktop-database")
                .arg(applications_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    fn launch(&self, launcher: &Path, args: &[String]) -> Result<i32> {
        let status = Command::new(launcher)
            .args(args)
            .status()
            .map_err(|e| Error::Failed(format!("Could not start {}: {e}", launcher.display())))?;
        Ok(status.code().unwrap_or(1))
    }
}
