# ifhai

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
| `ifhai install <file>...` | Install AppImage files. Also takes `owner/repo` (latest GitHub release) or an `https://` link |
| `ifhai remove <app>...` | Remove an app completely. Its own settings and data are not touched |
| `ifhai list` | Installed apps |
| `ifhai run <app> [args]` | Start an app (everything after the name is passed to it) |
| `ifhai updates` | Which apps have a newer release |
| `ifhai update [<app>...]` | Update them (default: all that can be) |
| `ifhai rollback <app>` | Go back to the previous version (run it again to return) |
| `ifhai doctor` | Check that everything is set up properly |

Options: `--keep` (install: copy instead of move), `--yes` / `--noconfirm`, `--no-color`.
Exit status: `0` ok, `1` failure, `2` usage, `3` not found, `4` ambiguous, `5` confirmation
required (use `--yes` when not on a terminal), `6` cancelled.

## What it does for you

- **Executable and placed.** The file is validated (ELF plus the AppImage marker) and moved to
  `~/Applications/<app>.AppImage`, so nothing is left in Downloads. `--keep` copies instead.
- **A real menu entry.** The `.desktop` file bundled in the AppImage is *rewritten*, not copied:
  `Exec`, `TryExec` and every action point at ifhai's launcher, `Icon` at an installed icon, paths
  are quoted correctly, and keys that bypass `Exec` (`DBusActivatable`, `Path`...) are dropped.
  Exactly one entry per app, so upgrades never leave a duplicate.
- **A command.** `~/.local/bin/<app>`. It warns if that directory is not on your `PATH`.
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
  "verified" in the plan. When none exists the plan says **"unverified"**: you decide.
  A checksum published next to the file protects against corruption, not against a compromised
  repository.
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

81 tests: ELF/AppImage parsing against hostile input, `.desktop` rewriting, asset selection and
checksums, and every command end to end (fake AppImages, mock network and system, plus a test that
installs under every failure mode and checks nothing is left behind). Tests never touch your real
home directory or the network.

## Not in this version

- Searching a catalogue and installing by name (`ifhai install obsidian`): install from a file,
  `owner/repo` or a link for now. See [spec.md](spec.md).
- Updates for apps that have no GitHub source. Say so, rather than guess.
- Sandboxing. `doctor` explains Electron sandbox problems; it never turns a sandbox off for you.
- Release binaries are built for x86_64.
- A licence file has not been chosen yet.
