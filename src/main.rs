use ifhai::commands::{self, Env, Io};
use ifhai::fetch::Curl;
use ifhai::state::Dirs;
use ifhai::system::RealSystem;
use std::io::{BufRead, IsTerminal, Write};

struct StdIo;

fn no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

impl Io for StdIo {
    fn out(&mut self, s: &str) {
        if std::io::stdout().lock().write_all(s.as_bytes()).is_err() {
            std::process::exit(0); // closed pipe
        }
    }
    fn err(&mut self, s: &str) {
        let _ = std::io::stderr().write_all(s.as_bytes());
    }
    fn ask(&mut self, prompt: &str) -> Option<String> {
        let mut o = std::io::stdout().lock();
        let _ = o.write_all(prompt.as_bytes());
        let _ = o.flush();
        drop(o);
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_string()),
        }
    }
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal()
    }
    fn stdout_is_tty(&self) -> bool {
        std::io::stdout().is_terminal()
    }
    fn color_allowed(&self) -> bool {
        self.stdout_is_tty() && !no_color_env()
    }
    fn stderr_is_tty(&self) -> bool {
        std::io::stderr().is_terminal()
    }
    fn stderr_color_allowed(&self) -> bool {
        self.stderr_is_tty() && !no_color_env()
    }
    fn unicode(&self) -> bool {
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .is_some_and(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("utf-8") || v.contains("utf8")
            })
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dirs = match Dirs::from_env() {
        Ok(d) => d,
        Err(e) => {
            if args.iter().any(|a| a == "-V" || a == "--version") {
                println!("ifhai {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            eprintln!("ifhai: {e}");
            std::process::exit(e.exit_code());
        }
    };
    let host = RealSystem::host();
    let mut io = StdIo;
    let mut env = Env {
        fetcher: &Curl,
        system: &RealSystem,
        dirs: &dirs,
        host: &host,
        io: &mut io,
        self_exe: std::env::current_exe().unwrap_or_else(|_| "ifhai".into()),
    };
    std::process::exit(commands::run(&args, &mut env));
}
