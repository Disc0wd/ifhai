# ifhai

*I fucking hate AppImages.*

Makes a downloaded AppImage just work.

You download a `.AppImage`. Normally you then `chmod +x` it, decide where to keep it, write or fake a
menu entry, hunt for an icon, and hope FUSE is installed. With ifhai:

```console
$ ifhai install ~/Downloads/Obsidian-1.8.9.AppImage
┃ Obsidian 1.8.9
┃ From     ~/Downloads/Obsidian-1.8.9.AppImage
┃ File     moved (the original is not kept) to ~/Applications
┃ Menu     an entry with its icon
┃ Command  obsidian

Install Obsidian 1.8.9? [Y/n]
✓ Installed Obsidian 1.8.9
  Start it from your app menu, or run: obsidian
```

That one command makes the file executable, moves it into `~/Applications`, adds it to your app menu
with its icon, adds an `obsidian` command, and works out whether it can run directly or needs to be
unpacked first (no FUSE required either way). Installing a newer file of the same app upgrades it in
place and keeps the old version so you can `ifhai rollback`. `ifhai remove` takes everything away
again.

There is **no config file** and almost no options. It decides things for you.

## Install ifhai

With [ifhx](https://github.com/Disc0wd/ifhx): `ifhx install ifhai`. Or download the release binary:

```bash
curl -fsSLO https://github.com/Disc0wd/ifhai/releases/latest/download/ifhai-x86_64-linux
curl -fsSL  https://github.com/Disc0wd/ifhai/releases/latest/download/ifhai-x86_64-linux.sha256 | sha256sum -c -
install -Dm755 ifhai-x86_64-linux ~/.local/bin/ifhai
```

Or build it: `cargo install --git https://github.com/Disc0wd/ifhai`.

## Commands

| Command | What it does |
|:---|:---|
| `ifhai install <app>...` | Install an AppImage file, **an app name** (`ifhai install obsidian`), `owner/repo` (latest GitHub release) or an `https://` link |
| `ifhai search <text>` | Find an app by name in the AppImage catalogue |
| `ifhai remove <app>...` | Remove an app completely. Its own settings and data are not touched |
| `ifhai list` | Installed apps |
| `ifhai run <app> [args]` | Start an app (everything after the name is passed to it) |
| `ifhai updates` | Which apps have a newer release |
| `ifhai update [<app>...]` | Update them (default: all that can be) |
| `ifhai rollback <app>` | Go back to the previous version (run it again to return) |
| `ifhai doctor` | Check that everything is set up properly |

Options: `--keep` (install: copy instead of move), `--yes` / `--noconfirm`, `--no-color`.
`--yes` never accepts an unverified download (see Safety).
Exit status: `0` ok, `1` failure, `2` usage, `3` not found, `4` ambiguous, `5` confirmation
required (use `--yes` when not on a terminal), `6` cancelled.

## What it does for you

- **Executable and placed.** The file is validated (ELF plus the AppImage marker) and moved to
  `~/Applications/<app>.AppImage`, so nothing is left in Downloads. `--keep` copies instead.
- **A real menu entry.** The `.desktop` file bundled in the AppImage is *rewritten*, not copied:
  `Exec`, `TryExec` and every action point at ifhai's launcher, `Icon` at an installed icon, paths
  are quoted correctly, and keys that bypass `Exec` (`DBusActivatable`, `Path`...) are dropped.
  Exactly one entry per app, so upgrades never leave a duplicate.
- **A command.** `~/.local/bin/<app>`. It warns if that directory is not on your `PATH`. If a command
  of that name already exists, or a file is already there that ifhai did not create, the app's command
  is `ifhai-<app>` instead and ifhai tells you.
- **Badly packaged apps.** A missing or incomplete menu file still gives a valid, visible entry; icons
  are searched for where packages actually put them, and empty or broken ones are skipped for a
  generic icon. ifhai says when it had to fill something in. A package with no `AppRun` cannot start
  and is refused.
- **Wrong processor caught.** A 32-bit or ARM build that this computer cannot run is reported as
  such, before anything is executed.
- **FUSE handled.** Old AppImage runtimes need FUSE 2, which many distros no longer install, and
  every runtime needs a `fusermount` to mount itself. ifhai detects both from the file and the
  system. If the host cannot run the file directly it unpacks the app once and runs it from there;
  otherwise it runs the file directly. (In a sample of 125 current AppImages none needed FUSE 2, so
  this mostly rescues old files and bare systems. See [docs/DATA.md](docs/DATA.md).)
- **Updates without setup.** AppImages that embed GitHub update information update with
  `ifhai update`, and so does anything installed with `ifhai install owner/repo`. Checking for
  updates follows GitHub's `releases/latest` redirect and only uses the API when there is something
  to fetch, so the 60-requests-an-hour limit is not a problem. (Set `GITHUB_TOKEN` to raise it.)
- **Double-click works.** The first install adds an "Install with ifhai" entry to your file
  manager's *Open With* list for AppImage files (it is not made the default, and it disappears with
  the last app). Choose it on a downloaded file and a terminal opens, shows the plan and installs.
- **Update source found automatically.** Chromium-based browsers record where a file came from on
  the file itself. If that was a GitHub release, ifhai uses it as the exact update source, with no
  `owner/repo` step. (Firefox does not record this, so a file from Firefox only updates if it embeds
  update information or was installed with `owner/repo`.)
- **Download progress.** Big downloads show a progress bar on a terminal; update checks run in
  parallel.
- **Install by name.** `ifhai install obsidian` looks the app up in the AppImageHub catalogue (kept
  in a local cache and refreshed weekly), checks the real GitHub release before offering anything
  (about 15% of catalogue entries no longer publish an AppImage, and those are never offered),
  verifies the download, and remembers where it came from. If several apps match equally, it asks.
- **More apps update on their own.** Besides embedded update information and the download URL a
  browser records, ifhai reads the GitHub repos named in an app's own metadata and uses one as the
  update source only after its latest release is verified to contain a file matching what you
  installed. A repo that merely looks related is rejected, and each app is tried once. Installing a
  newer file of an app you already follow keeps following the same place.
- **Asks when a release has several candidates** (say a normal and a `-legacy` build) instead of
  giving up, and remembers the choice for updates.
- **Delta updates.** If `zsync` is installed and the release publishes a `.zsync` file, an update
  downloads only what changed. The result is checked against the published checksum, and anything
  wrong falls back to a normal download. (Not exercised against a real `zsync` yet: this machine
  does not have it, so it is covered by tests with a stand-in only.)
- **Exact removal.** Everything ifhai creates for an app is recorded. Removal deletes exactly that,
  and refuses any path outside ifhai's own folders, even if the record is damaged.

## Why bother

Measured, not claimed ([docs/DATA.md](docs/DATA.md)): one command instead of about nine; the menu
entry actually launches (a hand-made one fails when your home path has a space, and the validator
doesn't notice); upgrades don't leave duplicate entries; you can roll back and remove cleanly; 85%
of releases can be checksum-verified automatically. It is not faster in any way you would notice,
both take milliseconds.

## Safety

- Downloads are **HTTPS only**. When GitHub publishes a SHA-256 for a release file (its API does
  since June 2025, or a checksum file in the release), ifhai verifies it and shows
  "verified" in the plan. When none exists the plan says **"unverified"**, and ifhai installs it only
  after you answer yes at a terminal: `--yes` does not apply, and pressing Enter means no. An
  unattended `update` applies the verified updates, skips the unverified ones, says which, and exits
  non-zero.
  A checksum published next to the file protects against corruption, not against a compromised
  repository.
- **Nothing it did not create is replaced**: not a command in `~/.local/bin`, and not an app file in
  `~/Applications`. An app never takes over the name of an existing command such as `git`.
- Responses are size-limited, text from the catalogue and from AppImages is stripped of terminal
  control characters, and an update source that now redirects to a different repository is refused.
- To read an AppImage's name and icon, ifhai runs the AppImage's own runtime with
  `--appimage-extract` after you confirm the install. That runs the file's runtime (not the app),
  which is what every AppImage manager does. Only install files you trust. Before you confirm,
  nothing is executed: the checks use the file's bytes only.
- Every step is staged and committed with an undo: a failure at any point leaves the previous
  version, your download and your menu exactly as they were.
- No root, no daemon, nothing outside your home directory.

## Where things go

```text
~/Applications/<app>.AppImage              the app (visible; browse it like any folder)
~/Applications/.ifhai/<app>/               previous version for rollback, unpacked files if needed
~/.local/bin/<app>                         the command
~/.local/share/applications/ifhai-<app>.desktop   menu entry
~/.local/share/icons/hicolor/…/ifhai-<app>.png    icon
~/.local/share/ifhai/state.toml            what is installed
```

## Development

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

103 tests: ELF/AppImage parsing against hostile input, `.desktop` rewriting, asset selection and
checksums, and every command end to end (fake AppImages, mock network and system, plus a test that
installs under every failure mode and checks nothing is left behind). Tests never touch your real
home directory or the network.

## Not in this version

- Updates for an app that names no GitHub repo, was not installed from one, and came from a browser
  that does not record the download URL (Firefox). ifhai says so rather than guess.
- Sandboxing. `doctor` explains Electron sandbox problems; it never turns a sandbox off for you.
- Release binaries are built for x86_64.

## Licence

[MIT](LICENSE). Use, copy, modify and redistribute this code for any purpose, including commercial
use, as long as the copyright notice and licence text stay with it (that notice is the credit).
