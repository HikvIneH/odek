//! Pure text transforms behind editor commands, kept out of AppKit so they
//! can be unit-tested.

fn leading_ws(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn code(line: &str) -> &str {
    line.trim_start_matches([' ', '\t'])
        .trim_end_matches(['\n', '\r'])
}

/// VS Code's ⌘/ on whole lines: if every non-blank line is commented,
/// uncomment them; otherwise comment them all at the shallowest indent.
/// Returns `None` when there's nothing to do (only blank lines).
pub fn toggle_line_comment(block: &str, prefix: &str) -> Option<String> {
    let lines: Vec<&str> = block.split_inclusive('\n').collect();
    let filled: Vec<&str> = lines.iter().copied().filter(|l| !code(l).is_empty()).collect();
    if filled.is_empty() {
        return None;
    }
    let uncomment = filled.iter().all(|l| code(l).starts_with(prefix));
    let indent = filled.iter().map(|l| leading_ws(l)).min().unwrap_or(0);
    let mut out = String::with_capacity(block.len() + lines.len() * (prefix.len() + 1));
    for l in lines {
        if code(l).is_empty() {
            out.push_str(l);
        } else if uncomment {
            let lead = leading_ws(l);
            let rest = &l[lead + prefix.len()..];
            out.push_str(&l[..lead]);
            out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        } else {
            out.push_str(&l[..indent]);
            out.push_str(prefix);
            out.push(' ');
            out.push_str(&l[indent..]);
        }
    }
    Some(out)
}

/// Whitespace that starts `line`, for auto-indent on Enter.
pub fn indent_of(line: &str) -> &str {
    &line[..leading_ws(line)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_at_shallowest_indent_and_keeps_blank_lines() {
        let src = "\tif x {\n\t\ty()\n\n\t}\n";
        let out = toggle_line_comment(src, "//").unwrap();
        assert_eq!(out, "\t// if x {\n\t// \ty()\n\n\t// }\n");
        assert_eq!(toggle_line_comment(&out, "//").unwrap(), src);
    }

    #[test]
    fn mixed_block_gets_commented_not_uncommented() {
        let out = toggle_line_comment("# a\nb\n", "#").unwrap();
        assert_eq!(out, "# # a\n# b\n");
    }

    #[test]
    fn uncomment_without_space_and_last_line_without_newline() {
        assert_eq!(toggle_line_comment("--a\n  -- b", "--").unwrap(), "a\n  b");
        assert_eq!(toggle_line_comment("% x", "%").unwrap(), "x");
    }

    #[test]
    fn blank_only_is_noop_and_indent() {
        assert!(toggle_line_comment("\n  \n", "//").is_none());
        assert_eq!(indent_of("\t  foo"), "\t  ");
        assert_eq!(indent_of("foo"), "");
    }
}
