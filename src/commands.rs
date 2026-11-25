use crate::appimage;
use crate::catalogue;
use crate::cli::{self, Command, HELP, Invocation, Opts};
use crate::error::{Error, Result};
use crate::fetch::Fetcher;
use crate::github::{self, Asset};
use crate::install::{self, Ctx, Origin, Outcome};
use crate::output::{self, Style};
use crate::state::{App, Dirs, Source, State};
use crate::system::{Host, System};
use crate::version;
use std::path::{Path, PathBuf};

pub trait Io {
    fn out(&mut self, s: &str);
    fn err(&mut self, s: &str);
    fn ask(&mut self, prompt: &str) -> Option<String>;
    fn interactive(&self) -> bool;
    fn stdout_is_tty(&self) -> bool;
    fn color_allowed(&self) -> bool;
    fn stderr_is_tty(&self) -> bool;
    fn stderr_color_allowed(&self) -> bool;
    fn unicode(&self) -> bool;
}

pub struct Env<'a> {
    pub fetcher: &'a dyn Fetcher,
    pub system: &'a dyn System,
    pub dirs: &'a Dirs,
    pub host: &'a Host,
    pub io: &'a mut dyn Io,
    /// This executable: the "Install with ifhai" entry runs it.
    pub self_exe: PathBuf,
}

fn out_style(env: &Env, no_color: bool) -> Style {
    let fancy = env.io.stdout_is_tty();
    Style {
        fancy,
        color: fancy && !no_color && env.io.color_allowed(),
        unicode: env.io.unicode(),
    }
}

fn err_style(env: &Env) -> Style {
    let fancy = env.io.stderr_is_tty();
    Style {
        fancy,
        color: fancy && env.io.stderr_color_allowed(),
        unicode: env.io.unicode(),
    }
}

fn notes(env: &mut Env, list: &[String]) {
    let st = err_style(env);
    for n in list {
        env.io.err(&output::note(st, n));
    }
}

fn confirm(env: &mut Env, st: Style, question: &str, skip: bool) -> Result<()> {
    if skip {
        return Ok(());
    }
    if !env.io.interactive() {
        return Err(Error::NeedsConfirmation("Confirmation is required but input is not a terminal.\n\nRe-run with --yes to proceed without asking.".into()));
    }
    match env
        .io
        .ask(&output::prompt(st, question, true))
        .as_deref()
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("") | Some("y") | Some("yes") => Ok(()),
        _ => Err(Error::Aborted),
    }
}

fn combine(mut problems: Vec<Error>) -> Error {
    if problems.len() == 1 {
        return problems.remove(0);
    }
    let msg = problems
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n\n");
    match problems[0] {
        Error::NotFound(_) => Error::NotFound(msg),
        Error::Ambiguous(_) => Error::Ambiguous(msg),
        _ => Error::Failed(msg),
    }
}

/// `/home/u/Applications/x` -> `~/Applications/x`.
fn tilde(env: &Env, p: &Path) -> String {
    match env.dirs.apps.parent().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

fn path_note(env: &Env) -> Option<String> {
    let path = env.host.path_var.as_deref()?;
    if std::env::split_paths(path).any(|p| p == env.dirs.bin) {
        return None;
    }
    // `~` is not expanded inside double quotes, so the command must say $HOME.
    let for_shell = match env
        .dirs
        .apps
        .parent()
        .and_then(|h| env.dirs.bin.strip_prefix(h).ok())
    {
        Some(rest) => format!("$HOME/{}", rest.display()),
        None => env.dirs.bin.display().to_string(),
    };
    Some(format!(
        "{} is not on your PATH, so the new command will not be found by name (the app menu entry works anyway). Add it in your shell profile:\n  export PATH=\"{for_shell}:$PATH\"",
        tilde(env, &env.dirs.bin),
    ))
}

pub fn execute(env: &mut Env, inv: Invocation) -> Result<()> {
    let Invocation { cmd, opts } = inv;
    match cmd {
        Command::Help => {
            let st = out_style(env, opts.no_color);
            env.io.out(&output::help(HELP, st));
            Ok(())
        }
        Command::Version => {
            env.io
                .out(&format!("ifhai {}\n", env!("CARGO_PKG_VERSION")));
            Ok(())
        }
        Command::Install(t) => install(env, &opts, &t),
        Command::Remove(n) => remove(env, &opts, &n),
        Command::List => list(env, &opts),
        Command::Run(app, args) => run_app(env, &app, &args),
        Command::Updates => updates(env, &opts),
        Command::Update(n) => update(env, &opts, &n),
        Command::Rollback(n) => rollback(env, &opts, &n),
        Command::Search(q) => search(env, &opts, &q),
        Command::Doctor => doctor(env, &opts),
    }
}

// ---- install ---------------------------------------------------------------------------------

#[allow(clippy::large_enum_variant)] // a handful of short-lived values per run
enum Job {
    Local {
        path: PathBuf,
        title: String,
    },
    Remote {
        title: String,
        from: String,
        url: String,
        file_name: String,
        size: u64,
        expect: Option<String>,
        origin: Origin,
        /// (zsync URL, the installed file to build from): used when `zsync` is installed.
        delta: Option<(String, PathBuf)>,
    },
}

fn looks_like_repo(s: &str) -> bool {
    let mut p = s.split('/');
    let ok = |x: &str| {
        !x.is_empty()
            && x.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && x != ".."
    };
    matches!((p.next(), p.next(), p.next()), (Some(a), Some(b), None) if ok(a) && ok(b))
}

fn safe_file_name(s: &str) -> Option<String> {
    let n = s
        .rsplit('/')
        .next()
        .unwrap_or(s)
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let ok = !n.is_empty()
        && !n.starts_with(['.', '-'])
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c));
    (ok && n.to_lowercase().ends_with(".appimage")).then(|| n.to_string())
}

/// Let the user pick one of several equally good options. Never guesses: without a terminal this
/// is an error that lists them.
fn choose(env: &mut Env, head: &str, labels: &[String], tail: &str) -> Result<usize> {
    let list: String = labels
        .iter()
        .enumerate()
        .map(|(i, l)| format!("  {}. {l}\n", i + 1))
        .collect();
    if !env.io.interactive() {
        return Err(Error::Ambiguous(format!("{head}\n\n{list}\n{tail}")));
    }
    env.io.out(&format!("{head}\n\n{list}\n"));
    for _ in 0..3 {
        let Some(ans) = env
            .io
            .ask(&format!("Choose one [1-{}, q to cancel]: ", labels.len()))
        else {
            break;
        };
        if ans.is_empty() || ans.eq_ignore_ascii_case("q") {
            return Err(Error::Aborted);
        }
        if let Ok(n) = ans.parse::<usize>()
            && (1..=labels.len()).contains(&n)
        {
            return Ok(n - 1);
        }
        env.io.out("Not a valid choice.\n");
    }
    Err(Error::Aborted)
}

/// Find an app by name in the catalogue. Every close candidate is checked against GitHub first:
/// the catalogue is full of projects that stopped publishing, and none of those is offered.
fn resolve_name(env: &mut Env, name: &str) -> Result<(String, String)> {
    let (entries, note) = catalogue::load(env.fetcher, &env.dirs.cache)?;
    if let Some(n) = note {
        notes(env, &[n]);
    }
    let hits = catalogue::search(&entries, name);
    let Some(&(top, _)) = hits.first() else {
        return Err(Error::NotFound(format!(
            "No app called '{name}' in the AppImage catalogue.\n\nTry: ifhai search {name}\nor install it from its GitHub page: ifhai install owner/repo"
        )));
    };
    let tier: Vec<&catalogue::Entry> = hits
        .iter()
        .take_while(|(s, _)| *s == top)
        .take(5)
        .map(|(_, e)| *e)
        .collect();
    let mut alive: Vec<&catalogue::Entry> = Vec::new();
    for e in &tier {
        match github::latest_release(env.fetcher, &e.repo) {
            Ok(rel) if !github::candidates(&rel.assets, &env.host.arch, None).is_empty() => {
                alive.push(e)
            }
            Ok(_) | Err(Error::NotFound(_)) => {}
            Err(other) => return Err(other),
        }
    }
    match alive.as_slice() {
        [] => Err(Error::NotFound(format!(
            "{} is listed in the AppImage catalogue (github.com/{}), but its latest release has no AppImage for {} (it may have moved or stopped publishing).\n\nIf you know where it lives now: ifhai install owner/repo",
            tier[0].name, tier[0].repo, env.host.arch
        ))),
        [one] => Ok((one.repo.clone(), one.name.clone())),
        many => {
            let labels: Vec<String> = many
                .iter()
                .map(|e| {
                    format!(
                        "{} (github.com/{}){}",
                        e.name,
                        e.repo,
                        if e.description.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", e.description)
                        }
                    )
                })
                .collect();
            let i = choose(
                env,
                &format!("Several apps match '{name}':"),
                &labels,
                "Be more specific, or install one directly: ifhai install owner/repo",
            )?;
            Ok((many[i].repo.clone(), many[i].name.clone()))
        }
    }
}

fn classify_repo(env: &mut Env, repo: &str, label: Option<&str>) -> Result<Job> {
    let rel = github::latest_release(env.fetcher, repo)?;
    let asset: Asset = match github::select_asset(&rel.assets, &env.host.arch, None) {
        Ok(a) => a.clone(),
        Err(Error::Ambiguous(m)) => {
            let c = github::candidates(&rel.assets, &env.host.arch, None);
            if !env.io.interactive() {
                return Err(Error::Ambiguous(format!(
                    "{m}\n\nInstall the one you want by its download URL (right-click the file on the release page)."
                )));
            }
            let names: Vec<String> = c.iter().map(|a| a.name.clone()).collect();
            let i = choose(
                env,
                "This release has several AppImages that could be the right one:",
                &names,
                "",
            )?;
            c[i].clone()
        }
        Err(e) => return Err(e),
    };
    let expect = github::expected_sha256(env.fetcher, &asset, &rel.assets)?;
    let file_name = safe_file_name(&asset.name).ok_or_else(|| {
        Error::Failed(format!(
            "The release file name '{}' is not one ifhai will use.",
            asset.name
        ))
    })?;
    let (n, _) = appimage::name_and_version(&file_name);
    let v = version::display(&rel.tag_name).to_string();
    Ok(Job::Remote {
        title: format!("{} {v}", label.unwrap_or(&n)),
        from: format!("github.com/{repo} {}", rel.tag_name),
        url: asset.browser_download_url.clone(),
        file_name,
        size: asset.size,
        expect,
        origin: Origin {
            source: Some(Source {
                repo: repo.to_string(),
                pattern: github::pattern_from_asset(&asset.name, &rel.tag_name),
            }),
            tag: Some(rel.tag_name.clone()),
            version_hint: Some(v),
            ..Origin::default()
        },
        delta: None,
    })
}

fn classify(env: &mut Env, target: &str) -> Result<Job> {
    let path = Path::new(target);
    if path.is_dir() {
        return Err(Error::Usage(format!(
            "{target} is a folder. Give ifhai the .AppImage file itself."
        )));
    }
    if path.is_file() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (n, v) = appimage::name_and_version(&name);
        return Ok(Job::Local {
            path: path.to_path_buf(),
            title: v.map(|v| format!("{n} {v}")).unwrap_or(n),
        });
    }
    if target.starts_with("http://") {
        return Err(Error::Usage(
            "Only https:// links are accepted: a plain http:// download cannot be trusted.".into(),
        ));
    }
    if target.starts_with("https://") {
        let file_name = safe_file_name(target).ok_or_else(|| {
            Error::Usage(format!("{target} does not point at an .AppImage file."))
        })?;
        let (n, v) = appimage::name_and_version(&file_name);
        return Ok(Job::Remote {
            title: v.clone().map(|v| format!("{n} {v}")).unwrap_or(n),
            from: target.to_string(),
            url: target.to_string(),
            file_name,
            size: 0,
            expect: None,
            origin: Origin::default(),
            delta: None,
        });
    }
    if looks_like_repo(target) {
        return classify_repo(env, target, None);
    }
    let looks_like_file =
        target.to_lowercase().ends_with(".appimage") || target.contains(['/', '\\']);
    if !looks_like_file && !target.starts_with('-') && target.len() <= 100 {
        let (repo, name) = resolve_name(env, target)?;
        return classify_repo(env, &repo, Some(&name));
    }
    Err(Error::NotFound(format!(
        "There is no file called '{target}', and it is not an owner/repo or an https:// link either."
    )))
}

/// After a failed install, remove the working folders we created if they are empty. Never touches
/// a `~/Applications` that existed before.
fn tidy(dirs: &Dirs, apps_existed: bool) {
    let _ = std::fs::remove_dir(&dirs.support);
    if !apps_existed {
        let _ = std::fs::remove_dir(&dirs.apps);
    }
}

fn open_with_entry(dirs: &Dirs) -> PathBuf {
    dirs.applications.join("ifhai-install.desktop")
}

/// An "Install with ifhai" entry for AppImage files in the file manager's Open With list, so a
/// downloaded file can be installed without opening a terminal first. It does not become the
/// default handler, and it is removed together with the last app.
fn ensure_open_with(env: &Env) {
    let path = open_with_entry(env.dirs);
    if path.exists() {
        return;
    }
    let script = format!(
        "{} install \"$1\"; printf '\\nPress Enter to close. '; read _",
        install::sq(&env.self_exe.to_string_lossy())
    );
    // `%f` is a field code, not an argument: it must reach the desktop unquoted.
    let exec = format!(
        "{} %f",
        ["sh", "-c", &script, "ifhai-install"]
            .iter()
            .map(|a| crate::desktop::quote_exec_arg(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let body = format!(
        "# Managed by ifhai. Removed with the last app you remove.\n[Desktop Entry]\nType=Application\nName=Install with ifhai\nComment=Make this AppImage a real app\nExec={exec}\nTerminal=true\nNoDisplay=true\nMimeType=application/vnd.appimage;application/x-iso9660-appimage;\nCategories=Utility;\nX-IFHAI-Managed=true\n"
    );
    if std::fs::create_dir_all(&env.dirs.applications).is_ok() {
        let _ = std::fs::write(&path, body);
    }
}

fn size_text(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.0} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", b.div_ceil(1024))
    }
}

fn plan_block(env: &Env, st: Style, job: &Job, keep: bool) -> String {
    match job {
        Job::Local { path, title } => {
            let slug = appimage::slugify(
                &appimage::name_and_version(
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
                .0,
            );
            output::block(
                st,
                title,
                &[
                    ("From", tilde(env, path)),
                    (
                        "File",
                        format!(
                            "{} to {}",
                            if keep {
                                "copied"
                            } else {
                                "moved (the original is not kept)"
                            },
                            tilde(env, &env.dirs.apps)
                        ),
                    ),
                    ("Menu", "an entry with its icon".to_string()),
                    ("Command", slug),
                ],
            )
        }
        Job::Remote {
            title,
            from,
            size,
            expect,
            ..
        } => {
            let check = match expect {
                Some(_) => "SHA-256 verified against the published checksum".to_string(),
                None => "no checksum is published for this file (unverified)".to_string(),
            };
            let mut rows = vec![("From", from.clone())];
            if *size > 0 {
                rows.push(("Size", size_text(*size)));
            }
            rows.push(("Check", check));
            rows.push((
                "Install",
                format!(
                    "to {}, in your app menu with its icon",
                    tilde(env, &env.dirs.apps)
                ),
            ));
            output::block(st, title, &rows)
        }
    }
}

fn download_and_install(
    env: &mut Env,
    state: &mut State,
    job: &Job,
    keep: bool,
) -> Result<install::Done> {
    let ctx = Ctx {
        system: env.system,
        dirs: env.dirs,
        host: env.host,
    };
    match job {
        Job::Local { path, .. } => {
            let origin = match env
                .system
                .origin_url(path)
                .as_deref()
                .and_then(github::release_download_url)
            {
                Some((repo, tag, asset)) => Origin {
                    source: Some(Source {
                        repo,
                        pattern: github::pattern_from_asset(&asset, &tag),
                    }),
                    version_hint: Some(version::display(&tag).to_string()),
                    tag: Some(tag),
                    ..Origin::default()
                },
                None => Origin::default(),
            };
            install::install_file(&ctx, state, path, origin, keep)
        }
        Job::Remote {
            url,
            file_name,
            expect,
            origin,
            delta,
            ..
        } => {
            std::fs::create_dir_all(&env.dirs.support)?;
            let dir = install::temp_dir(&env.dirs.support, "dl");
            std::fs::create_dir_all(&dir)?;
            let file = dir.join(file_name);
            let result = (|| {
                // An update with a zsync file downloads only what changed, when zsync is installed. Any
                // problem simply means a normal download: the checksum below still decides.
                let mut by_delta = delta.as_ref().is_some_and(|(z, seed)| {
                    seed.exists() && env.fetcher.delta(z, seed, &file).is_ok() && file.exists()
                });
                // A delta result that does not match the published checksum is thrown away and
                // replaced by a normal download rather than failing the update.
                if by_delta
                    && let Some(want) = expect
                    && install::sha256_file(&file).ok().as_ref() != Some(want)
                {
                    by_delta = false;
                }
                if !by_delta {
                    let _ = std::fs::remove_file(&file);
                    env.fetcher.file(url, &file)?;
                }
                let mut origin = origin.clone();
                if let Some(want) = expect {
                    let got = install::sha256_file(&file)?;
                    if &got != want {
                        return Err(Error::Verify(format!(
                            "The downloaded {file_name} does not match its published checksum, so it was discarded.\n  expected {want}\n  got      {got}"
                        )));
                    }
                    origin.known_sha256 = Some(got); // verified once; do not read 120 MB again
                }
                install::install_file(&ctx, state, &file, origin, false)
            })();
            let _ = std::fs::remove_dir_all(&dir);
            result
        }
    }
}

fn report_done(env: &mut Env, st: Style, done: &install::Done) {
    let a = &done.app;
    let line = match &done.outcome {
        Outcome::Installed => format!("Installed {} {}", a.name, a.version),
        Outcome::Upgraded { from } => format!(
            "Upgraded {} {from} {} {}",
            a.name,
            st.arrow_text(),
            a.version
        ),
        Outcome::Unchanged => format!(
            "{} {} is already installed (this is the same file)",
            a.name, a.version
        ),
    };
    if st.fancy {
        let mark = if done.outcome == Outcome::Unchanged {
            st.dim("·")
        } else {
            st.ok_mark()
        };
        env.io.out(&format!("{mark} {}\n", st.bold(&line)));
    } else {
        env.io.out(&format!("{line}\n"));
    }
    if done.outcome != Outcome::Unchanged {
        env.io.out(&format!(
            "{}\n",
            st.dim(&format!(
                "  Start it from your app menu, or run: {}",
                a.slug
            ))
        ));
    }
    notes(env, &done.notes);
}

fn install(env: &mut Env, opts: &Opts, targets: &[String]) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let mut state = State::load(&env.dirs.state_file)?;
    let apps_existed = env.dirs.apps.exists();
    let (mut jobs, mut problems) = (Vec::new(), Vec::new());
    for t in targets {
        match classify(env, t) {
            Ok(j) => jobs.push(j),
            Err(e) => problems.push(e),
        }
    }
    if !problems.is_empty() {
        return Err(combine(problems));
    }
    for j in &jobs {
        env.io.out(&plan_block(env, st, j, opts.keep));
    }
    let q = if jobs.len() == 1 {
        match &jobs[0] {
            Job::Local { title, .. } | Job::Remote { title, .. } => format!("Install {title}?"),
        }
    } else {
        format!("Install {} apps?", jobs.len())
    };
    confirm(env, st, &q, opts.yes)?;
    let (mut ok, mut failed) = (0, Vec::new());
    for j in &jobs {
        let title = match j {
            Job::Local { title, .. } | Job::Remote { title, .. } => title.clone(),
        };
        if st.fancy {
            env.io
                .out(&format!("{} Installing {title}…\n", st.step_mark()));
        }
        match download_and_install(env, &mut state, j, opts.keep) {
            Ok(done) => {
                state.save(&env.dirs.state_file)?;
                report_done(env, st, &done);
                ok += 1;
            }
            Err(e) => {
                tidy(env.dirs, apps_existed);
                failed.push((title, e.to_string()));
            }
        }
    }
    if ok > 0 {
        ensure_open_with(env);
        if let Some(n) = path_note(env) {
            notes(env, &[n]);
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    let m = failed
        .iter()
        .map(|(n, e)| format!("Could not install {n}.\n{e}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Err(if ok == 0 {
        Error::Failed(m)
    } else {
        Error::Partial(format!(
            "Partly completed: {ok} installed, {} failed.\n\n{m}",
            failed.len()
        ))
    })
}

fn search(env: &mut Env, opts: &Opts, query: &str) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let (entries, note) = catalogue::load(env.fetcher, &env.dirs.cache)?;
    if let Some(n) = note {
        notes(env, &[n]);
    }
    let hits = catalogue::search(&entries, query);
    if hits.is_empty() {
        return Err(Error::NotFound(format!(
            "Nothing in the AppImage catalogue matches '{query}'."
        )));
    }
    let shown = &hits[..hits.len().min(15)];
    let clip = |s: &str| {
        if s.chars().count() > 60 {
            format!("{}…", s.chars().take(59).collect::<String>())
        } else {
            s.to_string()
        }
    };
    let rows: Vec<Vec<String>> = shown
        .iter()
        .map(|(_, e)| {
            vec![
                e.name.clone(),
                format!("github.com/{}", e.repo),
                clip(&e.description),
            ]
        })
        .collect();
    let pretty = env.io.stdout_is_tty();
    env.io.out(&output::table(
        &["App", "From", "About"],
        &rows,
        pretty,
        st,
        &|c, s| match c {
            0 => st.bold(s),
            1 => st.accent(s),
            _ => st.dim(s),
        },
    ));
    if hits.len() > shown.len() {
        env.io.out(&format!(
            "{}\n",
            st.dim(&format!(
                "Showing {} of {}. Narrow the search to see others.",
                shown.len(),
                hits.len()
            ))
        ));
    }
    if pretty {
        env.io.out(&format!(
            "{}\n",
            st.dim("Install one with: ifhai install <name>")
        ));
    }
    Ok(())
}

// ---- remove / list / run ------------------------------------------------------------------------

fn remove(env: &mut Env, opts: &Opts, names: &[String]) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let mut state = State::load(&env.dirs.state_file)?;
    let (mut targets, mut problems): (Vec<App>, Vec<Error>) = (vec![], vec![]);
    for n in names {
        match state.find(n) {
            Ok(a) if !targets.iter().any(|t| t.slug == a.slug) => targets.push(a.clone()),
            Ok(_) => {}
            Err(e) => problems.push(e),
        }
    }
    if !problems.is_empty() {
        return Err(combine(problems));
    }
    let blocks: String = targets
        .iter()
        .map(|a| {
            output::block(
                st,
                &a.name,
                &[
                    ("Version", a.version.clone()),
                    ("Delete", tilde(env, &a.file)),
                    ("Also", "its menu entry, icon and command".into()),
                ],
            )
        })
        .collect();
    env.io.out(&blocks);
    env.io.out(&format!(
        "{}\n\n",
        st.dim("The app's own settings and data are not touched.")
    ));
    let q = if targets.len() == 1 {
        format!("Remove {}?", targets[0].name)
    } else {
        format!("Remove {} apps?", targets.len())
    };
    confirm(env, st, &q, opts.yes)?;
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for a in targets {
        match install::remove_files(env.dirs, &a) {
            Ok(()) => {
                state.apps.remove(&a.slug);
                state.save(&env.dirs.state_file)?;
                done.push(a.name);
            }
            Err(e) => failed.push((a.name, e.to_string())),
        }
    }
    if !done.is_empty() {
        let _ = std::fs::remove_dir(&env.dirs.support); // only succeeds when it is empty
        if state.apps.is_empty() {
            let _ = std::fs::remove_file(open_with_entry(env.dirs));
        }
        env.system.refresh_desktop(&env.dirs.applications);
        env.io.out(&output::done_lines(st, "Removed", &done));
    }
    if failed.is_empty() {
        return Ok(());
    }
    let m = failed
        .iter()
        .map(|(n, e)| format!("Could not remove {n}.\n{e}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Err(if done.is_empty() {
        Error::Failed(m)
    } else {
        Error::Partial(m)
    })
}

fn list(env: &mut Env, opts: &Opts) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let state = State::load(&env.dirs.state_file)?;
    if state.apps.is_empty() {
        env.io
            .err("Nothing installed yet. Try: ifhai install ~/Downloads/<file>.AppImage\n");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = state
        .apps
        .values()
        .map(|a| {
            vec![
                a.name.clone(),
                a.version.clone(),
                if a.mode == "extract" {
                    "unpacked".into()
                } else {
                    "AppImage".into()
                },
                a.source
                    .as_ref()
                    .map(|s| format!("github.com/{}", s.repo))
                    .unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    let pretty = env.io.stdout_is_tty();
    env.io.out(&output::table(
        &["App", "Version", "Runs as", "Updates from"],
        &rows,
        pretty,
        st,
        &|c, s| if c == 0 { st.bold(s) } else { st.dim(s) },
    ));
    Ok(())
}

fn run_app(env: &mut Env, name: &str, args: &[String]) -> Result<()> {
    let state = State::load(&env.dirs.state_file)?;
    let app = state.find(name)?;
    match env.system.launch(&app.launcher, args)? {
        0 => Ok(()),
        c => Err(Error::Child(c)),
    }
}

// ---- updates ------------------------------------------------------------------------------------

/// Turn the repos named in an app's own metadata into a real update source, but only when the repo's
/// latest release contains a file matching what was installed. Each app is tried at most once.
fn discover_sources(env: &mut Env, state: &mut State) -> bool {
    let slugs: Vec<String> = state
        .apps
        .iter()
        .filter(|(_, a)| a.source.is_none() && !a.hints.is_empty() && !a.hints_tried)
        .map(|(k, _)| k.clone())
        .collect();
    let mut changed = false;
    for slug in slugs {
        let app = state.apps[&slug].clone();
        let (mut found, mut network_trouble) = (None, false);
        for hint in &app.hints {
            match github::latest_release(env.fetcher, hint) {
                Ok(rel) => {
                    if let Some(pattern) = github::verify_hint(
                        &rel.assets,
                        &env.host.arch,
                        &app.origin_name,
                        &app.version,
                    ) {
                        found = Some(Source {
                            repo: hint.clone(),
                            pattern,
                        });
                        break;
                    }
                }
                Err(Error::NotFound(_)) => {}
                Err(_) => {
                    network_trouble = true;
                    break;
                }
            }
        }
        if let Some(a) = state.apps.get_mut(&slug) {
            match found {
                Some(s) => {
                    a.source = Some(s);
                    changed = true;
                }
                None if !network_trouble => {
                    a.hints_tried = true; // do not spend an API call on this again
                    changed = true;
                }
                None => {}
            }
        }
    }
    changed
}

struct Check {
    app: App,
    latest: Result<Option<String>>,
}

impl Check {
    fn current(&self) -> String {
        self.app
            .tag
            .clone()
            .unwrap_or_else(|| self.app.version.clone())
    }
    fn newer(&self) -> Option<&str> {
        match &self.latest {
            Ok(Some(t)) if version::is_newer(t, &self.current()) => Some(t),
            _ => None,
        }
    }
}

fn check(env: &mut Env, apps: Vec<App>) -> Vec<Check> {
    let apps: Vec<App> = apps.into_iter().filter(|a| a.source.is_some()).collect();
    let fetcher = env.fetcher;
    let mut out = Vec::new();
    // Each check is a network round trip: run them side by side (eight at a time).
    for chunk in apps.chunks(8) {
        let latest: Vec<Result<Option<String>>> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|a| {
                    let repo = a
                        .source
                        .as_ref()
                        .map(|s| s.repo.clone())
                        .unwrap_or_default();
                    s.spawn(move || fetcher.latest_tag(&repo))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(Error::Network("the update check crashed".into())))
                })
                .collect()
        });
        out.extend(
            chunk
                .iter()
                .cloned()
                .zip(latest)
                .map(|(app, latest)| Check { app, latest }),
        );
    }
    out
}

fn updates(env: &mut Env, opts: &Opts) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let mut state = State::load(&env.dirs.state_file)?;
    if state.apps.is_empty() {
        env.io.err("Nothing installed yet.\n");
        return Ok(());
    }
    if discover_sources(env, &mut state) {
        state.save(&env.dirs.state_file)?;
    }
    let checks = check(env, state.apps.values().cloned().collect());
    let rows: Vec<Vec<String>> = state
        .apps
        .values()
        .map(|a| {
            let c = checks.iter().find(|c| c.app.slug == a.slug);
            let (latest, status) = match c.map(|c| (&c.latest, c)) {
                None => ("-".into(), "no update source"),
                Some((Err(_), _)) => ("?".into(), "check failed"),
                Some((Ok(None), _)) => ("-".into(), "no releases"),
                Some((Ok(Some(t)), c)) => (
                    version::display(t).to_string(),
                    if c.newer().is_some() {
                        "update available"
                    } else {
                        "up to date"
                    },
                ),
            };
            vec![
                a.name.clone(),
                a.version.clone(),
                latest,
                status.to_string(),
            ]
        })
        .collect();
    let pretty = env.io.stdout_is_tty();
    env.io.out(&output::table(
        &["App", "Installed", "Latest", "Status"],
        &rows,
        pretty,
        st,
        &|c, s| match (c, s) {
            (0, _) => st.bold(s),
            (3, "update available") => st.yellow(s),
            (3, "up to date") => st.green(s),
            _ => st.dim(s),
        },
    ));
    let errs: Vec<String> = checks
        .iter()
        .filter_map(|c| {
            c.latest.as_ref().err().map(|e| {
                format!(
                    "Could not check {}: {}",
                    c.app.name,
                    e.to_string().lines().next().unwrap_or("")
                )
            })
        })
        .collect();
    notes(env, &errs);
    if !checks.is_empty() && errs.len() == checks.len() {
        return Err(Error::Network(
            "Could not check for updates. Check your network connection.".into(),
        ));
    }
    Ok(())
}

fn update(env: &mut Env, opts: &Opts, names: &[String]) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let mut state = State::load(&env.dirs.state_file)?;
    if discover_sources(env, &mut state) {
        state.save(&env.dirs.state_file)?;
    }
    let mut chosen: Vec<App> = Vec::new();
    if names.is_empty() {
        chosen = state.apps.values().cloned().collect();
    } else {
        let mut problems = Vec::new();
        for n in names {
            match state.find(n) {
                Ok(a) if a.source.is_none() => problems.push(Error::Failed(format!("{} has no update source, so ifhai cannot tell where its newer versions are.\nIf it came from GitHub, install it from there once: ifhai install owner/repo", a.name))),
                Ok(a) => chosen.push(a.clone()),
                Err(e) => problems.push(e),
            }
        }
        if !problems.is_empty() {
            return Err(combine(problems));
        }
    }
    let without: Vec<String> = chosen
        .iter()
        .filter(|a| a.source.is_none())
        .map(|a| a.name.clone())
        .collect();
    if names.is_empty() && !without.is_empty() {
        env.io.err(&format!(
            "{}\n",
            st.dim(&format!("No update source for: {}", without.join(", ")))
        ));
    }
    let checks = check(env, chosen);
    let errs: Vec<String> = checks
        .iter()
        .filter_map(|c| {
            c.latest.as_ref().err().map(|e| {
                format!(
                    "Could not check {}: {}",
                    c.app.name,
                    e.to_string().lines().next().unwrap_or("")
                )
            })
        })
        .collect();
    notes(env, &errs);
    if !checks.is_empty() && errs.len() == checks.len() {
        return Err(Error::Network(
            "Could not check for updates. Check your network connection.".into(),
        ));
    }
    let todo: Vec<&Check> = checks.iter().filter(|c| c.newer().is_some()).collect();
    if todo.is_empty() {
        env.io
            .out(&output::ok_line(st, "Everything is up to date."));
        return Ok(());
    }
    // Resolve each update's file and checksum before asking anything.
    struct Plan {
        app: App,
        release: github::Release,
        asset: Asset,
        expect: Option<String>,
    }
    let (mut plans, mut problems) = (Vec::<Plan>::new(), Vec::new());
    for c in &todo {
        let src = c.app.source.clone().unwrap_or(Source {
            repo: String::new(),
            pattern: String::new(),
        });
        let r = github::latest_release(env.fetcher, &src.repo).and_then(|rel| {
            let asset =
                github::select_asset(&rel.assets, &env.host.arch, Some(&src.pattern))?.clone();
            let expect = github::expected_sha256(env.fetcher, &asset, &rel.assets)?;
            Ok(Plan {
                app: c.app.clone(),
                release: rel,
                asset,
                expect,
            })
        });
        match r {
            Ok(p) => plans.push(p),
            Err(e) => problems.push(Error::Failed(format!("{}: {}", c.app.name, e))),
        }
    }
    if plans.is_empty() {
        return Err(combine(problems));
    }
    env.io.out(&format!(
        "{}\n\n",
        st.bold(&format!("{} update(s) available:", plans.len()))
    ));
    let rows: Vec<Vec<String>> = plans
        .iter()
        .map(|p| {
            vec![
                p.app.name.clone(),
                p.app.version.clone(),
                st.arrow().to_string(),
                version::display(&p.release.tag_name).to_string(),
                if p.expect.is_some() {
                    "verified".into()
                } else {
                    "unverified".into()
                },
            ]
        })
        .collect();
    let pretty = env.io.stdout_is_tty();
    env.io.out(&output::table(
        &["App", "Installed", "", "Latest", "Checksum"],
        &rows,
        pretty,
        st,
        &|c, s| match (c, s) {
            (0, _) => st.bold(s),
            (3, _) => st.green(s),
            (4, "unverified") => st.yellow(s),
            _ => st.dim(s),
        },
    ));
    env.io.out("\n");
    confirm(env, st, "Apply these updates?", opts.yes)?;
    let (mut ok, mut failed) = (
        0,
        problems
            .iter()
            .map(|e| (String::new(), e.to_string()))
            .collect::<Vec<_>>(),
    );
    for p in &plans {
        if st.fancy {
            env.io
                .out(&format!("{} Updating {}…\n", st.step_mark(), p.app.name));
        }
        let file_name = match safe_file_name(&p.asset.name) {
            Some(n) => n,
            None => {
                failed.push((
                    p.app.name.clone(),
                    format!(
                        "The release file name '{}' is not one ifhai will use.",
                        p.asset.name
                    ),
                ));
                continue;
            }
        };
        let job = Job::Remote {
            title: p.app.name.clone(),
            from: String::new(),
            url: p.asset.browser_download_url.clone(),
            file_name,
            size: p.asset.size,
            expect: p.expect.clone(),
            delta: p
                .release
                .assets
                .iter()
                .find(|z| z.name == format!("{}.zsync", p.asset.name))
                .map(|z| (z.browser_download_url.clone(), p.app.file.clone())),
            origin: Origin {
                source: Some(Source {
                    repo: p
                        .app
                        .source
                        .as_ref()
                        .map(|s| s.repo.clone())
                        .unwrap_or_default(),
                    pattern: github::pattern_from_asset(&p.asset.name, &p.release.tag_name),
                }),
                tag: Some(p.release.tag_name.clone()),
                version_hint: Some(version::display(&p.release.tag_name).to_string()),
                ..Origin::default()
            },
        };
        match download_and_install(env, &mut state, &job, false) {
            Ok(done) => {
                state.save(&env.dirs.state_file)?;
                report_done(env, st, &done);
                ok += 1;
            }
            Err(e) => failed.push((p.app.name.clone(), e.to_string())),
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    let m = failed
        .iter()
        .map(|(n, e)| {
            if n.is_empty() {
                e.clone()
            } else {
                format!("Could not update {n}.\n{e}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Err(if ok == 0 {
        Error::Failed(m)
    } else {
        Error::Partial(format!(
            "Partly completed: {ok} updated, {} failed.\n\n{m}",
            failed.len()
        ))
    })
}

fn rollback(env: &mut Env, opts: &Opts, name: &str) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let mut state = State::load(&env.dirs.state_file)?;
    let app = state.find(name)?.clone();
    let Some(prev) = app.previous.clone() else {
        return Err(Error::NotFound(format!(
            "{} has no earlier version to go back to.",
            app.name
        )));
    };
    confirm(
        env,
        st,
        &format!(
            "Go back from {} {} to {}?",
            app.name, app.version, prev.version
        ),
        opts.yes,
    )?;
    let ctx = Ctx {
        system: env.system,
        dirs: env.dirs,
        host: env.host,
    };
    let (from, to) = install::rollback(&ctx, &mut state, &app.slug)?;
    state.save(&env.dirs.state_file)?;
    env.io.out(&output::ok_line(
        st,
        &format!(
            "{} is back at {to} (was {from}). Run `ifhai rollback {}` again to return.",
            app.name, app.slug
        ),
    ));
    Ok(())
}

// ---- doctor -------------------------------------------------------------------------------------

#[derive(PartialEq)]
enum Level {
    Ok,
    Info,
    Warn,
    Problem,
}

fn doctor(env: &mut Env, opts: &Opts) -> Result<()> {
    let st = out_style(env, opts.no_color);
    let state = State::load(&env.dirs.state_file)?;
    let mut lines: Vec<(Level, String)> = Vec::new();
    if !env.host.fusermount {
        lines.push((
            Level::Warn,
            "No FUSE (fusermount) was found, so AppImages cannot mount themselves. ifhai sets every app up unpacked instead and they still work. To run them directly, install fuse (Fedora: fuse3, Debian/Ubuntu: fuse3).".into(),
        ));
    } else if env.host.fuse2 {
        lines.push((
            Level::Ok,
            "FUSE 2 is installed: AppImages run directly.".into(),
        ));
    } else {
        lines.push((Level::Warn, "FUSE 2 is not installed. AppImages that need it are set up unpacked and still work. To run them directly, install fuse-libs (Fedora) or libfuse2 (Debian, Ubuntu).".into()));
    }
    match path_note(env) {
        None => lines.push((
            Level::Ok,
            format!("{} is on your PATH.", tilde(env, &env.dirs.bin)),
        )),
        Some(n) => lines.push((Level::Warn, n)),
    }
    for a in state.apps.values() {
        let mut missing: Vec<String> = Vec::new();
        for (what, p) in [
            ("the app", &a.file),
            ("its command", &a.launcher),
            ("its menu entry", &a.desktop),
        ] {
            if !p.exists() {
                missing.push(format!("{what} ({})", tilde(env, p)));
            }
        }
        if a.mode == "extract" && !a.support.join("root").join("AppRun").exists() {
            missing.push("its unpacked files".into());
        }
        if missing.is_empty() {
            lines.push((Level::Ok, format!("{} {} is intact.", a.name, a.version)));
        } else {
            lines.push((
                Level::Problem,
                format!(
                    "{} is missing {}. Reinstall it, or remove it with: ifhai remove {}",
                    a.name,
                    missing.join(", "),
                    a.slug
                ),
            ));
        }
    }
    let managed_names: Vec<String> = state.apps.values().map(|a| a.name.to_lowercase()).collect();
    let mut foreign = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&env.dirs.applications) {
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "desktop"))
            .collect();
        files.sort();
        for p in files {
            let fname = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if fname.starts_with("ifhai-") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let d = crate::desktop::parse(&text);
            if !d
                .get("Exec")
                .is_some_and(|e| e.to_lowercase().contains(".appimage"))
            {
                continue;
            }
            let name = d.get("Name").unwrap_or(&fname).to_string();
            if managed_names.contains(&name.to_lowercase()) {
                lines.push((Level::Problem, format!("{name} has a second menu entry that ifhai did not create ({}). Delete it with: rm '{}'", tilde(env, &p), p.display())));
            } else {
                foreign.push(name);
            }
        }
    }
    if !foreign.is_empty() {
        lines.push((
            Level::Info,
            format!(
                "{} other AppImage menu entr{} not managed by ifhai: {}.",
                foreign.len(),
                if foreign.len() == 1 {
                    "y is"
                } else {
                    "ies are"
                },
                foreign.join(", ")
            ),
        ));
    }
    let known: Vec<&PathBuf> = state.apps.values().map(|a| &a.file).collect();
    if let Ok(rd) = std::fs::read_dir(&env.dirs.apps) {
        let strays: Vec<String> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("appimage"))
                    && !known.contains(&p)
            })
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        if !strays.is_empty() {
            lines.push((Level::Info, format!("{} AppImage file(s) in {} are not installed: {}. Install them with: ifhai install {}/<file>", strays.len(), tilde(env, &env.dirs.apps), strays.join(", "), tilde(env, &env.dirs.apps))));
        }
    }
    let electron: Vec<&str> = state
        .apps
        .values()
        .filter(|a| a.electron)
        .map(|a| a.name.as_str())
        .collect();
    if env.host.userns_restricted && !electron.is_empty() {
        lines.push((Level::Warn, format!("This system restricts unprivileged user namespaces, which Electron apps use for their sandbox. {} may refuse to start with a sandbox error. The usual workaround is adding --no-sandbox to the command in ~/.local/bin (it turns that app's sandbox off, so only do it for apps you trust).", electron.join(", "))));
    }
    let problems = lines.iter().filter(|(l, _)| *l == Level::Problem).count();
    for (lvl, text) in &lines {
        let (mark, plain) = match lvl {
            Level::Ok => (st.ok_mark(), "[ok]"),
            Level::Info => (st.dim("·"), "[info]"),
            Level::Warn => (st.warn_mark(), "[warn]"),
            Level::Problem => (st.fail_mark(), "[problem]"),
        };
        env.io.out(&if st.fancy {
            format!("{mark} {text}\n")
        } else {
            format!("{plain} {text}\n")
        });
    }
    if problems > 0 {
        Err(Error::Child(1))
    } else {
        Ok(())
    }
}

/// Parse and execute; returns the exit code. All user-facing errors are printed here.
pub fn run(args: &[String], env: &mut Env) -> i32 {
    match cli::parse(args).and_then(|inv| execute(env, inv)) {
        Ok(()) => 0,
        Err(e) => {
            let st = err_style(env);
            match &e {
                Error::Child(_) => {}
                Error::Aborted => env.io.err(&output::cancelled(st)),
                _ => {
                    let tail = if matches!(e, Error::Usage(_)) {
                        "\n\nTry: ifhai --help"
                    } else {
                        ""
                    };
                    env.io.err(&output::error(st, &e.to_string(), tail));
                }
            }
            e.exit_code()
        }
    }
}
