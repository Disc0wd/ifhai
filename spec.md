# IFHAI — Product & Technical Specification

**Project:** IFHAI\
**Executable:** `ifhai`\
**Parent project family:** IFHX (siblings: `ifhf`, `ifhx`)\
**Status:** Draft specification, not yet implemented\
**Primary platform:** Linux (x86_64 first, aarch64 where assets exist)\
**Implementation language:** Rust\
**Backend:** AppImage files, GitHub Releases, the AppImageHub catalogue\
**Infrastructure requirement:** No hosted services, databases, APIs or paid infrastructure of our own

> ## Implementation status (v0.1.0)
>
> Built and tested: `install` from a local file (FR-101..105), from `owner/repo` and from an `https://`
> link, `remove`, `list`, `run`, `updates`, `update`, `rollback`, `doctor`; FUSE detection with
> unpacked mode; digest/checksum verification; embedded GitHub update information.
>
> **Deliberately simpler than this spec**, following the owner's brief ("not too many knobs"):
> there is **no config file**; the only options are `--yes`, `--keep` and `--no-color`; the launch
> mode, version retention (always one previous version) and update source are decided automatically;
> files live in a visible `~/Applications` and the original download is moved.
>
> **Not built yet:** catalogue `sync`/`search`/`info` and install by name (§5.3, §5.7), pacman-style
> syntax (§4.2), pre-release channels, pinning, `--no-sandbox` opt-in (doctor only explains),
> sandbox profiles, delta updates, `scan`/`doctor --fix` (doctor reports; it never deletes),
> double-click handler (FR-106).
>
> **Differences found while building:** reading an AppImage's `.desktop` file and icon runs its
> runtime with `--appimage-extract` after the user confirms (a pure-Rust SquashFS reader was not
> adopted, §8.2), with `unsquashfs` as the fallback when the runtime cannot start.

---

## 1. Product overview

**The job, in one sentence:** you downloaded a `.AppImage`; one command makes it a real,
working application: no `chmod +x`, no hunting for a place to put it, no hand-written menu entry,
no missing icon, no FUSE error.

```text
$ ifhai install ~/Downloads/Obsidian-1.8.9.AppImage
┃ Obsidian 1.8.9
┃ Move to     ~/.local/share/ifhai/apps/obsidian/
┃ Menu entry  Obsidian (with icon)
┃ Command     obsidian
┃ Runs with   FUSE (this system has it)

Install Obsidian? [Y/n]
✓ Installed Obsidian. Find it in your app menu, or run: obsidian
```

Everything else here (search by name, updates, rollback, `doctor`) is built **on top of** that job
and comes after it. IFHAI is the AppImage sibling of `ifhf` (Flatpak) and uses the same interaction
model: show what will happen, ask once, never guess. It integrates AppImages into the desktop
**cleanly and reversibly** and diagnoses the things that make them miserable: missing FUSE, broken
Electron sandboxes, duplicate menu entries and orphaned files.

It is the AppImage sibling of `ifhf` (Flatpak) and uses the same interaction model: resolve names
to something concrete, show what will happen, ask once, never guess.

### 1.1 Core product principle

> An AppImage should behave like an installed application: one command to get it, one to update
> it, one to remove it, and nothing left behind. Never run an unknown binary just to look at it.

### 1.2 Goals

1. **Make a downloaded AppImage just work with one command** (`ifhai install FILE`): executable,
   placed, in the menu with its icon, launchable, FUSE handled. No flags, no manual steps.
2. Later: also install by name, `owner/repo` or URL.
3. Keep AppImages current, with rollback when an update is worse than what you had.
4. Own the desktop integration: exactly one launcher, icon and menu entry per app, recorded so
   removal is exact. No daemon.
5. Work on systems without FUSE 2, and explain (not just fail) when something cannot run.
6. Verify what is downloaded and say so when it cannot be verified.
7. Be fast: local operations (search, list, info) never touch the network or spawn processes.
8. Match `ifhf`'s UX: confirmations, ambiguity handling, exit codes, terminal styling.

### 1.3 Non-goals

IFHAI will not:

- Be a GUI, a package builder, or an AppImage creation tool.
- Manage Flatpak, Snap, distro packages or other portable formats (that is `ifhf`'s and the
  distro's job). One format, done properly.
- Host a catalogue, mirror, database or API.
- Run a background daemon or watch directories (the `appimaged` model).
- Patch, repack or replace the runtime of an AppImage.
- Install system-wide or require root. User scope only.
- Silently disable a sandbox, change a sysctl, or write an AppArmor profile.
- Execute an AppImage in order to inspect it.

---

## 2. Research findings that shape this design

Findings are dated November 2026. Items marked *(unverified)* come from secondary sources.

### 2.1 What actually hurts

| Pain | Evidence | Design response |
|:---|:---|:---|
| **FUSE 2.** Older type-2 AppImages `dlopen` `libfuse.so.2` and call the FUSE 2 `fusermount`. Distros are removing FUSE 2 (Fedora's Atomic Desktops change drops it; Ubuntu ships fuse3). Newer static runtimes (type2-runtime) no longer need it, but plenty of existing files are old. | [AppImage FUSE docs](https://docs.appimage.org/user-guide/troubleshooting/fuse.html), [Fedora change](https://fedoraproject.org/wiki/Changes/AtomicDesktopDropFuse2) | Detect per file, choose a launch mode (§7). Extract once and run `AppRun` directly when FUSE 2 is missing. `doctor` explains. |
| **Electron sandbox.** Electron AppImages fail on Ubuntu 24.04+ because AppArmor restricts unprivileged user namespaces and the FUSE mount is `nosuid`, so the SUID helper cannot work. Users paste `--no-sandbox`. | [AppImage docs](https://docs.appimage.org/user-guide/troubleshooting/electron-sandboxing.html), [Ubuntu discussion](https://discourse.ubuntu.com/t/problem-runing-appimage-files-without-no-sandbox/78361) | Detect, explain, offer **per-app, explicit, reversible** `--no-sandbox` with a warning. Never silent. |
| **Duplicate / stale desktop entries.** `appimaged`, AppImageLauncher and the AppImages' own first-run integration create competing or duplicate `.desktop` files; entries survive updates; unquoted paths break on spaces. | [KDE bug 490657](https://bugs.kde.org/show_bug.cgi?id=490657), [LibreWolf issue](https://gitlab.com/librewolf-community/browser/appimage/-/issues/22), [AppImagePool #87](https://www.github.com/prateekmedia/appimagepool/issues/87) | One owned entry per app, tagged `X-IFHAI-ID`, every created file recorded in a manifest, `scan` finds foreign duplicates, `doctor --fix` removes them with consent. |
| **No update story.** Updating means finding the new file by hand. | [Gear Lever reviews](https://itsfoss.com/gear-lever/) | Embedded update information, catalogue lookup, tag-cheap checks (§6). |
| **Unmaintained integration tooling.** AppImageLauncher is widely described as stale *(unverified)*. | [AlternativeTo](https://alternativeto.net/software/appimagelauncher/) | Do not depend on it; do not conflict with it (detect and warn). |

### 2.2 What already exists, and the gap

| Tool | Shape | What it does well | Gap IFHAI targets |
|:---|:---|:---|:---|
| [AM / AppMan](https://github.com/ivan-hc/AM) | Large shell script, own app database | Huge curated list, sandboxing via Aisap, system or local install | Script-based; its database is its own packaging scripts, not AppImageHub |
| [Soar](https://github.com/pkgforge/soar) | Rust, multi-format (AppImage, static binaries, more) | Fast, checksum + signature verification, zsync delta updates, desktop entries, forge-release updates | Broader scope than "AppImages done properly"; depends on pkgforge metadata |
| [Gear Lever](https://github.com/mijorus/gearlever) | GTK Flatpak GUI | Integrates dropped files, update checks | GUI only; no CLI, no catalogue search |
| [AppManager](https://github.com/kem-a/AppManager) *(July 2026)* | GTK4 GUI, uruntime mounting (FUSE-less), zsync | Drag-to-install, GitHub/GitLab update links | GUI; Ubuntu-focused |
| AppImageLauncher / appimaged | Integration helper / daemon | Menu entries | Stale or daemon-based, and a source of the duplicate-entry problem |

**Gap:** no *CLI, name-resolving, ifhf-style* manager whose differentiators are exact reversible
integration, FUSE-independence diagnostics, update with rollback, and never executing unknown
binaries to inspect them. That is IFHAI.

### 2.3 Data sources

- **AppImageHub feed** at `https://appimage.github.io/feed.json`. Measured 2026-11-25: 2.8 MB,
  **4,391 items**, **4,061 with links**. Fields: `name`, `description`, `categories`, `authors`,
  `license`, `links` (`{type:"GitHub", url:"owner/repo"}` and a `Download` URL to the releases
  page), `icons`, `screenshots`, `libc`, `glibc_required`, `self_contained`. It has **no asset URLs,
  versions or checksums**, and the project warns the format may change
  ([README](https://github.com/AppImage/appimage.github.io)). Parse defensively; many entries are
  stale (repo abandoned, latest release has no AppImage).
- **GitHub Releases** is therefore the source of truth for what can be downloaded. Since
  [June 2025](https://github.blog/changelog/2025-06-03-releases-now-expose-digests-for-release-assets/)
  release assets carry a `digest` (`sha256:...`) in the REST API, **not backfilled** for older
  releases ([uv note](https://app.semanticdiff.com/gh/astral-sh/uv/commit/327c2bcd8a9567f46de4f2d86a5a744968346cf7)).
  Unauthenticated API use is limited to 60 requests/hour.
- **The AppImage specification** ([draft](https://github.com/AppImage/AppImageSpec/blob/master/draft.md)):
  type 2 images are ELF executables with a filesystem appended, magic `0x414902` at offset 8,
  optional update information in ELF section `.upd_info`, optional signature in `.sha256_sig`.
  Update transports: `zsync|<url>` and `gh-releases-zsync|<user>|<repo>|<tag>|<file pattern>` (tag may
  be `latest`, `latest-pre`, `latest-all`). Type 1 stores update info in the ISO 9660 volume
  descriptor at offset 33651. Newer forge transports (GitLab, Codeberg, Gitea) exist in tooling
  *(unverified)*.
- **Runtimes.** The classic type2-runtime uses SquashFS; community runtimes such as `uruntime` use
  DwarFS. A static runtime no longer needs FUSE 2 on the host. IFHAI must not assume SquashFS.

### 2.4 This machine (the reference user)

Fedora Linux 45, KDE Plasma. `libfuse.so.2` and `libfuse3` both present; `fusermount` and
`fusermount3` present; no `unsquashfs`, `zsync` or AppImage tooling installed; no AppImages
currently on disk. So the first target is *"install apps cleanly"* rather than *"rescue a broken
setup"*, but the FUSE-less path must work for the many users on systems without FUSE 2.

---

## 3. Target users and use cases

Linux desktop users, terminal-comfortable, who end up with AppImages because that is how some
software ships, and who want them to behave like installed software.

| ID | User goal | Example |
|:---|:---|:---|
| UC-01 | **Make a downloaded AppImage work** | `ifhai install ~/Downloads/App.AppImage` |
| UC-02 | Install several at once | `ifhai install ~/Downloads/*.AppImage` |
| UC-03 | See what is installed, launch it | `ifhai list`, `ifhai run obsidian` |
| UC-04 | Remove an app completely | `ifhai remove obsidian` |
| UC-05 | *(later)* Search the catalogue | `ifhai search markdown editor` |
| UC-06 | Inspect an app | `ifhai info obsidian` |
| UC-07 | List what is installed | `ifhai list` |
| UC-08 | Check for / apply updates | `ifhai updates`, `ifhai update` |
| UC-09 | Undo a bad update | `ifhai rollback obsidian` |
| UC-10 | Remove an app completely | `ifhai remove obsidian` |
| UC-11 | Find AppImages and integrations that are not managed | `ifhai scan` |
| UC-12 | Find out why something will not run, and fix it | `ifhai doctor` |
| UC-13 | Launch with a specific mode or arguments | `ifhai run obsidian --mode extract` |

---

## 4. Interface design

### 4.1 Natural syntax

```bash
ifhai install obsidian          ifhai update [app...]
ifhai remove obsidian           ifhai updates
ifhai search editor             ifhai rollback obsidian
ifhai info obsidian             ifhai install FILE...
ifhai list                      ifhai scan
ifhai run obsidian [-- args]    ifhai doctor [--fix]
ifhai sync                      ifhai config obsidian --mode extract
```

### 4.2 Pacman-style syntax

Both forms parse into one internal command, exactly as in `ifhf`:

| Natural | Compact |
|:---|:---|
| `ifhai install x` | `ifhai -S x` |
| `ifhai remove x` | `ifhai -R x` |
| `ifhai search x` | `ifhai -Ss x` |
| `ifhai info x` | `ifhai -Si x` |
| `ifhai list` | `ifhai -Q` |
| `ifhai updates` | `ifhai -Qu` |
| `ifhai update` | `ifhai -Syu` |
| `ifhai sync` | `ifhai -Sy` |

### 4.3 Shared conventions with `ifhf`

`--yes` / `--noconfirm`, `--no-color`, `NO_COLOR`, never wait on a closed terminal (exit 5),
distinct exit codes (0 ok, 1 failure, 2 usage, 3 not found, 4 ambiguous, 5 confirmation required,
6 cancelled), plain tab-separated output when piped, colour and symbols only on a terminal,
ASCII fallback on non-UTF-8 locales.

---

## 5. Functional requirements

Priority: **P0** first usable release, **P1** important follow-up, **P2** optional.

### 5.0 The core job: make a downloaded AppImage just work

**FR-101 — One command (P0).** `ifhai install FILE...` (also `ifhai -S FILE`; shell globs work). No
flags needed. For each file it: verifies it is an AppImage (ELF + magic, **without executing it**);
makes it executable; moves it into the managed directory under a clean name; reads the app's name,
version, icon, categories and bundled `.desktop` file from inside it without running it; creates the
menu entry and icon; puts a launcher on `PATH` (`~/.local/bin/<slug>`); chooses a launch mode that
will actually work on this machine (§7); and tells you how to start it.

**FR-102 — Plan, then one question (P0).** Shows name, version, where things go and what is
created; one confirmation for all files; `--yes` skips it; never waits on a closed terminal.

**FR-103 — No duplicates (P0).** Installing a newer file of an app that is already installed
upgrades it in place (the old version is kept for rollback) instead of creating a second copy and a
second menu entry. A file that cannot be told apart from an installed app by identity is asked
about, never guessed.

**FR-104 — The download is moved, not copied (P0).** No stray copy left in `~/Downloads`. `--keep`
copies instead. Moves across filesystems copy, verify, then delete.

**FR-105 — Works first time (P0).** After installing, IFHAI checks the app can start here (the file
is executable, the mode is chosen, required libraries are known) and reports any problem with the
fix, for example: *"Needs FUSE 2, which this system lacks. Installed to run extracted instead, so it
works."* This is the "a bunch of other stuff for it to work properly" the user should never do.

**FR-106 — Open from the file manager (P2).** Optionally register IFHAI as a handler for AppImage
files so opening a downloaded file offers to install it. A normal user-triggered desktop entry; no
daemon.

### 5.1 Installing from outside (name, repository, URL)

**FR-001 — Install by catalogue name (P1).** `ifhai install obsidian` resolves the name against
the catalogue, finds the latest release's AppImage for this architecture, shows app, version,
source and verification status, asks once, installs.

**FR-002 — Install by `owner/repo` and URL (P1).** Any GitHub repository, or a direct `https://`
URL. Not-in-catalogue apps are first-class. (`http://` is refused.)

**FR-003 — Install several apps (P1).** Resolve all first, one confirmation, report exactly what
succeeded and failed.

**FR-004 — Asset selection (P0).** From a release's assets choose one file by: suffix `.AppImage`
(case-insensitive); architecture match (`x86_64`/`amd64`, `aarch64`/`arm64`); exclude `.zsync`,
checksum, signature and obvious non-targets (`debug`, `source`). If several candidates remain, ask
(never pick arbitrarily) and remember the choice as a **name pattern** (version replaced by `*`) so
updates pick the same variant.

**FR-005 — Compatibility warning (P1).** If the catalogue lists `glibc_required` newer than the host's glibc,
warn before installing.

**FR-006 — Local files (superseded).** Installing a local file is FR-101. A source can be attached
later with `ifhai config APP --source owner/repo` so the app becomes updatable (P1).

**FR-007 — Install is atomic (P0).** Download to a temp file beside the destination, verify, `chmod
0755`, `rename`. Any failure leaves the previous version and all integration untouched.

### 5.2 Removing

**FR-008 — Remove cleanly (P0).** Delete every file recorded in the app's manifest (versions,
launcher, desktop entry, icons), refresh desktop caches best-effort, forget the app. Tolerate files
the user already deleted.

**FR-009 — Data is kept (P0).** Application data and config (`~/.config/<app>` and friends) are not
touched; say so. **FR-010 (P2):** `--purge` lists likely data directories and deletes only after
explicit confirmation of that list.

### 5.3 Search and discovery

**FR-011 — Search the catalogue (P1).** Name, description, categories, `owner/repo`; same
deterministic ranking approach as `ifhf` (exact name, normalised name, prefix, tokens, partial,
description). Ambiguity is never resolved silently.

**FR-012 — Result information (P1).** Name, `owner/repo`, summary, categories; mark installed apps.
Entries with no GitHub link are shown but marked not installable.

**FR-013 — Info (P1).** Catalogue data, installed version and path, latest release, update source,
runtime type, launch mode, whether a digest is available. Missing metadata is stated, not invented.

**FR-014 — Offline (P1).** Search and info work from the cached catalogue; say when it is stale.

### 5.4 Updates

**FR-015 — Check for updates (P1).** `updates` compares the installed version with the latest
release using the cheapest signal first (§6.2). Changes nothing.

**FR-016 — Update (P1).** Per app: download, verify, install side by side with the old version,
switch the launcher atomically, refresh the desktop entry (name and version may have changed), keep
the previous version for rollback. Report per-app results.

**FR-017 — Rollback (P1).** `ifhai rollback APP` switches back to the previous retained version.
Retain 1 previous version by default (`keep_versions` in config). A rolled-back app is not updated
again until `update APP` is explicit or a newer version than the one rolled back from appears.

**FR-018 — Pin (P1).** `ifhai config APP --pin` excludes an app from `update`.

**FR-019 — Delta updates (P2).** If the `zsync` program is on `PATH` and the app has a zsync
update source, use it; otherwise download the full file. IFHAI does not implement zsync itself.

**FR-020 — Pre-releases (P1).** Never by default; `--pre` or per-app config opts in (matches the
spec's `latest-pre` / `latest-all`).

### 5.5 Desktop integration

**FR-021 — One owned entry (P0).** Exactly one launcher, one `.desktop` file and one icon set per
managed app, all created by IFHAI and recorded in the manifest (§9).

**FR-022 — Rewrite, don't copy (P0).** The `.desktop` file inside the AppImage is parsed and
re-emitted: `Exec`/`TryExec` point at the launcher, `Icon` at an installed icon, every
`[Desktop Action]` `Exec` is rewritten, paths are quoted per the Desktop Entry spec, `X-AppImage-*`
integration keys are dropped, `X-IFHAI-ID=<slug>` and `X-IFHAI-Managed=true` are added.

**FR-023 — No hijacking (P0).** `MimeType` and URL-scheme handlers are kept in the entry but IFHAI
never sets default applications. P1: show them in the install plan.

**FR-024 — Foreign entries (P0).** `scan` reports `.desktop` files in the user's applications
directory that point at an AppImage and are not IFHAI's (appimaged, AppImageLauncher, the AppImage's
own first-run integration), including duplicates of managed apps. `doctor --fix` removes duplicates
of **managed** apps after confirmation; it never touches entries for unmanaged apps.

**FR-025 — Cache refresh (P1).** After changes run, if present and best-effort,
`update-desktop-database` and the icon-cache tool for the user directory. Never required, never
fatal.

### 5.6 Running and compatibility

**FR-026 — Launch modes (P0).** `fuse` (run the AppImage directly), `extract` (FUSE-free: run the
persistently extracted tree, §7), `auto` (default: decided at install time and re-evaluated by
`doctor`). The launcher generated for the app implements the mode.

**FR-027 — `run` (P0).** `ifhai run APP [-- args]` launches through the same launcher the menu uses.

**FR-028 — Doctor (P0).** Checks and explains, per §7.3: FUSE 2 / FUSE 3 libraries and
`fusermount`; user namespaces and the AppArmor restriction; `~/.local/bin` on `PATH`; desktop
database tooling; manifest-versus-disk drift; orphaned managed files; foreign duplicate entries;
apps whose runtime needs something missing. Read-only unless `--fix`.

**FR-029 — Electron sandbox (P1).** For apps that bundle `chrome-sandbox`, `doctor` reports whether
the sandbox can work here. `ifhai config APP --no-sandbox` adds `--no-sandbox` to that app's
launcher **only after an explicit warning and confirmation**, and `--sandbox` reverses it.

**FR-030 — Sandboxing profiles (P2, research first).** Optional `bubblewrap`-based run profiles.
Out of scope until a profile format and its security claims are evaluated; AM's Aisap
([notes](https://github.com/ivan-hc/AM/pull/663)) is experimental by its author's own description.

### 5.7 Catalogue and cache

**FR-031 — Sync (P1).** `ifhai sync` refreshes the catalogue (one HTTPS request, conditional via
`ETag`/`Last-Modified`).

**FR-032 — Local index (P1).** Parse `feed.json` once into a disposable, version-stamped, atomically
written text index (the `ifhf` design), rebuilt when the feed changes, IFHAI is upgraded, or the
cache is too old. A corrupt or foreign index is ignored and rebuilt. A failed refresh keeps the last
good index and is never presented as an empty catalogue.

**FR-033 — Defensive parsing (P1).** Unknown fields are ignored, a bad record is skipped and
counted, a feed that does not parse is an error, not an empty catalogue.

---

## 6. Resolution, versions and update logic

### 6.1 Name resolution

Same deterministic model as `ifhf` (exact id → exact name → normalised name → prefix → tokens →
partial → description), with `owner/repo` matched exactly first. Return candidates and an ambiguity
status; the CLI asks, or fails with exit code 4 and lists them.

### 6.2 Finding the latest version cheaply

The 60 requests/hour unauthenticated API limit makes naive checking unusable for a dozen apps.
Order of signals:

1. **Tag redirect, no API.** Request `https://github.com/<owner>/<repo>/releases/latest` and read the
   tag from the redirect (the technique `ifhx` already uses). If the tag equals the installed one,
   the app is up to date and **no API call is made**.
2. **Releases API** only when the tag changed or an install is happening: fetch the release's
   assets to select the file and read its `digest`. Honour `GITHUB_TOKEN` (env) or the config
   token to lift the limit; send `If-None-Match` and cache the response by `ETag`.
3. **Embedded update information** (`gh-releases-zsync`, or `zsync`) read from the installed file's
   `.upd_info` when the app has no catalogue/source entry. `zsync|url` sources use `ETag` /
   `Last-Modified` change detection; there is no version string, so they report "changed".
4. **Direct URL pins** use `ETag`/`Last-Modified`.

Rate-limit responses are reported as such ("GitHub rate limit reached; set GITHUB_TOKEN"), never as
"up to date".

### 6.3 Versions

Tags are compared as dotted numbers where possible (as in `ifhx`); anything else counts as
"different". The installed version is the release tag the file came from, not parsed from the file
name. Adopted files without a source have no version and are never auto-updated.

---

## 7. Launch modes and FUSE independence

### 7.1 Detection (no execution)

For each AppImage, from the file alone:

- Valid ELF; type from the magic at offset 8 (`AI\x01` type 1, `AI\x02` type 2).
- Embedded update information from `.upd_info` (type 2) or the volume descriptor (type 1).
- Whether the runtime region references `libfuse.so.2` (old dynamic runtime) or is a static runtime.
- Offset of the appended filesystem, found from the ELF headers.

### 7.2 Modes

| Mode | How | When |
|:---|:---|:---|
| `fuse` | Launcher `exec`s the AppImage | FUSE 2 present, or the runtime is static |
| `extract` | The image is extracted **once** at install to `…/<slug>/<version>/root/`; the launcher runs `AppRun` with `APPDIR` and `APPIMAGE` set | Dynamic runtime and FUSE 2 absent, or user's choice |
| `auto` | Choose at install, re-evaluated by `doctor` | Default |

`extract` is a persistent extraction rather than the runtime's per-launch `--appimage-extract-and-run`
(which re-extracts to a temp directory every start and is slow for large apps). Extraction uses the
AppImage's own runtime (`--appimage-extract`) **after** the user has chosen to install it (running it
is then what they asked for) or, where possible, IFHAI's own reader (§8.2), which needs no
execution. If the image is not SquashFS and cannot be read, `extract` is unavailable for that app
and `doctor` says why.

### 7.3 What `doctor` explains, with the actual cause

Messages state the cause and the fix, e.g. *"obsidian needs FUSE 2 (libfuse.so.2 not found). It is
set to run extracted instead, so this works. To use FUSE mode install fuse-libs."* Doctor never
changes a sysctl, installs packages, or edits AppArmor.

---

## 8. Reading AppImages without executing them

### 8.1 ELF and metadata

Plain ELF parsing in Rust (headers, section table, `.upd_info`, `.sha256_sig`): a few hundred lines,
no dependency needed. Validate bounds on every read; a malformed file is "not an AppImage", never a
panic.

### 8.2 Filesystem contents

To read the bundled `.desktop` file, icon (`.DirIcon`/hicolor) and AppStream metadata, IFHAI needs a
SquashFS reader. **Phase 0 task:** evaluate maintained pure-Rust SquashFS crates (for example
`backhand`) for correctness on real-world AppImages, compression support (gzip/xz/zstd) and
unsafe surface, per the dependency policy (§12). DwarFS-based images are out of scope for
pre-install inspection in v1; for those IFHAI falls back to catalogue data and file name and says
so.

---

## 9. State, layout and the manifest

All paths user-level, XDG-aware, none hardcoded.

```text
~/.local/share/ifhai/
  state.toml                 # installed apps, sources, modes, pins, retained versions
  apps/<slug>/
    <version>/<file>.AppImage
    <version>/root/          # only in extract mode
    current -> <version>     # atomic symlink; rollback flips it
~/.local/bin/<slug>          # launcher (symlink in fuse mode, tiny script in extract/args mode)
~/.local/share/applications/ifhai-<slug>.desktop
~/.local/share/icons/hicolor/<size>/apps/ifhai-<slug>.png
~/.cache/ifhai/              # catalogue index, ETags
~/.config/ifhai/config.toml
```

- The **manifest** in `state.toml` lists every file IFHAI created for an app. Removal deletes exactly
  that set. `doctor` compares manifest to disk and reports drift in both directions.
- A damaged `state.toml` is reported, never silently reset.
- Slugs are derived from the app name, restricted to `[a-z0-9-]`, and unique. File names taken from
  GitHub assets are sanitised before use (no path separators, no leading dot or dash).

---

## 10. Security and reliability

### 10.1 Requirements

- HTTPS only. Downloads, catalogue and API all over `https://`; `http://` URLs are refused.
- **Verification order:** (1) the release asset's GitHub `digest`; (2) a checksum asset in the release
  (`SHA256SUMS`, `<file>.sha256`); (3) the embedded `.sha256_sig` signature when `gpg` and a key are
  available (P2). If none exists, IFHAI says **"unverified: the publisher provides no checksum"**
  in the install plan and requires an explicit "yes" at the prompt. Under `--yes` it **refuses**
  unless `--allow-unverified` is also given. The recorded SHA-256 is stored in state and re-checked
  by `doctor`.
- Be honest about the limit: a digest from the same release protects against corruption and CDN
  tampering, not a compromised repository. The plan says "verified against GitHub's digest", not
  "safe".
- Never execute an AppImage to inspect it (§8). Executing happens only when the user installs
  (extract mode) or runs the app.
- No root. No daemon. No writing outside the user's XDG directories.
- Generated `.desktop` and launcher files are produced by escaping, not string-pasting: arguments
  passed as arguments, spaces and metacharacters in paths handled (the documented failure of other
  tools).
- Download size limit and timeout; a stalled or oversized download is aborted and cleaned up.

### 10.2 Reliability

Handle: no network, rate limiting, 404s on stale catalogue entries ("listed in AppImageHub but its
latest release has no x86_64 AppImage"), releases with several candidate assets, truncated downloads,
disk full, a manifest that no longer matches disk, and partial failure in multi-app operations
(report exactly which succeeded). Return meaningful exit codes.

---

## 11. Configuration

`~/.config/ifhai/config.toml`, all optional, precedence: command-line, config, default.

```toml
[general]
confirm_install = true
confirm_remove = true
color = true

[install]
keep_versions = 1          # previous versions retained for rollback
default_mode = "auto"      # auto | fuse | extract

[catalogue]
url = "https://appimage.github.io/feed.json"
max_age_hours = 24

[github]
token = ""                 # or GITHUB_TOKEN in the environment
```

Missing config never prevents startup; invalid values give actionable errors naming the file;
unknown keys are ignored.

---

## 12. Architecture and dependency policy

```text
ifhai/
├── Cargo.toml  README.md  spec.md
├── src/
│   ├── main.rs  lib.rs  error.rs
│   ├── cli/        (args, commands, io)            same split as ifhf
│   ├── core/       (models, resolver, service)
│   ├── appimage/   (elf.rs, update_info.rs, squashfs.rs, desktop.rs)   pure, no I/O side effects
│   ├── source/     (github.rs, catalogue.rs, fetch.rs)                  network behind a trait
│   ├── store/      (layout.rs, state.rs, integrate.rs, launcher.rs)     filesystem behind a trait
│   ├── doctor/     (checks.rs)
│   ├── config/ output/
└── tests/
```

- **Layers:** the CLI never touches the network or filesystem directly. Network sits behind a
  `Fetcher` trait and installation behind a `Store`, so everything is tested with fakes (the `ifhf`
  and `ifhx` pattern). Tests never touch the real home directory.
- **Dependencies:** `serde`, `toml`, `sha2`, and `serde_json` for the feed. HTTP through `curl` as in
  `ifhx` (no TLS stack) unless Phase 0 finds a reason otherwise. ELF parsing in-house. A SquashFS
  crate only after the Phase 0 evaluation. No async runtime.
- Check each new crate's maintenance, licence, API and `unsafe` surface before adopting it.

---

## 13. User experience

- The install plan shows app, version, source, file, size, verification status, launch mode and what
  will be created, then asks once.
- Same visual language as `ifhf`/`ifhx`: accent-coloured IDs, accent bar on plans, `✓`/`✗`, dim
  secondary text, quiet "Cancelled.", plain stable output when piped.
- Errors say what happened and what to do next. "Not an AppImage", "no x86_64 asset in the latest
  release", "FUSE 2 missing, running extracted instead" are all complete sentences with a next step.

---

## 14. Performance goals

Local operations never start a subprocess or touch the network.

| Operation | Target (warm) |
|:---|---:|
| Startup | < 5 ms |
| `search`, `info` on the catalogue | < 20 ms |
| `list` | < 10 ms |
| First index build from `feed.json` | < 300 ms |

Network-bound operations (`sync`, `updates`, installs) are measured, not targeted. Benchmarks use
`hyperfine`; comparisons against other managers are made only for like-for-like operations and only
if those tools are installed, and results state the machine and method. IFHAI cannot make downloads
faster.

---

## 15. Testing strategy

Mock `Fetcher` and `Store` for routine tests; temporary directories for filesystem behaviour; the
real binary against stubs for end-to-end. Never the real home directory or network.

- **Unit:** ELF parsing (valid, truncated, hostile offsets), update-information parsing, asset
  selection and ambiguity, name-pattern memory across versions, version comparison, resolver,
  `.desktop` rewriting (quoting, actions, dropped keys), config, catalogue index (round-trip,
  corruption, versioning), slug and file-name sanitising.
- **Integration:** install / update / rollback / remove with exact manifest cleanup; atomic failure
  at every step leaves the old version intact; unverified-download confirmation; partial multi-app
  failure; extract-mode launcher; foreign-entry detection and the "never touch unmanaged apps" rule;
  rate-limit and 404 reporting; offline behaviour; non-interactive rules.
- **Fixtures:** small synthetic AppImages (valid ELF + magic + tiny SquashFS) built in tests; a
  trimmed real `feed.json` sample.
- **Acceptance:** a user can install by name, update, roll back and remove with nothing left behind;
  an app whose runtime needs FUSE 2 runs on a host without it; `scan` finds appimaged-style duplicates;
  an unverifiable download is never installed silently.

---

## 16. Development roadmap

Each phase leaves a working tool.

**Phase 0 — Validation.** Prototype ELF/magic/`.upd_info` reading on several real AppImages
(classic SquashFS and DwarFS). Evaluate SquashFS crates for reading the bundled `.desktop` and icon
without executing. Prove `.desktop` rewriting (quoting, actions) against real files. Record decisions.

**Phase 1 — The core job.** `install FILE...`, `remove`, `list`, `run`: validation, chmod, move,
managed layout, manifest, desktop entry and icon, launcher on `PATH`, plan and confirmation,
no-duplicates upgrade-in-place, exact cleanup. No network.

**Phase 2 — Works first time.** FUSE detection, extract mode, post-install start check, `scan` for
foreign duplicate entries, `doctor`, Electron sandbox diagnosis with explicit opt-in.

**Phase 3 — Update with rollback.** Attach a source, cheap tag check, `updates`, `update`, retained
versions, `rollback`, embedded update information.

**Phase 4 — Install from outside.** `sync`, index, `search`, `info`; install by catalogue name,
`owner/repo` or URL; asset selection; digest verification.

**Phase 5 — Performance and hardening.** Benchmarks, parser fuzz-style tests, atomicity review.

**Phase 6 — Release.** CI with static musl binary and checksum, `ifhx` registry entry, README, site.

---

## 17. Explicit scope limits

Unless a later, separately justified phase requires them: a GUI, a daemon or directory watcher,
system-wide install, other package formats, a hosted catalogue or mirror, an in-house zsync
implementation, runtime patching or replacement, automatic background updates, sandbox profile
authoring, AppArmor or sysctl changes, an AppImage builder.

---

## 18. Definition of done: IFHAI 1.0

IFHAI 1.0 is ready when it can: install by name, `owner/repo`, URL and local file; verify what it
downloads and say when it cannot; update and roll back; remove with nothing left behind; integrate
exactly one entry per app and report foreign duplicates; run apps on a host without FUSE 2; explain
failures with `doctor`; search and list without network or subprocesses; pass its test suite; ship as
a verified static binary installable through `ifhx`; and carry clear documentation. Performance
claims must be backed by reproducible benchmarks.

---

## 19. Open decisions (need the project owner)

| # | Question | Recommendation |
|:--|:---|:---|
| 1 | Where do the AppImage files live? | `~/.local/share/ifhai/apps/` (hidden, tidy, exact cleanup) with `ifhai list --paths` for discoverability. A visible `~/Applications` is friendlier to people who browse files. |
| 2 | FUSE-less strategy | Persistent extraction (§7). Alternative: depend on an external `uruntime` / extract-and-run; simpler, but slower launches and one more thing to install. |
| 3 | Sandboxing in v1? | No. Diagnose Electron sandbox problems and offer explicit `--no-sandbox`; leave `bubblewrap` profiles (FR-030) until researched properly. |
| 4 | Catalogue scope (later phase) | AppImageHub only; "any GitHub repo" covers the rest. |
| 5 | Pacman-style syntax | Yes, for parity with `ifhf` (§4.2). Cheap, since it shares the command parser pattern. |
| 6 | Delta updates | Use the `zsync` binary if present (FR-019), no in-house implementation. |
| 7 | The original download | **Move** it (FR-104) so nothing is left in Downloads; `--keep` to copy. |

---

## 20. Instructions for a coding agent

Implement incrementally, starting at Phase 0.

**Before implementation:** inspect the repository; verify current crate APIs and compatibility;
resolve the open decisions in §19 with the owner; record any deviation from this specification
before implementing it.

**Rules:**

- Phase 0 and Phase 1 first; do not implement the whole roadmap in one pass.
- Reuse the architecture and conventions of `ifhf`/`ifhx` (backend traits, mock-based tests, output
  styling, exit codes). Do not invent new UX.
- Never execute an AppImage to inspect it. Never run anything as root. Never write outside XDG user
  directories.
- Every filesystem change is atomic or recorded in the manifest. Every failure path leaves the
  previous state intact, and has a test that proves it.
- No custom network stack, no daemon, no new dependency without a concrete requirement and a
  maintenance check.
- Run formatting, clippy (`-D warnings`) and tests after each meaningful step.

**First milestone:** `ifhai install ~/Downloads/Some.AppImage` takes a real, freshly downloaded,
non-executable AppImage to an app that is in the menu with its icon and starts, with no other manual
step; `list`, `run` and `remove` work, removal leaves nothing behind; all tested. Do not start network
features until it is reviewed.

---

## Sources

- AppImage specification: [draft](https://github.com/AppImage/AppImageSpec/blob/master/draft.md),
  [FUSE troubleshooting](https://docs.appimage.org/user-guide/troubleshooting/fuse.html),
  [Electron sandboxing](https://docs.appimage.org/user-guide/troubleshooting/electron-sandboxing.html),
  [updates](https://docs.appimage.org/packaging-guide/optional/updates.html)
- [Fedora: Atomic Desktops drop FUSE 2](https://fedoraproject.org/wiki/Changes/AtomicDesktopDropFuse2)
- [AppImage type2-runtime](https://github.com/AppImage/type2-runtime)
- [Ubuntu discussion on AppImages and user namespaces](https://discourse.ubuntu.com/t/problem-runing-appimage-files-without-no-sandbox/78361)
- [AppImageHub catalogue](https://github.com/AppImage/appimage.github.io) and
  [feed](https://appimage.github.io/feed.json) (measured directly)
- [GitHub: release assets expose digests](https://github.blog/changelog/2025-06-03-releases-now-expose-digests-for-release-assets/)
- Existing tools: [AM](https://github.com/ivan-hc/AM), [Soar](https://github.com/pkgforge/soar),
  [Gear Lever](https://github.com/mijorus/gearlever), [AppManager](https://github.com/kem-a/AppManager),
  [AppImageLauncher alternatives](https://alternativeto.net/software/appimagelauncher/)
- Desktop-integration problem reports: [KDE 490657](https://bugs.kde.org/show_bug.cgi?id=490657),
  [LibreWolf](https://gitlab.com/librewolf-community/browser/appimage/-/issues/22),
  [AppImagePool](https://www.github.com/prateekmedia/appimagepool/issues/87)
