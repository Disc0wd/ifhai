//! Report what ifhai's parser sees in the first bytes of AppImage files (used by bench/survey.py).
//! Reads files only; nothing is executed.
use ifhai::appimage::{Kind, github_source, inspect};

fn main() {
    for p in std::env::args().skip(1) {
        let data = std::fs::read(&p).unwrap_or_default();
        match inspect(&data) {
            Ok(i) => println!(
                "{p}\tok\t{}\t{}\t{}\t{}",
                if i.kind == Kind::Type2 {
                    "type2"
                } else {
                    "type1"
                },
                i.runtime_needs_fuse2,
                i.update_info
                    .as_deref()
                    .map(|u| u.split('|').next().unwrap_or(""))
                    .unwrap_or("none"),
                i.update_info.as_deref().and_then(github_source).is_some()
            ),
            Err(e) => println!("{p}\tinvalid\t{e}"),
        }
    }
}
