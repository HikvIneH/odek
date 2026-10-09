//! Finding URLs and file paths in a line of text. Pure functions, run lazily
//! for the line under the mouse only.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Path {
        path: String,
        line: Option<u32>,
        col: Option<u32>,
    },
}

const SCHEMES: [&str; 4] = ["https://", "http://", "file://", "mailto:"];

/// The link under `col`, given one char per cell ('\0' for wide-char
/// spacers). Returns the cell range `start..end` and where it points.
pub fn detect(chars: &[char], col: usize) -> Option<(usize, usize, Target)> {
    let is_sep = |c: char| c.is_whitespace() || "<>\"`|".contains(c);
    if chars.get(col).is_none_or(|&c| is_sep(c)) {
        return None;
    }
    let mut a = col;
    while a > 0 && !is_sep(chars[a - 1]) {
        a -= 1;
    }
    let mut b = col + 1;
    while b < chars.len() && !is_sep(chars[b]) {
        b += 1;
    }
    let (s, e, target) = classify(chars, a, b)?;
    (s <= col && col < e).then_some((s, e, target))
}

fn classify(chars: &[char], mut a: usize, mut b: usize) -> Option<(usize, usize, Target)> {
    let text = |a: usize, b: usize| chars[a..b].iter().filter(|&&c| c != '\0').collect::<String>();
    // A URL may sit inside brackets or after "key=": find its scheme.
    let url_at = (a..b).find(|&i| {
        let rest: String = chars[i..b]
            .iter()
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase();
        SCHEMES.iter().any(|s| rest.starts_with(s))
    });
    if let Some(i) = url_at {
        a = i;
        b = trim_end(chars, a, b);
        let url = text(a, b);
        let min = SCHEMES
            .iter()
            .find(|s| url.to_ascii_lowercase().starts_with(*s))
            .map_or(0, |s| s.len());
        return (url.len() > min).then_some((a, b, Target::Url(url)));
    }
    while a < b && "([{'".contains(chars[a]) {
        a += 1;
    }
    b = trim_end(chars, a, b);
    // Optional :line[:col] suffix.
    let (mut end, mut nums) = (b, [None, None]);
    for slot in (0..2).rev() {
        let digits = chars[a..end]
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digits == 0 || digits > 9 || end - digits <= a + 1 || chars[end - digits - 1] != ':' {
            break;
        }
        nums[slot] = text(end - digits, end).parse::<u32>().ok();
        end -= digits + 1;
    }
    let (line, col) = match nums {
        [Some(l), Some(c)] => (Some(l), Some(c)),
        [None, Some(l)] => (Some(l), None),
        _ => (None, None),
    };
    let path = text(a, end);
    looks_like_path(&path).then_some((a, b, Target::Path { path, line, col }))
}

/// Drop trailing punctuation; a closing bracket stays when it is balanced.
fn trim_end(chars: &[char], a: usize, mut b: usize) -> usize {
    while b > a {
        let c = chars[b - 1];
        let open = match c {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            '\0' => ' ',
            _ if ".,;:'!?".contains(c) => ' ',
            _ => break,
        };
        let count = |x: char| chars[a..b].iter().filter(|&&c| c == x).count();
        if open == ' ' || count(c) > count(open) {
            b -= 1;
        } else {
            break;
        }
    }
    b
}

fn looks_like_path(p: &str) -> bool {
    if p.len() < 2 || p.contains("://") || p.starts_with('-') {
        return false;
    }
    if p.starts_with('/') || p.starts_with("~/") || p.starts_with("./") || p.starts_with("../") {
        return !p.trim_matches(['/', '.']).is_empty();
    }
    if p.contains('/') {
        return p.split('/').any(|s| !s.is_empty() && s != "." && s != "..");
    }
    // README.md, main.rs: a dot-extension that is not just a number.
    match p.rsplit_once('.') {
        Some((name, ext)) => {
            ext.len() <= 8
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
                && name.chars().any(|c| c.is_alphanumeric())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str, col: usize) -> Option<(usize, usize, Target)> {
        let chars: Vec<char> = s.chars().collect();
        detect(&chars, col)
    }

    fn url(s: &str, col: usize) -> Option<String> {
        match at(s, col)? {
            (a, b, Target::Url(u)) => {
                assert!(a <= col && col < b);
                Some(u)
            }
            _ => None,
        }
    }

    fn path(s: &str, col: usize) -> Option<(String, Option<u32>, Option<u32>)> {
        match at(s, col)? {
            (_, _, Target::Path { path, line, col }) => Some((path, line, col)),
            _ => None,
        }
    }

    fn p(s: &str, col: usize, want: &str, line: Option<u32>, c: Option<u32>) {
        assert_eq!(path(s, col), Some((want.to_string(), line, c)), "{s}");
    }

    #[test]
    fn urls() {
        assert_eq!(
            url("see https://example.com/a?b=1#c now", 8).as_deref(),
            Some("https://example.com/a?b=1#c")
        );
        assert_eq!(url("see https://example.com/a?b=1#c now", 3), None);
        assert_eq!(
            url("(https://example.com).", 3).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            url("https://en.wikipedia.org/wiki/Rust_(language).", 5).as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        assert_eq!(
            url("[docs](https://x.io/d)", 10).as_deref(),
            Some("https://x.io/d")
        );
        assert_eq!(
            url("go to http://localhost:3000, ok", 8).as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(url("mail mailto:a@b.co;", 7).as_deref(), Some("mailto:a@b.co"));
        assert_eq!(url("file:///tmp/x.txt", 0).as_deref(), Some("file:///tmp/x.txt"));
        assert_eq!(url("https://", 2), None);
    }

    #[test]
    fn paths() {
        p("src/term/view.rs", 3, "src/term/view.rs", None, None);
        p("at src/main.rs:42:7: error", 5, "src/main.rs", Some(42), Some(7));
        p("src/main.rs:42", 0, "src/main.rs", Some(42), None);
        p("src/main.rs:42:", 0, "src/main.rs", Some(42), None);
        p("see ./x and ../y.", 5, "./x", None, None);
        p("see ./x and ../y.", 14, "../y", None, None);
        p("edit README.md.", 7, "README.md", None, None);
        p("'/usr/local/bin/odek'", 5, "/usr/local/bin/odek", None, None);
        p("~/Developer/x", 1, "~/Developer/x", None, None);
        p("(src/a.rs)", 3, "src/a.rs", None, None);
        p("a.rs:1: x", 0, "a.rs", Some(1), None);
    }

    #[test]
    fn not_links() {
        assert_eq!(at("hello world", 1), None);
        assert_eq!(at("version 1.2.3 and 3.14", 9), None);
        assert_eq!(at("a  b", 1), None);
        assert_eq!(at("-rw-r--r--", 2), None);
        assert_eq!(at("/", 0), None);
        assert_eq!(at("ssh://host/x", 3), None);
        assert_eq!(at("x", 5), None);
    }

    #[test]
    fn wide_chars_keep_columns() {
        // A double-width char occupies two cells: the char and a '\0' spacer.
        let chars: Vec<char> = "世\0 src/世\0.rs".chars().collect();
        let (a, b, t) = detect(&chars, 4).unwrap();
        assert_eq!((a, b), (3, chars.len()));
        assert_eq!(
            t,
            Target::Path {
                path: "src/世.rs".into(),
                line: None,
                col: None
            }
        );
    }
}
