use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Install(Vec<String>),
    Remove(Vec<String>),
    List,
    Run(String, Vec<String>),
    Update(Vec<String>),
    Updates,
    Rollback(String),
    Doctor,
    Help,
    Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opts {
    pub yes: bool,
    /// Copy the file instead of moving it.
    pub keep: bool,
    pub no_color: bool,
}

pub struct Invocation {
    pub cmd: Command,
    pub opts: Opts,
}

fn usage<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Usage(m.into()))
}

pub fn parse(args: &[String]) -> Result<Invocation> {
    let mut opts = Opts::default();
    let (mut verb, mut targets): (Option<String>, Vec<String>) = (None, vec![]);
    let (mut help, mut version, mut only_targets) = (false, false, false);
    for a in args {
        // Everything after `run <app>` belongs to the app, flags included.
        let passthrough = verb.as_deref() == Some("run") && !targets.is_empty();
        if passthrough || only_targets || !a.starts_with('-') || a == "-" {
            if verb.is_none() {
                verb = Some(a.clone())
            } else {
                targets.push(a.clone())
            }
        } else if a == "--" {
            only_targets = true;
        } else if let Some(long) = a.strip_prefix("--") {
            match long {
                "yes" | "noconfirm" => opts.yes = true,
                "keep" => opts.keep = true,
                "no-color" => opts.no_color = true,
                "help" => help = true,
                "version" => version = true,
                _ => return usage(format!("Unknown option '--{long}'.")),
            }
        } else {
            for c in a[1..].chars() {
                match c {
                    'y' => opts.yes = true,
                    'h' => help = true,
                    'V' => version = true,
                    _ => return usage(format!("Unknown option '-{c}'.")),
                }
            }
        }
    }
    if help {
        return Ok(Invocation {
            cmd: Command::Help,
            opts,
        });
    }
    if version {
        return Ok(Invocation {
            cmd: Command::Version,
            opts,
        });
    }
    let need = |name: &str, t: Vec<String>, what: &str| {
        if t.is_empty() {
            usage(format!("'{name}' needs {what}."))
        } else {
            Ok(t)
        }
    };
    let none = |name: &str, t: &[String]| {
        if t.is_empty() {
            Ok(())
        } else {
            usage(format!(
                "'{name}' does not take arguments (got '{}').",
                t[0]
            ))
        }
    };
    let cmd = match verb.as_deref() {
        None | Some("help") => Command::Help,
        Some("version") => Command::Version,
        Some("install") => Command::Install(need(
            "install",
            targets,
            "an AppImage file (or owner/repo, or an https:// link)",
        )?),
        Some("remove") => Command::Remove(need("remove", targets, "an app name")?),
        Some("update") => Command::Update(targets),
        Some("rollback") => match targets.as_slice() {
            [one] => Command::Rollback(one.clone()),
            _ => return usage("'rollback' needs exactly one app name."),
        },
        Some("run") => {
            let mut t = need("run", targets, "an app name")?;
            let app = t.remove(0);
            Command::Run(app, t)
        }
        Some(v @ ("list" | "updates" | "doctor")) => {
            none(v, &targets)?;
            match v {
                "list" => Command::List,
                "updates" => Command::Updates,
                _ => Command::Doctor,
            }
        }
        Some(other) => return usage(format!("Unknown command '{other}'. Try: ifhai --help")),
    };
    if opts.keep && !matches!(cmd, Command::Install(_)) {
        return usage("--keep only applies to 'install'.");
    }
    Ok(Invocation { cmd, opts })
}

pub const HELP: &str = "\
ifhai - makes a downloaded AppImage just work

USAGE
  ifhai install ~/Downloads/Some.AppImage

It makes the file executable, moves it to ~/Applications, adds it to your
app menu with its icon, adds a command for it, and sorts out FUSE.

COMMANDS
  install <file>...   Install AppImage files (also: owner/repo, or an https:// link)
  remove <app>...     Remove an app completely (its own settings are kept)
  list                Show installed apps
  run <app> [args]    Start an app
  updates             Show which apps have a newer release
  update [<app>...]   Update apps (default: all that can be updated)
  rollback <app>      Go back to the previous version
  doctor              Check that everything is set up properly

OPTIONS
  --keep              With install: copy the file instead of moving it
  --yes, --noconfirm  Do not ask for confirmation (required when not on a terminal)
  --no-color          Disable colour (NO_COLOR is also respected)
  -h, --help          Show this help
  -V, --version       Show the version

EXIT STATUS
  0 success   1 failure   2 usage error   3 not found   4 ambiguous
  5 confirmation required (use --yes)   6 cancelled
";

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &str) -> Result<Invocation> {
        parse(&a.split_whitespace().map(String::from).collect::<Vec<_>>())
    }
    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn commands() {
        assert_eq!(
            p("install a.AppImage b.AppImage").unwrap().cmd,
            Command::Install(v(&["a.AppImage", "b.AppImage"]))
        );
        assert_eq!(p("update").unwrap().cmd, Command::Update(vec![]));
        assert_eq!(
            p("rollback foo").unwrap().cmd,
            Command::Rollback("foo".into())
        );
        assert_eq!(p("doctor").unwrap().cmd, Command::Doctor);
        assert_eq!(p("").unwrap().cmd, Command::Help);
        assert_eq!(p("-V").unwrap().cmd, Command::Version);
        assert_eq!(p("install x -h").unwrap().cmd, Command::Help);
    }

    #[test]
    fn run_passes_everything_after_the_app_through() {
        assert_eq!(
            p("run obsidian --no-sandbox -y --help").unwrap().cmd,
            Command::Run("obsidian".into(), v(&["--no-sandbox", "-y", "--help"]))
        );
        assert_eq!(
            p("run obsidian").unwrap().cmd,
            Command::Run("obsidian".into(), vec![])
        );
        assert!(
            !p("run obsidian --yes").unwrap().opts.yes,
            "flags after the app are the app's"
        );
    }

    #[test]
    fn options() {
        let i = p("install a --keep -y --no-color").unwrap();
        assert_eq!(
            i.opts,
            Opts {
                yes: true,
                keep: true,
                no_color: true
            }
        );
        assert!(p("install a --noconfirm").unwrap().opts.yes);
    }

    #[test]
    fn usage_errors() {
        for bad in [
            "install",
            "remove",
            "run",
            "rollback",
            "rollback a b",
            "frobnicate",
            "list extra",
            "--bogus",
            "-x",
            "update --keep",
            "doctor --keep",
        ] {
            assert_eq!(p(bad).err().map(|e| e.exit_code()), Some(2), "{bad}");
        }
    }
}
