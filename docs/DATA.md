# Data: why not just do it by hand?

Measured 2026-11-25 on a laptop (i5-10310U, Fedora 45). Scripts: `bench/`. Nothing downloaded here
was executed; files were only read.

**Summary:** ifhai is not faster in any way that matters (both take milliseconds). Its value is that
a manual setup is ~9 steps that fail silently, and that it handles what comes *after*: updates,
upgrades, rollback, removal.

## 1. Effort

| | By hand | ifhai |
|:---|---:|---:|
| Commands to make one download a launchable app | ~9 (`chmod`, `mkdir`, `mv`, unpack, copy icon, write/edit `.desktop`, `ln -s`, refresh database) | **1** |
| To upgrade it later | repeat, then hunt down the old menu entry | `ifhai install new.AppImage` (or `ifhai update`) |
| To undo an upgrade | not possible unless you kept the old file | `ifhai rollback app` |
| To remove it | remember every file you created | `ifhai remove app` |

## 2. Time (hyperfine, 20 runs, scripted manual steps in `bench/manual.sh`)

| AppImage | By hand (script) | ifhai |
|:---|---:|---:|
| tiny test app (16 KB) | 17.0 ms | **4.8 ms** |
| median real size (120 MB) | 17.7 ms | **13.2 ms** |

Both are instant; ifhai spawns fewer processes. An earlier version took **1.0 s** on the 120 MB file
because it hashed the whole file just to install it (this CPU has no SHA hardware: `sha256sum` alone
takes 0.48 s). It now hashes only when it must: verifying a download, or when an installed file of
the same size needs comparing.

## 3. Correctness: does the menu entry actually launch?

The test machine's home directory contained a space (common on some setups and in work/WSL paths).

| Method | Menu entry launches? | `desktop-file-validate` |
|:---|:---:|:---:|
| **ifhai** | **yes** | 0 messages |
| Scripted manual steps | **no**: unquoted path, program resolves to `.../my` | 0 messages |
| "Just copy the AppImage's own `.desktop`" | **no**: `Exec=AppRun` | 0 messages |

The validator cannot tell, so these failures are silent: the entry shows up in the menu and does
nothing. ifhai rewrites `Exec`/`TryExec`/actions and quotes per the Desktop Entry specification.

## 4. What real AppImages look like (random sample of the AppImageHub catalogue)

150 random repos from the 3,997 catalogue entries that link to GitHub; each one's latest release,
and the first 2 MB of each x86_64 AppImage read with ifhai's own parser (`bench/survey.py`,
`examples/survey.rs`).

| Finding | Result |
|:---|---:|
| Catalogue entry is still installable (latest release has an x86_64 AppImage) | 128 / 150 (85%) |
| ...latest release has no AppImage at all | 11 (7%) |
| ...repo has no releases | 11 (7%) |
| GitHub publishes a SHA-256 for the file, so ifhai can verify it | 109 / 128 (85%) |
| A checksum file in the release instead | 1 (1%) |
| Nothing to verify against (shown as *unverified*) | 18 (14%) |
| A release has several equally good x86_64 files (the user must choose) | 3 (2%) |
| Valid AppImages among those read | 125 / 125 |
| Format: type 2 / type 1 | 124 / 1 |
| **Runtime needs FUSE 2 (cannot start without it)** | **0 / 125 (0%)** |
| Embed update information | 10 / 125 (8%), 9 of them GitHub (usable by ifhai) |
| Median / largest file | 120 MB / 1.2 GB |
| Median age of the latest release | 26 days (16% are over a year old) |

**What this means, honestly:**

- The widely reported **FUSE 2 problem is mostly a problem of old files**, not current releases: none
  of 125 sampled current AppImages needed it, because modern runtimes are static. ifhai still handles
  it (and the related case of a host with no `fusermount` at all), but do not expect it to matter
  for fresh downloads.
- Only **8%** of AppImages carry their own update information, so "update from the file alone" works
  for few apps. Installing with `ifhai install owner/repo` gives every GitHub app an update path.
- **85%** of installable releases can be checksum-verified automatically.
- **15% of catalogue entries are dead**, which matters for any future "install by name": a catalogue
  alone would send people to releases that no longer exist.

Caveats: one random sample of 150 (seed 7), only repos that link to GitHub, latest releases only,
no check that the apps run. Fixture AppImages for the timing and menu tests are built from
`bench/fake.c`; no third-party binary was executed.
