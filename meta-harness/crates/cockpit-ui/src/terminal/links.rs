//! What on a terminal's line can be gone to: files (`src/a.rs`,
//! `src/a.rs:12`, `src/a.rs:12:5`, an absolute path in the project) and web
//! links.

/// Something on a line to go to: where it starts (its column) and how long
/// it is, and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub col: u16,
    pub len: u16,
    pub to: To,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum To {
    /// A file (from the project's folder), at a line when it says one.
    File { path: String, line: Option<u32> },
    Url(String),
}

/// Characters that end a word a link could be.
fn breaks(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '|')
}

/// The links on `text` (a line as shown: a column a character), its paths
/// read from `folder` (an absolute path in it becomes the project's).
pub fn links(text: &str, folder: &str) -> Vec<Link> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if breaks(chars[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < chars.len() && !breaks(chars[end]) {
            end += 1;
        }
        // What a sentence puts after a word isn't part of it.
        let mut stop = end;
        while stop > start && matches!(chars[stop - 1], '.' | ':' | '!' | '?') {
            stop -= 1;
        }
        let word: String = chars[start..stop].iter().collect();
        if let Some(to) = link(&word, folder) {
            out.push(Link { col: start as u16, len: (stop - start) as u16, to });
        }
        start = end;
    }
    out
}

fn link(word: &str, folder: &str) -> Option<To> {
    if word.starts_with("http://") || word.starts_with("https://") {
        return (word.len() > "https://".len()).then(|| To::Url(word.to_owned()));
    }
    // file, file:line, file:line:col
    let mut parts = word.split(':');
    let path = parts.next()?;
    let line = parts.next().map(|l| l.parse::<u32>().ok());
    if matches!(line, Some(None)) || parts.next().is_some_and(|c| c.parse::<u32>().is_err()) {
        return None;
    }
    let plausible = !path.starts_with('-')
        && path.chars().all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~' | '+' | '@'))
        && path.chars().any(char::is_alphabetic)
        && (path.contains('/') || has_extension(path));
    if !plausible {
        return None;
    }
    let path = match path.strip_prefix(folder.trim_end_matches('/')).and_then(|p| p.strip_prefix('/')) {
        Some(inside) => inside.to_owned(),
        None => path.trim_start_matches("./").to_owned(),
    };
    Some(To::File { path, line: line.flatten() })
}

/// `name.ext`: an extension of letters (and digits, a letter first) after a
/// dot: not a version number, a duration (`0.5s`) or a sentence's end.
fn has_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && ext.len() <= 8 && ext.chars().all(char::is_alphanumeric) && ext.starts_with(char::is_alphabetic),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, line: Option<u32>) -> To {
        To::File { path: path.into(), line }
    }

    #[test]
    fn files_lines_and_web_links_are_found_where_they_are() {
        let found = links("error: src/a.rs:12:5: expected `;` (see https://doc.rust-lang.org/x).", "/p");
        assert_eq!(found.iter().map(|l| (l.col, l.to.clone())).collect::<Vec<_>>(), [(7, file("src/a.rs", Some(12))), (40, To::Url("https://doc.rust-lang.org/x".into()))]);
        assert_eq!(links("Cargo.toml  src", "/p")[0], Link { col: 0, len: 10, to: file("Cargo.toml", None) });
        assert_eq!(links("/p/crates/x/lib.rs:3", "/p")[0].to, file("crates/x/lib.rs", Some(3)), "an absolute path in the project is the project's");
        assert_eq!(links("./build.sh", "/p")[0].to, file("build.sh", None));
        assert!(links("version 1.2.3 took 0.5s, done.", "/p").is_empty(), "numbers and sentences aren't files");
        assert!(links("--release -v", "/p").is_empty(), "flags aren't");
        assert!(links("a.rs:x", "/p").is_empty(), "a line is a number");
    }
}
