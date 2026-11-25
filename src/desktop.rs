//! Parse and re-emit the `.desktop` file bundled in an AppImage. We rewrite it instead of copying
//! it: `Exec`, `TryExec` and `Icon` must point at what *we* installed, paths must be quoted
//! correctly, and keys that make the desktop bypass `Exec` are dropped.

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub lines: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Desktop {
    pub sections: Vec<Section>,
}

pub fn parse(text: &str) -> Desktop {
    let mut d = Desktop::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            d.sections.push(Section {
                name: name.to_string(),
                lines: vec![],
            });
        } else if let (Some(sec), Some((k, v))) = (d.sections.last_mut(), line.split_once('=')) {
            let k = k.trim();
            let ok_key = !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-[]@_.".contains(c));
            if ok_key {
                sec.lines.push((k.to_string(), v.trim_start().to_string()));
            }
        }
    }
    d
}

impl Desktop {
    pub fn entry(&self) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == "Desktop Entry")
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entry()?
            .lines
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }
}

/// Split `Exec=` into the program and the raw remainder (arguments and `%` field codes), keeping
/// the remainder exactly as written so quoting inside it survives.
pub fn split_exec(exec: &str) -> (String, String) {
    let e = exec.trim();
    if let Some(rest) = e.strip_prefix('"') {
        let mut prog = String::new();
        let mut chars = rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        prog.push(n);
                    }
                }
                '"' => return (prog, rest[i + 1..].trim().to_string()),
                c => prog.push(c),
            }
        }
        (prog, String::new())
    } else {
        match e.split_once(char::is_whitespace) {
            Some((p, r)) => (p.to_string(), r.trim().to_string()),
            None => (e.to_string(), String::new()),
        }
    }
}

/// Quote one `Exec` argument per the Desktop Entry specification.
pub fn quote_exec_arg(s: &str) -> String {
    let special = |c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`%".contains(c);
    if !s.contains(special) {
        return s.to_string();
    }
    // Two layers of escaping apply (Desktop Entry spec, "The Exec key"): inside the quotes, `"` `` ` ``
    // `$` and `\` take a backslash; and because a backslash in any value must itself be written
    // `\\`, that backslash is doubled in the file. So an argument `a$b` is written `"a\\$b"`.
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"), // a literal % must be doubled
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Keys dropped because they would route around our `Exec` or describe the original AppDir.
fn dropped(key: &str) -> bool {
    let base = key.split('[').next().unwrap_or(key);
    matches!(
        base,
        "Exec" | "TryExec" | "Icon" | "Path" | "DBusActivatable" | "Hidden"
    ) || base.starts_with("X-AppImage")
        || base.starts_with("X-IFHAI")
}

/// Render our own entry from the bundled one.
pub fn render(d: &Desktop, slug: &str, launcher: &Path, icon: &str) -> String {
    let l = quote_exec_arg(&launcher.to_string_lossy());
    let mut out = String::from("# Managed by ifhai. Remove the app with: ifhai remove ");
    out.push_str(slug);
    out.push('\n');
    let mut wrote_entry = false;
    for s in &d.sections {
        let is_entry = s.name == "Desktop Entry";
        let is_action = s.name.starts_with("Desktop Action ");
        if !is_entry && !is_action {
            continue;
        }
        out.push_str(&format!("\n[{}]\n", s.name));
        for (k, v) in &s.lines {
            if dropped(k) {
                continue;
            }
            out.push_str(&format!("{k}={v}\n"));
        }
        let orig_exec = s
            .lines
            .iter()
            .find(|(k, _)| k == "Exec")
            .map(|(_, v)| v.as_str());
        let rest = orig_exec.map(|e| split_exec(e).1).unwrap_or_default();
        let exec = if rest.is_empty() {
            l.clone()
        } else {
            format!("{l} {rest}")
        };
        out.push_str(&format!("Exec={exec}\n"));
        if is_entry {
            wrote_entry = true;
            out.push_str(&format!(
                "TryExec={l}\nIcon={icon}\nX-IFHAI-ID={slug}\nX-IFHAI-Managed=true\n"
            ));
        } else {
            out.push_str(&format!("Icon={icon}\n"));
        }
    }
    if !wrote_entry {
        out.push_str(&format!("\n[Desktop Entry]\nType=Application\nName={slug}\nExec={l}\nIcon={icon}\nX-IFHAI-ID={slug}\nX-IFHAI-Managed=true\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[Desktop Entry]\nType=Application\nName=Obsidian\nName[de]=Obsidian DE\nComment=Notes\nExec=AppRun --no-sandbox %U\nTryExec=AppRun\nIcon=obsidian\nCategories=Office;\nMimeType=x-scheme-handler/obsidian;\nX-AppImage-Version=1.8.9\nX-AppImage-Integrate=false\nPath=/opt/obsidian\nDBusActivatable=true\n\n[Desktop Action new]\nName=New window\nExec=AppRun --new-window\n\n[Other]\nFoo=bar\n";

    #[test]
    fn parses_sections_and_values() {
        let d = parse(SAMPLE);
        assert_eq!(d.get("Name"), Some("Obsidian"));
        assert_eq!(d.get("X-AppImage-Version"), Some("1.8.9"));
        assert_eq!(d.sections.len(), 3);
        assert_eq!(
            parse("junk\nKey=1\n").sections.len(),
            0,
            "lines before any section are ignored"
        );
        assert_eq!(
            parse("[Desktop Entry]\nbad key=1\nGood=2\n")
                .entry()
                .unwrap()
                .lines
                .len(),
            1
        );
    }

    #[test]
    fn exec_splitting_and_quoting() {
        assert_eq!(
            split_exec("AppRun --x %U"),
            ("AppRun".into(), "--x %U".into())
        );
        assert_eq!(
            split_exec("\"/opt/my app/run\" %f"),
            ("/opt/my app/run".into(), "%f".into())
        );
        assert_eq!(split_exec("tool"), ("tool".into(), "".into()));
        assert_eq!(
            quote_exec_arg("/home/u/.local/bin/obsidian"),
            "/home/u/.local/bin/obsidian"
        );
        assert_eq!(
            quote_exec_arg("/home/my user/bin/x"),
            "\"/home/my user/bin/x\""
        );
        assert_eq!(
            quote_exec_arg("/a/b$c"),
            "\"/a/b\\\\$c\"",
            "file-level: two backslashes before the $"
        );
        assert_eq!(quote_exec_arg("/a/b\"c"), "\"/a/b\\\\\"c\"");
        assert_eq!(
            quote_exec_arg("/a/b\\c"),
            "\"/a/b\\\\\\\\c\"",
            "a literal backslash is four in the file"
        );
        assert_eq!(quote_exec_arg("/a/100%"), "\"/a/100%%\"");
    }

    #[test]
    fn rewrites_exec_icon_and_drops_dangerous_keys() {
        let out = render(
            &parse(SAMPLE),
            "obsidian",
            Path::new("/h/.local/bin/obsidian"),
            "ifhai-obsidian",
        );
        assert!(
            out.contains("Exec=/h/.local/bin/obsidian --no-sandbox %U\n"),
            "{out}"
        );
        assert!(
            out.contains("TryExec=/h/.local/bin/obsidian\n")
                && out.contains("Icon=ifhai-obsidian\n")
        );
        assert!(
            out.contains("Exec=/h/.local/bin/obsidian --new-window\n"),
            "actions are rewritten too: {out}"
        );
        assert!(
            out.contains("Name[de]=Obsidian DE")
                && out.contains("MimeType=x-scheme-handler/obsidian;")
        );
        for gone in [
            "X-AppImage",
            "Path=",
            "DBusActivatable",
            "AppRun",
            "Icon=obsidian\n",
            "[Other]",
        ] {
            assert!(!out.contains(gone), "{gone} must not survive: {out}");
        }
        assert!(out.contains("X-IFHAI-ID=obsidian") && out.contains("X-IFHAI-Managed=true"));
        assert_eq!(
            out.lines().filter(|l| l.starts_with("Exec=")).count(),
            2,
            "{out}"
        );
    }

    #[test]
    fn a_path_with_spaces_is_quoted_in_every_exec() {
        let out = render(
            &parse(SAMPLE),
            "obsidian",
            Path::new("/home/my user/.local/bin/obsidian"),
            "i",
        );
        assert!(
            out.contains("Exec=\"/home/my user/.local/bin/obsidian\" --no-sandbox %U\n"),
            "{out}"
        );
        assert!(out.contains("TryExec=\"/home/my user/.local/bin/obsidian\"\n"));
    }

    #[test]
    fn a_missing_desktop_entry_still_yields_a_usable_launcher() {
        let out = render(
            &Desktop::default(),
            "tool",
            Path::new("/b/tool"),
            "ifhai-tool",
        );
        assert!(
            out.contains("[Desktop Entry]")
                && out.contains("Exec=/b/tool\n")
                && out.contains("Name=tool")
        );
    }
}
