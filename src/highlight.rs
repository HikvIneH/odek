//! Syntax highlighting: tree-sitter grammars compiled in, configured lazily
//! the first time a file of that language is opened.

use std::cell::RefCell;
use std::path::Path;
use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Colour slots the UI maps to system colours (so light/dark just works).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Property,
    Escape,
    Heading,
    Link,
}

/// Capture names we recognise, each mapped to a colour. tree-sitter-highlight
/// matches the longest dotted prefix, so `function.method` → `function`.
const NAMES: &[(&str, Color)] = &[
    ("comment", Color::Comment),
    ("keyword", Color::Keyword),
    ("conditional", Color::Keyword),
    ("repeat", Color::Keyword),
    ("include", Color::Keyword),
    ("exception", Color::Keyword),
    ("storageclass", Color::Keyword),
    ("variable.builtin", Color::Keyword),
    ("boolean", Color::Number),
    ("string", Color::String),
    ("string.special.key", Color::Property),
    ("character", Color::String),
    ("text.literal", Color::String),
    ("markup.raw", Color::String),
    ("number", Color::Number),
    ("float", Color::Number),
    ("constant", Color::Number),
    ("function", Color::Function),
    ("method", Color::Function),
    ("type", Color::Type),
    ("constructor", Color::Type),
    ("tag", Color::Type),
    ("module", Color::Type),
    ("namespace", Color::Type),
    ("property", Color::Property),
    ("attribute", Color::Property),
    ("label", Color::Property),
    ("escape", Color::Escape),
    ("string.escape", Color::Escape),
    ("text.title", Color::Heading),
    ("markup.heading", Color::Heading),
    ("text.uri", Color::Link),
    ("text.reference", Color::Link),
    ("markup.link", Color::Link),
];

/// A coloured run in UTF-16 units (what NSString ranges use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Go,
    TypeScript,
    Tsx,
    JavaScript,
    Rust,
    Swift,
    Json,
    Markdown,
    Sql,
    Latex,
    Python,
    Bash,
    Css,
    Html,
    Yaml,
}

const LANG_COUNT: usize = 15;

impl Lang {
    pub fn detect(path: &Path) -> Option<Lang> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".zprofile"
        ) {
            return Some(Lang::Bash);
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "go" => Lang::Go,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "rs" => Lang::Rust,
            "swift" => Lang::Swift,
            "json" | "jsonc" => Lang::Json,
            "md" | "markdown" => Lang::Markdown,
            "sql" => Lang::Sql,
            "tex" | "sty" | "cls" | "ltx" => Lang::Latex,
            "py" => Lang::Python,
            "sh" | "bash" | "zsh" => Lang::Bash,
            "css" => Lang::Css,
            "html" | "htm" => Lang::Html,
            "yaml" | "yml" => Lang::Yaml,
            _ => return None,
        })
    }

    fn config(self) -> Option<&'static HighlightConfiguration> {
        static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; LANG_COUNT] =
            [const { OnceLock::new() }; LANG_COUNT];
        CONFIGS[self as usize].get_or_init(|| build(self)).as_ref()
    }
}

const LATEX_HIGHLIGHTS: &str = r#"
[(line_comment) (block_comment) (comment_environment)] @comment
(command_name) @function
(begin command: _ @keyword name: (curly_group_text (text) @type))
(end command: _ @keyword name: (curly_group_text (text) @type))
[(section) (subsection) (subsubsection) (chapter) (part) (paragraph)] @text.title
[(inline_formula) (displayed_equation) (math_environment)] @string
[(package_include) (class_include)] @keyword
(label_definition) @property
(label_reference) @property
(citation) @property
[(curly_group_path) (curly_group_uri)] @text.uri
"#;

fn build(lang: Lang) -> Option<HighlightConfiguration> {
    let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let (language, name, query): (tree_sitter::Language, &str, String) = match lang {
        Lang::Go => (
            tree_sitter_go::LANGUAGE.into(),
            "go",
            tree_sitter_go::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            format!("{}\n{}", tree_sitter_typescript::HIGHLIGHTS_QUERY, js),
        ),
        Lang::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            "tsx",
            format!(
                "{}\n{}\n{}",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                js
            ),
        ),
        Lang::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            format!("{}\n{}", tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, js),
        ),
        Lang::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Swift => (
            tree_sitter_swift::LANGUAGE.into(),
            "swift",
            tree_sitter_swift::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Json => (
            tree_sitter_json::LANGUAGE.into(),
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            "markdown",
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
        ),
        Lang::Sql => (
            tree_sitter_sequel::LANGUAGE.into(),
            "sql",
            tree_sitter_sequel::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Latex => (
            codebook_tree_sitter_latex::LANGUAGE.into(),
            "latex",
            LATEX_HIGHLIGHTS.into(),
        ),
        Lang::Python => (
            tree_sitter_python::LANGUAGE.into(),
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Bash => (
            tree_sitter_bash::LANGUAGE.into(),
            "bash",
            tree_sitter_bash::HIGHLIGHT_QUERY.into(),
        ),
        Lang::Css => (
            tree_sitter_css::LANGUAGE.into(),
            "css",
            tree_sitter_css::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Html => (
            tree_sitter_html::LANGUAGE.into(),
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY.into(),
        ),
        Lang::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            "yaml",
            tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
        ),
    };
    match HighlightConfiguration::new(language, name, &query, "", "") {
        Ok(mut cfg) => {
            let names: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
            cfg.configure(&names);
            Some(cfg)
        }
        Err(e) => {
            eprintln!("{name} highlight query failed: {e:?}");
            None
        }
    }
}

/// Highlight `text`, returning coloured spans in UTF-16 offsets.
pub fn highlight(lang: Lang, text: &str) -> Vec<Span> {
    thread_local! {
        static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
    }
    let Some(cfg) = lang.config() else {
        return Vec::new();
    };
    HIGHLIGHTER.with_borrow_mut(|hl| {
        let Ok(events) = hl.highlight(cfg, text.as_bytes(), None, |_| None) else {
            return Vec::new();
        };
        let mut spans: Vec<Span> = Vec::new();
        let mut stack: Vec<Color> = Vec::new();
        // Events arrive in order, so convert byte offsets to UTF-16 incrementally.
        let (mut byte_pos, mut u16_pos) = (0usize, 0u32);
        let mut to_u16 = |b: usize| -> u32 {
            if b > byte_pos {
                let chunk = &text.as_bytes()[byte_pos..b];
                u16_pos += if chunk.is_ascii() {
                    chunk.len() as u32
                } else {
                    text[byte_pos..b].encode_utf16().count() as u32
                };
                byte_pos = b;
            }
            u16_pos
        };
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(NAMES[h.0].1),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    let s = to_u16(start);
                    let e = to_u16(end);
                    let Some(&color) = stack.last() else { continue };
                    if e <= s {
                        continue;
                    }
                    match spans.last_mut() {
                        Some(p) if p.color == color && p.start + p.len == s => p.len += e - s,
                        _ => spans.push(Span {
                            start: s,
                            len: e - s,
                            color,
                        }),
                    }
                }
                Err(_) => break,
            }
        }
        spans
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_configures() {
        for lang in [
            Lang::Go,
            Lang::TypeScript,
            Lang::Tsx,
            Lang::JavaScript,
            Lang::Rust,
            Lang::Swift,
            Lang::Json,
            Lang::Markdown,
            Lang::Sql,
            Lang::Latex,
            Lang::Python,
            Lang::Bash,
            Lang::Css,
            Lang::Html,
            Lang::Yaml,
        ] {
            assert!(lang.config().is_some(), "{:?} failed to configure", lang);
        }
    }

    #[test]
    fn go_keywords_and_strings() {
        let spans = highlight(Lang::Go, "package main\n// hi\nvar s = \"x\"\n");
        assert!(spans.iter().any(|s| s.color == Color::Keyword && s.start == 0));
        assert!(spans.iter().any(|s| s.color == Color::Comment));
        assert!(spans.iter().any(|s| s.color == Color::String));
    }

    #[test]
    fn utf16_offsets_after_multibyte() {
        // "é" is 2 bytes / 1 UTF-16 unit; "😀" is 4 bytes / 2 units.
        let src = "// é😀\nvar x = 1\n";
        let spans = highlight(Lang::Go, src);
        let var = spans.iter().find(|s| s.color == Color::Keyword).unwrap();
        assert_eq!(var.start, "// é😀\n".encode_utf16().count() as u32);
    }

    #[test]
    fn latex_commands() {
        let spans = highlight(Lang::Latex, "\\section{Hi}\n% c\n\\textbf{x} $a+b$\n");
        assert!(spans.iter().any(|s| s.color == Color::Comment));
        assert!(spans.iter().any(|s| s.color == Color::String));
    }
}
