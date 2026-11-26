mod common;
use common::*;
use std::os::unix::fs::PermissionsExt;

fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for d in it.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn obsidian(f: &Fixture, v: &str) -> std::path::PathBuf {
    f.download(&Spec::new(&format!("Obsidian-{v}.AppImage"), "Obsidian"))
}

// ---- the core job ---------------------------------------------------------------------------

#[test]
fn a_downloaded_file_becomes_a_working_app_with_one_command() {
    let f = Fixture::new();
    let src = obsidian(&f, "1.8.9");
    assert_eq!(
        mode(&src),
        0o644,
        "starts life non-executable, as downloaded"
    );
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(
        f.run(&mut io, &["install", src.to_str().unwrap()]),
        0,
        "{}",
        io.err
    );

    let app = f.dirs.apps.join("obsidian.AppImage");
    assert!(app.exists() && !src.exists(), "moved, not copied");
    assert_eq!(mode(&app), 0o755, "made executable");
    let launcher = f.dirs.bin.join("obsidian");
    assert_eq!(mode(&launcher), 0o755);
    let l = std::fs::read_to_string(&launcher).unwrap();
    assert!(
        l.contains(&format!("exec '{}' \"$@\"", app.display())) && l.starts_with("#!/bin/sh"),
        "{l}"
    );

    let desktop =
        std::fs::read_to_string(f.dirs.applications.join("ifhai-obsidian.desktop")).unwrap();
    assert!(
        desktop.contains(&format!("Exec={} %U\n", launcher.display())),
        "{desktop}"
    );
    assert!(
        desktop.contains("Icon=ifhai-obsidian\n")
            && desktop.contains("X-IFHAI-ID=obsidian")
            && desktop.contains("Categories=Utility;"),
        "{desktop}"
    );
    assert!(!desktop.contains("AppRun"));
    assert!(
        f.dirs
            .icons
            .join("256x256/apps/ifhai-obsidian.png")
            .exists(),
        "icon sized from the image"
    );

    let a = &f.state().apps["obsidian"];
    assert_eq!(
        (a.name.as_str(), a.version.as_str(), a.mode.as_str()),
        ("Obsidian", "1.8.9", "fuse")
    );
    assert_eq!(a.file, app);
    assert!(
        io.out.contains("Installed Obsidian 1.8.9") && io.out.contains("run: obsidian"),
        "{}",
        io.out
    );
    assert!(
        io.out.contains("moved (the original is not kept)"),
        "{}",
        io.out
    );
    assert_eq!(io.asked, vec!["Install Obsidian 1.8.9? [Y/n] "]);
    assert!(
        *f.system.refreshes.lock().unwrap() >= 1,
        "desktop database refreshed"
    );
    assert!(
        std::fs::read_dir(&f.dirs.support)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().starts_with(".tmp")),
        "no temp files left"
    );
}

#[test]
fn keep_copies_instead_of_moving() {
    let f = Fixture::new();
    let src = obsidian(&f, "1.0");
    let (code, io) = f.yes(&["install", src.to_str().unwrap(), "--keep"]);
    assert_eq!(code, 0, "{}", io.err);
    assert!(src.exists() && f.dirs.apps.join("obsidian.AppImage").exists());
    assert!(io.out.contains("copied to"), "{}", io.out);
}

#[test]
fn several_files_one_confirmation() {
    let f = Fixture::new();
    let a = obsidian(&f, "1.0");
    let b = f.download(&Spec::new("Krita-5.2-x86_64.AppImage", "Krita"));
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(
        f.run(
            &mut io,
            &["install", a.to_str().unwrap(), b.to_str().unwrap()]
        ),
        0,
        "{}",
        io.err
    );
    assert_eq!(io.asked, vec!["Install 2 apps? [Y/n] "]);
    assert_eq!(f.state().apps.len(), 2);
}

#[test]
fn confirmation_rules_and_nothing_changes_on_no() {
    let f = Fixture::new();
    let src = obsidian(&f, "1.0");
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["install", src.to_str().unwrap()]), 5);
    assert!(io.err.contains("--yes") && io.asked.is_empty());
    let mut io = TestIo::answering(&["n"]);
    assert_eq!(f.run(&mut io, &["install", src.to_str().unwrap()]), 6);
    assert_eq!(io.err, "ifhai: Cancelled.\n");
    assert!(
        src.exists() && mode(&src) == 0o644,
        "the download is untouched"
    );
    assert!(!f.dirs.apps.exists() && !f.dirs.state_file.exists());
    let mut io = TestIo::answering(&[""]);
    assert_eq!(
        f.run(&mut io, &["install", src.to_str().unwrap()]),
        0,
        "enter accepts the default"
    );
}

#[test]
fn things_that_are_not_appimages_are_refused_cleanly() {
    let f = Fixture::new();
    let txt = f.downloads().join("notes.AppImage");
    std::fs::write(
        &txt,
        "just text, nothing to see here, definitely more than sixty-four bytes of it okay",
    )
    .unwrap();
    let (code, io) = f.yes(&["install", txt.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        io.err.contains("not an executable") || io.err.contains("ELF"),
        "{}",
        io.err
    );
    let mut elf = ifhai::appimage::testutil::fake(None, false);
    elf[8..11].copy_from_slice(&[0, 0, 0]);
    let plain = f.downloads().join("tool");
    std::fs::write(&plain, elf).unwrap();
    let (code, io) = f.yes(&["install", plain.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(io.err.contains("not an AppImage"), "{}", io.err);
    assert!(
        txt.exists() && plain.exists() && !f.dirs.bin.exists(),
        "nothing was touched or created"
    );
    assert_eq!(f.fetcher.fetches(""), 0);
}

#[test]
fn bad_targets_get_clear_errors() {
    let f = Fixture::new();
    let (code, io) = f.yes(&["install", f.downloads().to_str().unwrap()]);
    assert_eq!(
        (code, io.err.contains("is a folder")),
        (2, true),
        "{}",
        io.err
    );
    let (code, io) = f.yes(&["install", "no-such-file.AppImage"]);
    assert_eq!(code, 3);
    assert!(io.err.contains("no file called"), "{}", io.err);
    let (code, io) = f.yes(&["install", "http://example.org/a.AppImage"]);
    assert_eq!(code, 2);
    assert!(io.err.contains("https://"), "{}", io.err);
    let (code, _) = f.yes(&["install", "https://example.org/readme.txt"]);
    assert_eq!(code, 2);
    // Only the last path segment ever becomes a file name, so `..` in a URL cannot escape anywhere.
    let mut io = TestIo::answering(&["y"]);
    let code = f.run(
        &mut io,
        &["install", "https://example.org/../evil.AppImage?x=1"],
    );
    assert_eq!(code, 1, "the download simply fails here");
    assert!(!f.dirs.apps.exists() && f.state().apps.is_empty());
}

// ---- FUSE ------------------------------------------------------------------------------------

#[test]
fn without_fuse2_an_old_runtime_is_installed_unpacked_and_still_works() {
    let mut f = Fixture::new();
    f.host.fuse2 = false;
    let mut s = Spec::new("Old-1.0.AppImage", "Old");
    s.needs_fuse2 = true;
    let src = f.download(&s);
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    let a = &f.state().apps["old"];
    assert_eq!(a.mode, "extract");
    assert!(a.support.join("root/AppRun").exists());
    let l = std::fs::read_to_string(&a.launcher).unwrap();
    assert!(
        l.contains("APPDIR=")
            && l.contains("exec \"$APPDIR/AppRun\" \"$@\"")
            && l.contains("APPIMAGE="),
        "{l}"
    );
    assert!(
        io.err.contains("needs FUSE 2") && io.err.contains("run unpacked"),
        "{}",
        io.err
    );
}

#[test]
fn a_modern_runtime_needs_no_fuse2_and_runs_directly() {
    let mut f = Fixture::new();
    f.host.fuse2 = false;
    let src = obsidian(&f, "1.0");
    let (code, _) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(f.state().apps["obsidian"].mode, "fuse");
    assert!(!f.dirs.support.join("obsidian/root").exists());
}

#[test]
fn an_old_runtime_with_fuse2_present_runs_directly() {
    let f = Fixture::new();
    let mut s = Spec::new("Old-1.0.AppImage", "Old");
    s.needs_fuse2 = true;
    let src = f.download(&s);
    f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(f.state().apps["old"].mode, "fuse");
}

#[test]
fn unpacked_mode_without_apprun_is_an_honest_failure() {
    let mut f = Fixture::new();
    f.host.fuse2 = false;
    let mut s = Spec::new("Old-1.0.AppImage", "Old");
    s.needs_fuse2 = true;
    s.app_run = false;
    let src = f.download(&s);
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(io.err.contains("no AppRun"), "{}", io.err);
    assert!(src.exists() && !f.dirs.apps.join("old.AppImage").exists());
}

// ---- no duplicates, upgrade, rollback ----------------------------------------------------------

#[test]
fn a_newer_file_of_the_same_app_upgrades_in_place_with_one_menu_entry() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let (code, io) = f.yes(&["install", obsidian(&f, "2.0").to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    assert!(
        io.out.contains("Upgraded Obsidian 1.0 -> 2.0"),
        "{}",
        io.out
    );
    let entries: Vec<_> = std::fs::read_dir(&f.dirs.applications)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "ifhai-install.desktop")
        .collect();
    assert_eq!(entries.len(), 1, "one menu entry for the app, not two");
    let a = &f.state().apps["obsidian"];
    assert_eq!(a.version, "2.0");
    assert_eq!(a.previous.as_ref().unwrap().version, "1.0");
    assert!(a.support.join("previous.AppImage").exists());
    assert_eq!(f.state().apps.len(), 1);
}

#[test]
fn installing_the_identical_file_again_changes_nothing_but_drops_the_redundant_download() {
    let f = Fixture::new();
    let p = obsidian(&f, "1.0");
    f.yes(&["install", p.to_str().unwrap(), "--keep"]);
    let before = f.state().apps["obsidian"].clone();
    let (code, io) = f.yes(&["install", p.to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    assert!(io.out.contains("already installed"), "{}", io.out);
    assert_eq!(
        f.state().apps["obsidian"],
        before,
        "nothing about the install changed"
    );
    assert!(f.dirs.apps.join("obsidian.AppImage").exists());
    assert!(
        !p.exists(),
        "the identical download is removed, as the plan said it would be moved"
    );
    // With --keep it is left alone.
    let q = obsidian(&f, "1.0");
    f.yes(&["install", q.to_str().unwrap(), "--keep"]);
    assert!(q.exists());
}

#[test]
fn rollback_swaps_versions_and_back() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    f.yes(&["install", obsidian(&f, "2.0").to_str().unwrap()]);
    let before = std::fs::read(f.dirs.apps.join("obsidian.AppImage")).unwrap();
    let (code, io) = f.yes(&["rollback", "obsidian"]);
    assert_eq!(code, 0, "{}", io.err);
    assert!(io.out.contains("is back at 1.0 (was 2.0)"), "{}", io.out);
    let a = &f.state().apps["obsidian"];
    assert_eq!(
        (
            a.version.as_str(),
            a.previous.as_ref().unwrap().version.as_str()
        ),
        ("1.0", "2.0")
    );
    assert_eq!(
        std::fs::read(a.support.join("previous.AppImage")).unwrap(),
        before,
        "the newer one is kept"
    );
    assert_eq!(mode(&a.file), 0o755);
    f.yes(&["rollback", "obsidian"]);
    assert_eq!(f.state().apps["obsidian"].version, "2.0");
}

#[test]
fn rollback_with_no_earlier_version_is_not_found() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let (code, io) = f.yes(&["rollback", "obsidian"]);
    assert_eq!(code, 3);
    assert!(io.err.contains("no earlier version"), "{}", io.err);
    assert_eq!(f.yes(&["rollback", "nothing"]).0, 3);
}

// ---- atomicity -------------------------------------------------------------------------------

#[test]
fn a_failed_unpack_changes_nothing() {
    let f = Fixture::new();
    let src = obsidian(&f, "1.0");
    *f.system.fail_extract.lock().unwrap() = true;
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        io.err.contains("Could not install") && io.err.contains("Could not unpack"),
        "{}",
        io.err
    );
    assert!(
        src.exists()
            && !f.dirs.bin.exists()
            && !f.dirs.applications.exists()
            && !f.dirs.state_file.exists()
    );
    let leftovers = std::fs::read_dir(&f.dirs.support)
        .map(|r| r.flatten().count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "temp area cleaned");
}

#[test]
fn a_failure_after_the_move_puts_the_download_back() {
    let f = Fixture::new();
    let src = obsidian(&f, "1.0");
    std::fs::create_dir_all(f.dirs.icons.parent().unwrap()).unwrap();
    std::fs::write(&f.dirs.icons, "a file where a folder must go").unwrap(); // icon step will fail
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 1, "{}", io.err);
    assert!(src.exists(), "the download is back where it was");
    assert!(
        !f.dirs.apps.join("obsidian.AppImage").exists() && !f.dirs.bin.join("obsidian").exists()
    );
    assert!(f.state().apps.is_empty());
}

#[test]
fn a_failed_upgrade_leaves_the_working_version_working() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let old = std::fs::read(f.dirs.apps.join("obsidian.AppImage")).unwrap();
    let new = obsidian(&f, "2.0");
    std::fs::remove_dir_all(&f.dirs.icons).unwrap();
    std::fs::write(&f.dirs.icons, "blocker").unwrap();
    let (code, _) = f.yes(&["install", new.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert_eq!(
        std::fs::read(f.dirs.apps.join("obsidian.AppImage")).unwrap(),
        old,
        "old version restored"
    );
    assert!(new.exists());
    assert_eq!(f.state().apps["obsidian"].version, "1.0");
    assert!(
        f.dirs.bin.join("obsidian").exists()
            && f.dirs.applications.join("ifhai-obsidian.desktop").exists()
    );
}

// ---- remove / list / run ----------------------------------------------------------------------

#[test]
fn remove_deletes_everything_it_created_and_nothing_else() {
    let mut f = Fixture::new();
    f.host.fuse2 = false;
    let mut s = Spec::new("Old-1.0.AppImage", "Old");
    s.needs_fuse2 = true;
    f.yes(&["install", f.download(&s).to_str().unwrap()]);
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let other = f.dirs.apps.join("my-own-notes.txt");
    std::fs::write(&other, "mine").unwrap();
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(f.run(&mut io, &["remove", "old"]), 0, "{}", io.err);
    assert!(
        io.out.contains("settings and data are not touched") && io.out.contains("Removed: Old"),
        "{}",
        io.out
    );
    assert!(
        !f.dirs.apps.join("old.AppImage").exists()
            && !f.dirs.bin.join("old").exists()
            && !f.dirs.support.join("old").exists()
    );
    assert!(
        !f.dirs.applications.join("ifhai-old.desktop").exists()
            && !f.dirs.icons.join("256x256/apps/ifhai-old.png").exists()
    );
    assert!(
        other.exists() && f.dirs.apps.join("obsidian.AppImage").exists(),
        "unrelated files and other apps untouched"
    );
    assert!(!f.state().apps.contains_key("old") && f.state().apps.contains_key("obsidian"));
    assert!(f.dirs.support.exists(), "still in use by the other app");
    assert_eq!(f.yes(&["remove", "old"]).0, 3);
}

#[test]
fn remove_refuses_a_tampered_record_instead_of_deleting_elsewhere() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let victim = f.home().join("Documents/important.txt");
    std::fs::create_dir_all(victim.parent().unwrap()).unwrap();
    std::fs::write(&victim, "precious").unwrap();
    let mut state = f.state();
    state.apps.get_mut("obsidian").unwrap().launcher = victim.clone();
    state.save(&f.dirs.state_file).unwrap();
    let (code, io) = f.yes(&["remove", "obsidian"]);
    assert_eq!(code, 1);
    assert!(
        io.err.contains("not somewhere ifhai manages") && io.err.contains("Nothing was deleted"),
        "{}",
        io.err
    );
    assert!(victim.exists() && f.dirs.apps.join("obsidian.AppImage").exists());
}

#[test]
fn ambiguous_names_are_never_guessed() {
    let f = Fixture::new();
    f.yes(&[
        "install",
        f.download(&Spec::new("Krita-5.AppImage", "Krita"))
            .to_str()
            .unwrap(),
    ]);
    f.yes(&[
        "install",
        f.download(&Spec::new("KritaNext-6.AppImage", "Krita Next"))
            .to_str()
            .unwrap(),
    ]);
    let (code, io) = f.yes(&["remove", "kri"]);
    assert_eq!(code, 4);
    assert!(
        io.err.contains("krita") && io.err.contains("krita-next"),
        "{}",
        io.err
    );
    assert_eq!(f.state().apps.len(), 2);
    assert_eq!(f.yes(&["remove", "Krita"]).0, 0, "an exact name wins");
}

#[test]
fn list_shows_installed_apps_and_an_empty_hint() {
    let f = Fixture::new();
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["list"]), 0);
    assert!(io.err.contains("Nothing installed yet") && io.out.is_empty());
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["list"]);
    assert_eq!(io.out, "Obsidian\t1.0\tAppImage\t-\n");
    let mut io = TestIo::term(&[]);
    f.run(&mut io, &["list"]);
    assert!(strip(&io.out).starts_with("App"), "header on a terminal");
}

#[test]
fn run_launches_through_the_launcher_and_passes_arguments_and_exit_codes() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["run", "obsidian", "--flag", "file.md"]), 0);
    assert_eq!(
        f.system.launched.lock().unwrap()[0],
        (
            f.dirs.bin.join("obsidian"),
            vec!["--flag".to_string(), "file.md".to_string()]
        )
    );
    *f.system.exit_code.lock().unwrap() = 7;
    let mut io = TestIo::non_interactive();
    assert_eq!(
        f.run(&mut io, &["run", "obsidian"]),
        7,
        "the app's exit code is ours"
    );
    assert!(io.err.is_empty(), "nothing extra printed");
    assert_eq!(f.run(&mut TestIo::non_interactive(), &["run", "ghost"]), 3);
}

// ---- GitHub / URL installs ---------------------------------------------------------------------

fn kdenlive_assets(f: &Fixture, v: &str) -> (Spec, Vec<(String, Vec<u8>)>) {
    let spec = Spec::new(&format!("Kdenlive-{v}-x86_64.AppImage"), "Kdenlive");
    let bytes = f.bytes_of(&spec);
    let arm = f.bytes_of(&Spec::new(
        &format!("Kdenlive-{v}-aarch64.AppImage"),
        "Kdenlive",
    ));
    (
        spec.clone(),
        vec![
            (spec.file_name.clone(), bytes),
            (format!("Kdenlive-{v}-aarch64.AppImage"), arm),
            (format!("Kdenlive-{v}.zsync"), b"z".to_vec()),
        ],
    )
}

fn publish(f: &Fixture, repo: &str, tag: &str, v: &str, digest: bool) -> Spec {
    let (spec, assets) = kdenlive_assets(f, v);
    let refs: Vec<(&str, Vec<u8>)> = assets
        .iter()
        .map(|(n, b)| (n.as_str(), b.clone()))
        .collect();
    f.release(repo, tag, &refs, digest);
    spec
}

#[test]
fn owner_repo_installs_the_right_asset_verified_and_remembers_where_it_came_from() {
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v25.08.1", "25.08.1", true);
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(
        f.run(&mut io, &["install", "KDE/kdenlive"]),
        0,
        "{}",
        io.err
    );
    assert!(
        io.out.contains("github.com/KDE/kdenlive v25.08.1") && io.out.contains("SHA-256 verified"),
        "{}",
        io.out
    );
    let a = &f.state().apps["kdenlive"];
    assert_eq!(
        (a.version.as_str(), a.tag.as_deref()),
        ("25.08.1", Some("v25.08.1"))
    );
    let src = a.source.as_ref().unwrap();
    assert_eq!(
        (src.repo.as_str(), src.pattern.as_str()),
        ("KDE/kdenlive", "Kdenlive-*-x86_64.AppImage")
    );
    assert!(
        f.fetcher.fetches("aarch64") == 0,
        "only the right architecture is downloaded"
    );
    assert!(
        std::fs::read_dir(&f.dirs.support)
            .unwrap()
            .flatten()
            .all(|e| e.file_name() != ".dl"),
        "download area cleaned"
    );
}

#[test]
fn a_corrupted_download_is_discarded_and_nothing_is_installed() {
    let f = Fixture::new();
    let spec = publish(&f, "KDE/kdenlive", "v1.0", "1.0", true);
    f.fetcher.files.lock().unwrap().insert(
        format!(
            "https://github.com/KDE/kdenlive/releases/download/v1.0/{}",
            spec.file_name
        ),
        b"tampered".to_vec(),
    );
    let (code, io) = f.yes(&["install", "KDE/kdenlive"]);
    assert_eq!(code, 1);
    assert!(
        io.err.contains("does not match its published checksum") && io.err.contains("discarded"),
        "{}",
        io.err
    );
    assert!(f.state().apps.is_empty() && !f.dirs.apps.join("kdenlive.AppImage").exists());
    assert_eq!(
        f.system.extracted.lock().unwrap().len(),
        0,
        "never even unpacked"
    );
}

#[test]
fn a_release_without_any_checksum_is_flagged_as_unverified() {
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v1.0", "1.0", false);
    // --yes does not accept something that cannot be verified, and Enter means no.
    let (code, io) = f.yes(&["install", "KDE/kdenlive"]);
    assert_eq!(code, 5, "{}", io.err);
    let mut enter = TestIo::answering(&[""]);
    assert_eq!(f.run(&mut enter, &["install", "KDE/kdenlive", "--yes"]), 6);
    assert!(f.state().apps.is_empty());
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(
        f.run(&mut io, &["install", "KDE/kdenlive"]),
        0,
        "{}",
        io.err
    );
    assert!(
        io.out
            .contains("no checksum is published for this file (unverified)"),
        "{}",
        io.out
    );
}

#[test]
fn a_checksum_file_in_the_release_is_used_when_the_api_has_no_digest() {
    let f = Fixture::new();
    let spec = Spec::new("Tool-1.0-x86_64.AppImage", "Tool");
    let bytes = f.bytes_of(&spec);
    let sums = format!("{}  {}\n", sha256(&bytes), spec.file_name);
    f.release(
        "o/tool",
        "v1.0",
        &[(&spec.file_name, bytes), ("SHA256SUMS", sums.into_bytes())],
        false,
    );
    f.fetcher.texts.lock().unwrap().insert(
        "https://github.com/o/tool/releases/download/v1.0/SHA256SUMS".into(),
        format!("{}  {}\n", sha256(&f.bytes_of(&spec)), spec.file_name),
    );
    let (code, io) = f.yes(&["install", "o/tool"]);
    assert_eq!(code, 0, "{}", io.err);
    assert!(io.out.contains("SHA-256 verified"), "{}", io.out);
}

#[test]
fn github_problems_are_reported_as_what_they_are() {
    let f = Fixture::new();
    let (code, io) = f.yes(&["install", "ghost/repo"]);
    assert_eq!(code, 3);
    assert!(io.err.contains("no published releases"), "{}", io.err);
    f.release(
        "o/arm",
        "v1",
        &[("App-aarch64.AppImage", b"x".to_vec())],
        false,
    );
    let (code, io) = f.yes(&["install", "o/arm"]);
    assert_eq!(code, 3);
    assert!(io.err.contains("no AppImage for x86_64"), "{}", io.err);
    f.release(
        "o/two",
        "v1",
        &[
            ("App-x86_64.AppImage", b"a".to_vec()),
            ("App-legacy-x86_64.AppImage", b"b".to_vec()),
        ],
        false,
    );
    let (code, io) = f.yes(&["install", "o/two"]);
    assert_eq!(code, 4);
    assert!(
        io.err.contains("legacy") && io.err.contains("download URL"),
        "{}",
        io.err
    );
    f.fetcher.down.lock().unwrap().push("o/net".into());
    let (code, io) = f.yes(&["install", "o/net"]);
    assert_eq!(code, 1);
    assert!(io.err.contains("network down"), "{}", io.err);
}

#[test]
fn an_https_link_installs_but_is_marked_unverified() {
    let f = Fixture::new();
    let spec = Spec::new("Tool-1.0-x86_64.AppImage", "Tool");
    let bytes = f.bytes_of(&spec);
    f.fetcher.files.lock().unwrap().insert(
        "https://example.org/dl/Tool-1.0-x86_64.AppImage".into(),
        bytes,
    );
    let mut io = TestIo::answering(&["y"]);
    let url = "https://example.org/dl/Tool-1.0-x86_64.AppImage";
    assert_eq!(f.run(&mut io, &["install", url]), 0, "{}", io.err);
    assert!(io.out.contains("(unverified)"), "{}", io.out);
    assert!(
        f.state().apps["tool"].source.is_none(),
        "no update source from a bare link"
    );
}

// ---- updates ---------------------------------------------------------------------------------

#[test]
fn updates_and_update_follow_the_source_and_keep_the_old_version() {
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v1.0", "1.0", true);
    f.yes(&["install", "KDE/kdenlive"]);
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["updates"]), 0);
    assert_eq!(io.out, "Kdenlive\t1.0\t1.0\tup to date\n");
    let api_calls = f.fetcher.fetches("api.github.com");

    publish(&f, "KDE/kdenlive", "v2.0", "2.0", true);
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["updates"]);
    assert_eq!(io.out, "Kdenlive\t1.0\t2.0\tupdate available\n");
    assert_eq!(
        f.fetcher.fetches("api.github.com"),
        api_calls,
        "checking for updates uses the redirect, not the API"
    );

    let mut io = TestIo::answering(&["y"]);
    assert_eq!(f.run(&mut io, &["update"]), 0, "{}", io.err);
    assert_eq!(
        f.fetcher.fetches("api.github.com"),
        api_calls + 1,
        "the API is used only when there is something to fetch"
    );
    assert!(
        io.out.contains("1 update(s) available") && io.out.contains("verified"),
        "{}",
        io.out
    );
    assert!(
        io.out.contains("Upgraded Kdenlive 1.0 -> 2.0"),
        "{}",
        io.out
    );
    let a = &f.state().apps["kdenlive"];
    assert_eq!(
        (
            a.version.as_str(),
            a.tag.as_deref(),
            a.previous.as_ref().unwrap().version.as_str()
        ),
        ("2.0", Some("v2.0"), "1.0")
    );
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["update"]);
    assert!(io.out.contains("Everything is up to date."), "{}", io.out);
}

#[test]
fn a_locally_installed_app_with_embedded_update_info_can_be_updated() {
    let f = Fixture::new();
    let mut s = Spec::new("Kdenlive-1.0-x86_64.AppImage", "Kdenlive");
    s.update_info =
        Some("gh-releases-zsync|KDE|kdenlive|latest|Kdenlive-*x86_64.AppImage.zsync".into());
    f.yes(&["install", f.download(&s).to_str().unwrap()]);
    let src = f.state().apps["kdenlive"].source.clone().unwrap();
    assert_eq!(
        (src.repo.as_str(), src.pattern.as_str()),
        ("KDE/kdenlive", "Kdenlive-*x86_64.AppImage")
    );
    publish(&f, "KDE/kdenlive", "v2.0", "2.0", true);
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 0, "{}", io.err);
    assert_eq!(f.state().apps["kdenlive"].version, "2.0");
}

#[test]
fn apps_without_a_source_say_so_instead_of_pretending() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["updates"]);
    assert_eq!(io.out, "Obsidian\t1.0\t-\tno update source\n");
    let (code, io) = f.yes(&["update", "obsidian"]);
    assert_eq!(code, 1);
    assert!(
        io.err.contains("no update source") && io.err.contains("ifhai install owner/repo"),
        "{}",
        io.err
    );
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 0);
    assert!(
        io.err.contains("No update source for: Obsidian"),
        "{}",
        io.err
    );
}

#[test]
fn a_dead_network_is_an_error_not_up_to_date() {
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v1.0", "1.0", true);
    f.yes(&["install", "KDE/kdenlive"]);
    f.fetcher.down.lock().unwrap().push("KDE/kdenlive".into());
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 1);
    assert!(io.err.contains("Could not check for updates"), "{}", io.err);
    assert!(!io.out.contains("up to date"));
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["updates"]), 1);
}

#[test]
fn a_bad_update_download_keeps_the_working_version() {
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v1.0", "1.0", true);
    f.yes(&["install", "KDE/kdenlive"]);
    let good = std::fs::read(f.dirs.apps.join("kdenlive.AppImage")).unwrap();
    let spec = publish(&f, "KDE/kdenlive", "v2.0", "2.0", true);
    f.fetcher.files.lock().unwrap().insert(
        format!(
            "https://github.com/KDE/kdenlive/releases/download/v2.0/{}",
            spec.file_name
        ),
        b"corrupt".to_vec(),
    );
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 1);
    assert!(
        io.err.contains("Could not update Kdenlive") && io.err.contains("checksum"),
        "{}",
        io.err
    );
    assert_eq!(
        std::fs::read(f.dirs.apps.join("kdenlive.AppImage")).unwrap(),
        good
    );
    assert_eq!(f.state().apps["kdenlive"].version, "1.0");
}

// ---- doctor ------------------------------------------------------------------------------------

#[test]
fn doctor_reports_a_healthy_setup() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["doctor"]), 0, "{}", io.out);
    assert!(
        io.out.contains("[ok] FUSE 2 is installed")
            && io.out.contains("[ok] Obsidian 1.0 is intact.")
            && io.out.contains("is on your PATH"),
        "{}",
        io.out
    );
}

#[test]
fn doctor_finds_missing_files_duplicates_strays_and_environment_problems() {
    let mut f = Fixture::new();
    f.host.fuse2 = false;
    f.host.userns_restricted = true;
    f.host.path_var = Some("/usr/bin".into());
    let mut s = Spec::new("Obsidian-1.0.AppImage", "Obsidian");
    s.electron = true;
    f.yes(&["install", f.download(&s).to_str().unwrap()]);
    assert!(f.state().apps["obsidian"].electron);
    std::fs::remove_file(f.dirs.bin.join("obsidian")).unwrap();
    // A second menu entry made by some other tool, plus an unrelated AppImage entry.
    std::fs::write(
        f.dirs
            .applications
            .join("obsidian-appimagelauncher.desktop"),
        "[Desktop Entry]\nName=Obsidian\nExec=/home/u/Downloads/Obsidian.AppImage\n",
    )
    .unwrap();
    std::fs::write(
        f.dirs.applications.join("other.desktop"),
        "[Desktop Entry]\nName=Other Thing\nExec=/home/u/other.appimage %U\n",
    )
    .unwrap();
    std::fs::write(
        f.dirs.applications.join("firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\nExec=firefox\n",
    )
    .unwrap();
    std::fs::write(f.dirs.apps.join("Stray-3.AppImage"), "x").unwrap();
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["doctor"]), 1);
    let o = &io.out;
    assert!(o.contains("[warn] FUSE 2 is not installed"), "{o}");
    assert!(
        o.contains("[warn]") && o.contains("is not on your PATH"),
        "{o}"
    );
    assert!(
        o.contains("[problem] Obsidian is missing its command"),
        "{o}"
    );
    assert!(
        o.contains("[problem] Obsidian has a second menu entry that ifhai did not create"),
        "{o}"
    );
    assert!(
        o.contains("1 other AppImage menu entry is not managed by ifhai: Other Thing"),
        "{o}"
    );
    assert!(
        !o.contains("Firefox"),
        "non-AppImage entries are none of our business: {o}"
    );
    assert!(
        o.contains("Stray-3.AppImage") && o.contains("ifhai install"),
        "{o}"
    );
    assert!(
        o.contains("restricts unprivileged user namespaces") && o.contains("Obsidian"),
        "{o}"
    );
    assert!(
        io.err.is_empty(),
        "doctor's exit code speaks for itself: {}",
        io.err
    );
    assert!(
        f.dirs
            .applications
            .join("obsidian-appimagelauncher.desktop")
            .exists(),
        "doctor never deletes anything"
    );
}

// ---- presentation & misc --------------------------------------------------------------------------

#[test]
fn terminal_output_is_styled_and_pipes_stay_plain() {
    let f = Fixture::new();
    let src = obsidian(&f, "1.0");
    let mut io = TestIo::term(&["y"]);
    assert_eq!(
        f.run(&mut io, &["install", src.to_str().unwrap()]),
        0,
        "{}",
        io.err
    );
    assert!(io.out.contains("\x1b["));
    let p = strip(&io.out);
    assert!(
        p.contains("┃ Obsidian 1.0")
            && p.contains("✓ Installed Obsidian 1.0")
            && p.contains("→ Installing Obsidian 1.0…"),
        "{p}"
    );
    assert_eq!(strip(&io.asked[0]), "Install Obsidian 1.0? [Y/n] ");
    let f2 = Fixture::new();
    let mut plain = TestIo::non_interactive();
    f2.run(
        &mut plain,
        &["install", obsidian(&f2, "1.0").to_str().unwrap(), "--yes"],
    );
    assert!(
        !plain.out.contains('\x1b') && plain.out.is_ascii(),
        "{}",
        plain.out
    );
}

#[test]
fn errors_and_cancel_on_a_terminal() {
    let f = Fixture::new();
    let mut io = TestIo::term(&[]);
    assert_eq!(f.run(&mut io, &["install", "nope.AppImage", "--yes"]), 3);
    assert!(
        strip(&io.err).starts_with("✗ There is no file called 'nope.AppImage'"),
        "{}",
        io.err
    );
    let mut io = TestIo::term(&["n"]);
    assert_eq!(
        f.run(&mut io, &["install", obsidian(&f, "1.0").to_str().unwrap()]),
        6
    );
    assert_eq!(strip(&io.err), "Cancelled.\n");
}

#[test]
fn a_missing_path_entry_is_called_out_after_install() {
    let mut f = Fixture::new();
    f.host.path_var = Some("/usr/bin".into());
    let (code, io) = f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(
        io.err.contains("is not on your PATH") && io.err.contains("export PATH="),
        "{}",
        io.err
    );
}

#[test]
fn damaged_state_is_reported_not_reset() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.dirs.state_file.parent().unwrap()).unwrap();
    std::fs::write(&f.dirs.state_file, "apps = [[[").unwrap();
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["list"]), 1);
    assert!(io.err.contains("damaged"), "{}", io.err);
    assert_eq!(
        std::fs::read_to_string(&f.dirs.state_file).unwrap(),
        "apps = [[["
    );
}

#[test]
fn usage_help_and_version() {
    let f = Fixture::new();
    for a in [
        &["install"][..],
        &["frobnicate"],
        &["--bogus"],
        &["update", "--keep"],
    ] {
        let mut io = TestIo::non_interactive();
        assert_eq!(f.run(&mut io, a), 2, "{a:?}");
        assert!(io.err.contains("ifhai --help"), "{}", io.err);
    }
    for a in [&[][..], &["--help"], &["help"]] {
        let mut io = TestIo::non_interactive();
        assert_eq!(f.run(&mut io, a), 0);
        assert!(
            io.out.contains("USAGE") && io.out.contains("ifhai install ~/Downloads/Some.AppImage")
        );
    }
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["--version"]);
    assert_eq!(io.out, format!("ifhai {}\n", env!("CARGO_PKG_VERSION")));
    let mut io = TestIo::term(&[]);
    f.run(&mut io, &["--help"]);
    assert_eq!(strip(&io.out), ifhai::cli::HELP);
}

#[test]
fn removing_the_last_app_leaves_no_working_folder_behind() {
    let f = Fixture::new();
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    assert!(f.dirs.support.exists());
    f.yes(&["remove", "obsidian"]);
    assert!(!f.dirs.support.exists(), "the hidden .ifhai folder is gone");
    assert!(
        f.dirs.apps.exists(),
        "but the visible Applications folder stays, it is the user's"
    );
}

#[test]
fn a_host_with_no_fusermount_gets_every_app_unpacked_even_with_a_modern_runtime() {
    let mut f = Fixture::new();
    f.host.fuse2 = false;
    f.host.fusermount = false;
    let src = obsidian(&f, "1.0");
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    let a = &f.state().apps["obsidian"];
    assert_eq!(
        a.mode, "extract",
        "a static runtime still needs fusermount to mount itself"
    );
    assert!(a.support.join("root/AppRun").exists());
    assert!(
        io.err.contains("no FUSE (no fusermount)") && io.err.contains("unpacked"),
        "{}",
        io.err
    );
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["doctor"]);
    assert!(
        io.out.contains("[warn] No FUSE (fusermount) was found"),
        "{}",
        io.out
    );
}

#[test]
fn a_verified_download_is_hashed_once_not_twice() {
    // The checksum is computed to verify the download and handed on; the install step must not
    // read the (typically 100+ MB) file again. Observable result: same state, correct hash.
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v1.0", "1.0", true);
    let (code, io) = f.yes(&["install", "KDE/kdenlive"]);
    assert_eq!(code, 0, "{}", io.err);
    let a = &f.state().apps["kdenlive"];
    assert_eq!(
        a.sha256,
        sha256(&std::fs::read(&a.file).unwrap()),
        "the recorded hash is the real one"
    );
}

// ---- under the hood ------------------------------------------------------------------------------

#[test]
fn a_file_already_in_place_is_never_copied_onto_itself() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.dirs.apps).unwrap();
    // Someone drops the download straight into ~/Applications under the name ifhai would use.
    let spec = Spec::new("obsidian.AppImage", "Obsidian");
    let bytes = f.bytes_of(&spec);
    let p = f.dirs.apps.join("obsidian.AppImage");
    std::fs::write(&p, &bytes).unwrap();
    for keep in [true, false] {
        let mut args = vec!["install", p.to_str().unwrap()];
        if keep {
            args.push("--keep");
        }
        let (code, io) = f.yes(&args);
        assert_eq!(code, 0, "keep={keep}: {}", io.err);
        assert_eq!(
            std::fs::read(&p).unwrap(),
            bytes,
            "the file must be intact (keep={keep})"
        );
    }
    assert!(f.state().apps.contains_key("obsidian"));
}

#[test]
fn the_browsers_download_url_becomes_an_exact_update_source() {
    let f = Fixture::new();
    let src = f.download(&Spec::new("Kdenlive-1.0-x86_64.AppImage", "Kdenlive"));
    f.system.origins.lock().unwrap().insert(
        "Kdenlive-1.0-x86_64.AppImage".into(),
        "https://github.com/KDE/kdenlive/releases/download/v1.0/Kdenlive-1.0-x86_64.AppImage"
            .into(),
    );
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    let a = &f.state().apps["kdenlive"];
    let s = a
        .source
        .as_ref()
        .expect("source discovered from the download URL");
    assert_eq!(
        (s.repo.as_str(), s.pattern.as_str(), a.tag.as_deref()),
        ("KDE/kdenlive", "Kdenlive-*-x86_64.AppImage", Some("v1.0"))
    );
    publish(&f, "KDE/kdenlive", "v2.0", "2.0", true);
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 0, "{}", io.err);
    assert_eq!(
        f.state().apps["kdenlive"].version,
        "2.0",
        "updated without ever being installed from GitHub by hand"
    );
}

#[test]
fn a_foreign_or_malformed_origin_is_ignored() {
    let f = Fixture::new();
    for (i, url) in [
        "https://example.org/dl/App.AppImage",
        "https://github.com/a/b/releases/download/v1/App.zip",
        "not a url",
    ]
    .iter()
    .enumerate()
    {
        let name = format!("App{i}-1.0.AppImage");
        let src = f.download(&Spec::new(&name, &format!("App{i}")));
        f.system
            .origins
            .lock()
            .unwrap()
            .insert(name, url.to_string());
        let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{}", io.err);
        assert!(f.state().apps[&format!("app{i}")].source.is_none(), "{url}");
    }
}

#[test]
fn an_install_with_ifhai_entry_appears_for_appimage_files_and_goes_with_the_last_app() {
    let f = Fixture::new();
    let entry = f.dirs.applications.join("ifhai-install.desktop");
    f.yes(&["install", obsidian(&f, "1.0").to_str().unwrap()]);
    let d = std::fs::read_to_string(&entry).unwrap();
    assert!(
        d.contains("MimeType=application/vnd.appimage;application/x-iso9660-appimage;"),
        "{d}"
    );
    assert!(
        d.contains("Terminal=true")
            && d.contains("NoDisplay=true")
            && d.contains("Name=Install with ifhai"),
        "{d}"
    );
    assert!(
        d.contains("/opt/ifhai/bin/ifhai") && d.contains("%f"),
        "{d}"
    );
    assert!(
        !d.lines()
            .any(|l| l.starts_with("MimeType") && l.contains("x-scheme")),
        "never a URL handler"
    );
    // A path with spaces and quotes must survive the Exec line.
    let exec = d.lines().find(|l| l.starts_with("Exec=")).unwrap();
    assert!(
        exec.starts_with("Exec=sh -c \"") && exec.ends_with(" ifhai-install %f"),
        "{exec}"
    );
    f.yes(&[
        "install",
        f.download(&Spec::new("Krita-5.AppImage", "Krita"))
            .to_str()
            .unwrap(),
    ]);
    f.yes(&["remove", "obsidian"]);
    assert!(
        entry.exists(),
        "still needed while another app is installed"
    );
    f.yes(&["remove", "krita"]);
    assert!(
        !entry.exists(),
        "nothing left behind once the last app is gone"
    );
}

#[test]
fn update_checks_run_side_by_side_and_keep_their_order() {
    let f = Fixture::new();
    for (i, repo) in ["o/a", "o/b", "o/c"].iter().enumerate() {
        let spec = Spec::new(&format!("App{i}-1.0-x86_64.AppImage"), &format!("App{i}"));
        let b = f.bytes_of(&spec);
        f.release(repo, "v1.0", &[(spec.file_name.as_str(), b)], true);
        f.yes(&["install", repo]);
    }
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["updates"]), 0, "{}", io.err);
    let apps: Vec<&str> = io
        .out
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(apps, vec!["App0", "App1", "App2"], "{}", io.out);
    f.fetcher.down.lock().unwrap().push("o/b".into());
    let mut io = TestIo::non_interactive();
    assert_eq!(
        f.run(&mut io, &["updates"]),
        0,
        "one failure must not hide the others"
    );
    assert!(
        io.out.contains("App1\t1.0\t?\tcheck failed")
            && io.out.contains("App0\t1.0\t1.0\tup to date"),
        "{}",
        io.out
    );
}

// ---- by name: the catalogue ------------------------------------------------------------------------

fn feed(f: &Fixture, items: &[(&str, &str, &str)]) {
    let js: Vec<String> = items
        .iter()
        .map(|(n, repo, d)| format!(r#"{{"name":"{n}","description":"{d}","links":[{{"type":"GitHub","url":"{repo}"}}]}}"#))
        .collect();
    f.fetcher.texts.lock().unwrap().insert(
        ifhai::catalogue::FEED_URL.into(),
        format!(r#"{{"version":"1","items":[{}]}}"#, js.join(",")),
    );
}

fn publish_app(f: &Fixture, repo: &str, name: &str, tag: &str, v: &str) {
    let spec = Spec::new(&format!("{name}-{v}-x86_64.AppImage"), name);
    let bytes = f.bytes_of(&spec);
    f.release(repo, tag, &[(spec.file_name.as_str(), bytes)], true);
}

#[test]
fn an_app_is_installed_by_its_plain_name() {
    let f = Fixture::new();
    feed(
        &f,
        &[
            ("Krita", "KDE/krita", "Digital painting"),
            ("Obsidian", "obsidianmd/obsidian-releases", "Notes"),
        ],
    );
    publish_app(&f, "KDE/krita", "Krita", "v5.2.0", "5.2.0");
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(f.run(&mut io, &["install", "krita"]), 0, "{}", io.err);
    assert!(
        io.out.contains("Krita 5.2.0")
            && io.out.contains("github.com/KDE/krita v5.2.0")
            && io.out.contains("SHA-256 verified"),
        "{}",
        io.out
    );
    let a = &f.state().apps["krita"];
    assert_eq!(a.source.as_ref().unwrap().repo, "KDE/krita");
    assert_eq!(f.fetcher.fetches("tag "), 0);
}

#[test]
fn the_exact_name_wins_over_longer_names() {
    let f = Fixture::new();
    feed(
        &f,
        &[
            ("Krita Next", "KDE/krita-next", "Nightly"),
            ("Krita", "KDE/krita", "Painting"),
        ],
    );
    publish_app(&f, "KDE/krita", "Krita", "v5", "5");
    publish_app(&f, "KDE/krita-next", "KritaNext", "v6", "6");
    let (code, io) = f.yes(&["install", "Krita"]);
    assert_eq!(code, 0, "no needless question: {}", io.err);
    assert_eq!(
        f.state().apps["krita"].source.as_ref().unwrap().repo,
        "KDE/krita"
    );
}

#[test]
fn dead_catalogue_entries_are_never_offered() {
    let f = Fixture::new();
    feed(
        &f,
        &[
            ("Notes", "dead/notes", "Old"),
            ("Notes", "alive/notes", "New"),
        ],
    );
    f.release(
        "dead/notes",
        "v1",
        &[("source.tar.gz", b"x".to_vec())],
        false,
    ); // no AppImage any more
    publish_app(&f, "alive/notes", "Notes", "v2", "2");
    let (code, io) = f.yes(&["install", "notes"]);
    assert_eq!(code, 0, "the dead twin is skipped silently: {}", io.err);
    assert_eq!(
        f.state().apps["notes"].source.as_ref().unwrap().repo,
        "alive/notes"
    );
}

#[test]
fn a_listed_but_dead_app_says_exactly_that() {
    let f = Fixture::new();
    feed(&f, &[("Ghost", "gone/ghost", "")]);
    f.release(
        "gone/ghost",
        "v1",
        &[("source.tar.gz", b"x".to_vec())],
        false,
    );
    let (code, io) = f.yes(&["install", "ghost"]);
    assert_eq!(code, 3);
    assert!(
        io.err
            .contains("Ghost is listed in the AppImage catalogue (github.com/gone/ghost)")
            && io.err.contains("no AppImage for x86_64")
            && io.err.contains("ifhai install owner/repo"),
        "{}",
        io.err
    );
    let (code, io) = f.yes(&["install", "no-such-thing"]);
    assert_eq!(code, 3);
    assert!(
        io.err.contains("No app called 'no-such-thing'") && io.err.contains("ifhai search"),
        "{}",
        io.err
    );
}

#[test]
fn equally_good_matches_are_asked_about_never_guessed() {
    let f = Fixture::new();
    feed(
        &f,
        &[
            ("Notes", "a/notes", "First"),
            ("Notes", "b/notes", "Second"),
        ],
    );
    publish_app(&f, "a/notes", "Notes", "v1", "1");
    publish_app(&f, "b/notes", "Notes", "v1", "1");
    let (code, io) = f.yes(&["install", "notes"]);
    assert_eq!(code, 4);
    assert!(
        io.err.contains("1. Notes (github.com/a/notes)")
            && io.err.contains("2. Notes (github.com/b/notes)")
            && io.err.contains("owner/repo"),
        "{}",
        io.err
    );
    assert!(f.state().apps.is_empty());
    let mut io = TestIo::answering(&["2", "y"]);
    assert_eq!(f.run(&mut io, &["install", "notes"]), 0, "{}", io.err);
    assert_eq!(
        f.state().apps["notes"].source.as_ref().unwrap().repo,
        "b/notes"
    );
    let f2 = Fixture::new();
    feed(&f2, &[("Notes", "a/notes", ""), ("Notes", "b/notes", "")]);
    publish_app(&f2, "a/notes", "Notes", "v1", "1");
    publish_app(&f2, "b/notes", "Notes", "v1", "1");
    let mut io = TestIo::answering(&["q"]);
    assert_eq!(f2.run(&mut io, &["install", "notes"]), 6);
}

#[test]
fn file_like_arguments_never_touch_the_catalogue() {
    let f = Fixture::new();
    for t in ["missing.AppImage", "some/dir/thing"] {
        let (code, _) = f.yes(&["install", t]);
        assert_ne!(code, 0, "{t}");
    }
    assert_eq!(
        f.fetcher.fetches("appimage.github.io"),
        0,
        "no network for something that is plainly a path"
    );
}

#[test]
fn search_lists_matches_with_their_source() {
    let f = Fixture::new();
    feed(
        &f,
        &[
            ("Krita", "KDE/krita", "Digital painting"),
            ("Pinta", "o/pinta", "Image editor like krita"),
            ("Obsidian", "x/obs", "Notes"),
        ],
    );
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["search", "krita"]), 0, "{}", io.err);
    assert_eq!(
        io.out,
        "Krita\tgithub.com/KDE/krita\tDigital painting\nPinta\tgithub.com/o/pinta\tImage editor like krita\n"
    );
    let mut io = TestIo::term(&[]);
    f.run(&mut io, &["search", "krita"]);
    assert!(
        strip(&io.out).starts_with("App")
            && strip(&io.out).contains("Install one with: ifhai install <name>"),
        "{}",
        io.out
    );
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["search", "zzzz"]), 3);
}

#[test]
fn the_catalogue_is_cached_survives_being_offline_and_ignores_garbage() {
    let f = Fixture::new();
    feed(&f, &[("Krita", "KDE/krita", "")]);
    f.run(&mut TestIo::non_interactive(), &["search", "krita"]);
    f.run(&mut TestIo::non_interactive(), &["search", "krita"]);
    assert_eq!(
        f.fetcher.fetches("appimage.github.io"),
        1,
        "second use reads the cache"
    );
    assert!(f.dirs.cache.join("catalogue.json").exists());
    // Stale + offline: the old copy is used, with a warning.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(f.dirs.cache.join("catalogue.json"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    f.fetcher
        .down
        .lock()
        .unwrap()
        .push("appimage.github.io".into());
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["search", "krita"]), 0, "{}", io.err);
    assert!(
        io.err.contains("Could not refresh the app catalogue") && io.out.contains("Krita"),
        "{} / {}",
        io.err,
        io.out
    );
    // No cache at all and offline: a clear error that points at the other ways in.
    let g = Fixture::new();
    g.fetcher
        .down
        .lock()
        .unwrap()
        .push("appimage.github.io".into());
    let mut io = TestIo::non_interactive();
    assert_eq!(g.run(&mut io, &["search", "krita"]), 1);
    assert!(
        io.err.contains("apps cannot be found by name") && io.err.contains("owner/repo"),
        "{}",
        io.err
    );
    // A captive portal's HTML is neither trusted nor cached.
    let h = Fixture::new();
    h.fetcher.texts.lock().unwrap().insert(
        ifhai::catalogue::FEED_URL.into(),
        "<html>log in</html>".into(),
    );
    assert_eq!(
        h.run(&mut TestIo::non_interactive(), &["search", "krita"]),
        1
    );
    assert!(!h.dirs.cache.join("catalogue.json").exists());
}

// ---- several candidates, asked about ----------------------------------------------------------------

#[test]
fn a_release_with_several_candidates_lets_you_choose_and_remembers_the_choice() {
    let f = Fixture::new();
    let a = Spec::new("App-1.0-x86_64.AppImage", "App");
    let b = Spec::new("App-1.0-legacy-x86_64.AppImage", "App");
    let (ab, bb) = (f.bytes_of(&a), f.bytes_of(&b));
    f.release(
        "o/app",
        "v1.0",
        &[(a.file_name.as_str(), ab), (b.file_name.as_str(), bb)],
        true,
    );
    let mut io = TestIo::answering(&["2", "y"]);
    assert_eq!(f.run(&mut io, &["install", "o/app"]), 0, "{}", io.err);
    assert!(
        io.out.contains("1. App-1.0-legacy-x86_64.AppImage")
            || io.out.contains("2. App-1.0-legacy-x86_64.AppImage"),
        "{}",
        io.out
    );
    let s = f.state().apps["app"].source.clone().unwrap();
    assert!(
        ifhai::appimage::glob(&s.pattern, "App-2.0-legacy-x86_64.AppImage"),
        "the pattern keeps the legacy variant: {}",
        s.pattern
    );
    assert!(!ifhai::appimage::glob(
        &s.pattern,
        "App-2.0-x86_64.AppImage"
    ));
}

// ---- update sources: found, verified, inherited ----------------------------------------------------------

fn with_hint(name: &str, file: &str, repo: &str) -> Spec {
    let mut s = Spec::new(file, name);
    s.metainfo = Some(format!(
        "<component><url type=\"homepage\">https://github.com/{repo}</url><url type=\"bugtracker\">https://github.com/{repo}/issues</url></component>"
    ));
    s
}

#[test]
fn a_repo_named_in_the_metadata_becomes_the_update_source_only_after_it_checks_out() {
    let f = Fixture::new();
    let src = f.download(&with_hint(
        "Kdenlive",
        "Kdenlive-1.0-x86_64.AppImage",
        "KDE/kdenlive",
    ));
    f.yes(&["install", src.to_str().unwrap()]);
    let a = f.state().apps["kdenlive"].clone();
    assert_eq!(
        a.hints,
        vec!["KDE/kdenlive"],
        "the repo is remembered as a hint only"
    );
    assert!(a.source.is_none() && !a.hints_tried);
    publish(&f, "KDE/kdenlive", "v2.0", "2.0", true);
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["updates"]), 0, "{}", io.err);
    assert_eq!(
        io.out, "Kdenlive\t1.0\t2.0\tupdate available\n",
        "found and verified in one go"
    );
    assert_eq!(
        f.state().apps["kdenlive"].source.as_ref().unwrap().pattern,
        "Kdenlive-*-x86_64.AppImage"
    );
    let (code, _) = f.yes(&["update"]);
    assert_eq!(code, 0);
    assert_eq!(f.state().apps["kdenlive"].version, "2.0");
}

#[test]
fn a_hint_that_does_not_match_is_rejected_and_never_retried() {
    let f = Fixture::new();
    let src = f.download(&with_hint(
        "Kdenlive",
        "Kdenlive-1.0-x86_64.AppImage",
        "someone/fork",
    ));
    f.yes(&["install", src.to_str().unwrap()]);
    publish_app(&f, "someone/fork", "TotallyDifferent", "v9", "9");
    let mut io = TestIo::non_interactive();
    f.run(&mut io, &["updates"]);
    assert_eq!(io.out, "Kdenlive\t1.0\t-\tno update source\n");
    let a = &f.state().apps["kdenlive"];
    assert!(a.source.is_none() && a.hints_tried, "tried once, rejected");
    let calls = f.fetcher.fetches("api.github.com");
    f.run(&mut TestIo::non_interactive(), &["updates"]);
    assert_eq!(
        f.fetcher.fetches("api.github.com"),
        calls,
        "no further API calls for this app"
    );
}

#[test]
fn network_trouble_does_not_burn_the_one_attempt() {
    let f = Fixture::new();
    let src = f.download(&with_hint(
        "Kdenlive",
        "Kdenlive-1.0-x86_64.AppImage",
        "KDE/kdenlive",
    ));
    f.yes(&["install", src.to_str().unwrap()]);
    f.fetcher.down.lock().unwrap().push("KDE/kdenlive".into());
    f.run(&mut TestIo::non_interactive(), &["updates"]);
    assert!(
        !f.state().apps["kdenlive"].hints_tried,
        "offline is not a verdict"
    );
}

#[test]
fn a_newer_local_file_keeps_following_the_same_source() {
    let f = Fixture::new();
    publish(&f, "KDE/kdenlive", "v1.0", "1.0", true);
    f.yes(&["install", "KDE/kdenlive"]);
    let newer = f.download(&Spec::new("Kdenlive-1.5-x86_64.AppImage", "Kdenlive"));
    f.yes(&["install", newer.to_str().unwrap()]);
    let a = &f.state().apps["kdenlive"];
    assert_eq!(a.version, "1.5");
    assert_eq!(
        a.source.as_ref().unwrap().repo,
        "KDE/kdenlive",
        "the update source survives installing a file by hand"
    );
}

// ---- delta updates ----------------------------------------------------------------------------------------

fn publish_with_zsync(f: &Fixture, tag: &str, v: &str) {
    let spec = Spec::new(&format!("Kdenlive-{v}-x86_64.AppImage"), "Kdenlive");
    let bytes = f.bytes_of(&spec);
    let z = format!("{}.zsync", spec.file_name);
    f.release(
        "KDE/kdenlive",
        tag,
        &[
            (spec.file_name.as_str(), bytes),
            (z.as_str(), b"zsync-control-file".to_vec()),
        ],
        true,
    );
}

#[test]
fn an_update_with_a_zsync_file_downloads_only_the_difference() {
    let f = Fixture::new();
    publish_with_zsync(&f, "v1.0", "1.0");
    f.yes(&["install", "KDE/kdenlive"]);
    publish_with_zsync(&f, "v2.0", "2.0");
    let before = f.fetcher.fetches(
        "file https://github.com/KDE/kdenlive/releases/download/v2.0/Kdenlive-2.0-x86_64.AppImage",
    );
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 0, "{}", io.err);
    assert_eq!(f.fetcher.fetches("delta https://github.com/KDE/kdenlive/releases/download/v2.0/Kdenlive-2.0-x86_64.AppImage.zsync seed=kdenlive.AppImage"), 1, "built from the installed file");
    assert_eq!(f.fetcher.fetches("file https://github.com/KDE/kdenlive/releases/download/v2.0/Kdenlive-2.0-x86_64.AppImage"), before, "no full download");
    assert_eq!(f.state().apps["kdenlive"].version, "2.0");
}

#[test]
fn without_zsync_or_when_it_fails_the_update_is_a_normal_download() {
    let f = Fixture::new();
    publish_with_zsync(&f, "v1.0", "1.0");
    f.yes(&["install", "KDE/kdenlive"]);
    publish_with_zsync(&f, "v2.0", "2.0");
    *f.fetcher.no_delta.lock().unwrap() = true;
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 0, "{}", io.err);
    assert_eq!(f.state().apps["kdenlive"].version, "2.0");
    assert_eq!(f.fetcher.fetches("file https://github.com/KDE/kdenlive/releases/download/v2.0/Kdenlive-2.0-x86_64.AppImage"), 1);
}

#[test]
fn a_delta_result_that_fails_its_checksum_is_replaced_by_a_full_download() {
    let f = Fixture::new();
    publish_with_zsync(&f, "v1.0", "1.0");
    f.yes(&["install", "KDE/kdenlive"]);
    publish_with_zsync(&f, "v2.0", "2.0");
    *f.fetcher.corrupt_delta.lock().unwrap() = Some(b"not the real file".to_vec());
    let (code, io) = f.yes(&["update"]);
    assert_eq!(code, 0, "{}", io.err);
    assert_eq!(f.state().apps["kdenlive"].version, "2.0");
    let installed = std::fs::read(f.dirs.apps.join("kdenlive.AppImage")).unwrap();
    assert_ne!(
        installed, b"not the real file",
        "the corrupt delta output was never installed"
    );
    assert_eq!(f.fetcher.fetches("file https://github.com/KDE/kdenlive/releases/download/v2.0/Kdenlive-2.0-x86_64.AppImage"), 1, "fell back to the full file");
}

#[test]
fn a_cache_from_an_older_format_is_refetched_not_misread() {
    let f = Fixture::new();
    feed(&f, &[("Krita", "KDE/krita", "")]);
    std::fs::create_dir_all(&f.dirs.cache).unwrap();
    // What the first version wrote: a bare array, with markup left in the text.
    std::fs::write(
        f.dirs.cache.join("catalogue.json"),
        r#"[{"name":"Old","description":"<p>x</p>","repo":"o/old"}]"#,
    )
    .unwrap();
    let mut io = TestIo::non_interactive();
    assert_eq!(f.run(&mut io, &["search", "krita"]), 0, "{}", io.err);
    assert_eq!(f.fetcher.fetches("appimage.github.io"), 1, "refetched");
    assert!(io.out.contains("Krita") && !io.out.contains("Old"));
}

// ---- never take over what ifhai did not create ------------------------------------------------

#[test]
fn a_file_already_in_the_bin_folder_keeps_its_name_and_the_app_gets_another() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.dirs.bin).unwrap();
    std::fs::write(f.dirs.bin.join("obsidian"), "the user's own script").unwrap();
    let src = obsidian(&f, "1.8.9");
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    assert_eq!(
        std::fs::read_to_string(f.dirs.bin.join("obsidian")).unwrap(),
        "the user's own script"
    );
    assert_eq!(
        f.state().apps["obsidian"].launcher,
        f.dirs.bin.join("ifhai-obsidian")
    );
    assert!(
        io.out.contains("run: ifhai-obsidian") && io.err.contains("already exists"),
        "{}{}",
        io.out,
        io.err
    );
    // An upgrade keeps the same command, and removal leaves the user's file alone.
    let newer = obsidian(&f, "1.9.0");
    assert_eq!(f.yes(&["install", newer.to_str().unwrap()]).0, 0);
    assert_eq!(
        f.state().apps["obsidian"].launcher,
        f.dirs.bin.join("ifhai-obsidian")
    );
    assert_eq!(f.yes(&["remove", "obsidian"]).0, 0);
    assert!(f.dirs.bin.join("obsidian").exists());
    assert!(!f.dirs.bin.join("ifhai-obsidian").exists());
}

#[test]
fn an_app_named_after_a_system_command_does_not_take_it_over() {
    let f = Fixture::new();
    let src = f.download(&Spec::new("sh-1.0.AppImage", "sh"));
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 0, "{}", io.err);
    assert!(!f.dirs.bin.join("sh").exists());
    assert!(f.dirs.bin.join("ifhai-sh").exists());
}

#[test]
fn an_app_file_ifhai_did_not_install_is_not_replaced() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.dirs.apps).unwrap();
    std::fs::write(f.dirs.apps.join("obsidian.AppImage"), "saved by hand").unwrap();
    let src = obsidian(&f, "1.8.9");
    let (code, io) = f.yes(&["install", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(io.err.contains("did not install it"), "{}", io.err);
    assert_eq!(
        std::fs::read_to_string(f.dirs.apps.join("obsidian.AppImage")).unwrap(),
        "saved by hand"
    );
    assert!(src.exists() && f.state().apps.is_empty());
}

#[test]
fn an_unverified_update_does_not_hold_back_the_verified_ones() {
    let f = Fixture::new();
    publish_app(&f, "o/safe", "Safe", "v1.0", "1.0");
    assert_eq!(f.yes(&["install", "o/safe"]).0, 0);
    let loose = |tag: &str, v: &str| {
        let spec = Spec::new(&format!("Loose-{v}-x86_64.AppImage"), "Loose");
        let bytes = f.bytes_of(&spec);
        f.release("o/loose", tag, &[(spec.file_name.as_str(), bytes)], false);
    };
    loose("v1.0", "1.0");
    let mut io = TestIo::answering(&["y"]);
    assert_eq!(f.run(&mut io, &["install", "o/loose"]), 0, "{}", io.err);
    publish_app(&f, "o/safe", "Safe", "v2.0", "2.0");
    loose("v2.0", "2.0");

    // Unattended: the checked update happens, the other is reported and left alone.
    let (code, io) = f.yes(&["update"]);
    assert_ne!(code, 0);
    assert!(
        io.err.contains("Loose") && io.err.contains("was skipped"),
        "{}",
        io.err
    );
    assert_eq!(f.state().apps["safe"].version, "2.0");
    assert_eq!(f.state().apps["loose"].version, "1.0");

    // At a terminal each unverified app is asked about; Enter means no, and that is not a failure.
    let mut no = TestIo::answering(&[""]);
    assert_eq!(f.run(&mut no, &["update", "--yes"]), 0, "{}", no.err);
    assert!(no.asked[0].contains("[y/N]"), "{:?}", no.asked);
    assert_eq!(f.state().apps["loose"].version, "1.0");
    let mut yes = TestIo::answering(&["y"]);
    assert_eq!(f.run(&mut yes, &["update"]), 0, "{}", yes.err);
    assert_eq!(f.state().apps["loose"].version, "2.0");
}
