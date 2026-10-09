//! Find in the scrollback. Pure: reads a `Term`, returns where the query is.
//!
//! Soft-wrapped lines are matched as one logical line, so a match can cross a
//! wrap; it is then reported as one segment per physical line (same `m`).
//! Only one logical line's text is held at a time — no copy of the scrollback.

use super::grid::flag;
use super::term::Term;

/// Stop collecting after this many matches (bounds memory on huge buffers).
pub const MAX_MATCHES: usize = 10_000;

/// One highlighted run of cells on one line; `end` is exclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Seg {
    /// Stable line id (see `Term::first_id`).
    pub id: u64,
    pub start: usize,
    pub end: usize,
    /// Match number; segments of a match that crosses a wrap share it.
    pub m: u32,
}

#[derive(Default)]
pub struct Found {
    /// In document order.
    pub segs: Vec<Seg>,
    pub count: usize,
    /// Hit `MAX_MATCHES`; later matches are missing.
    pub capped: bool,
}

impl Found {
    /// Segments of match `m`.
    pub fn of(&self, m: usize) -> &[Seg] {
        let lo = self.segs.partition_point(|s| (s.m as usize) < m);
        let hi = self.segs.partition_point(|s| (s.m as usize) <= m);
        &self.segs[lo..hi]
    }
}

fn fold(c: char, ci: bool) -> char {
    if !ci {
        return c;
    }
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// Case-insensitive unless `query` has an uppercase letter (smart case).
pub fn find(t: &Term, query: &str) -> Found {
    let mut out = Found::default();
    if query.is_empty() {
        return out;
    }
    let ci = !query.chars().any(char::is_uppercase);
    let q: Vec<char> = query.chars().map(|c| fold(c, ci)).collect();
    let first = t.first_id();
    let total = t.total_lines();
    // One logical line: folded chars, and for each the (line offset, first col, cols).
    let mut hay: Vec<char> = Vec::new();
    let mut at: Vec<(u32, u32, u8)> = Vec::new();
    let mut i = 0;
    while i < total {
        hay.clear();
        at.clear();
        let base = i;
        loop {
            let line = t.line(i);
            for (col, c) in line.cells.iter().enumerate() {
                if c.flags & flag::SPACER != 0 {
                    continue;
                }
                let w = if c.flags & flag::WIDE != 0 { 2 } else { 1 };
                let rec = ((i - base) as u32, col as u32, w);
                if c.flags & flag::CLUSTER != 0 {
                    for ch in t.clusters.get(c.ch).chars() {
                        hay.push(fold(ch, ci));
                        at.push(rec);
                    }
                } else {
                    hay.push(fold(char::from_u32(c.ch).unwrap_or(' '), ci));
                    at.push(rec);
                }
            }
            i += 1;
            if !line.wrapped || i >= total {
                break;
            }
        }
        let mut s = 0;
        while s + q.len() <= hay.len() {
            if hay[s..s + q.len()] != q[..] {
                s += 1;
                continue;
            }
            if out.count >= MAX_MATCHES {
                out.capped = true;
                return out;
            }
            let m = out.count as u32;
            out.count += 1;
            let mut seg: Option<Seg> = None;
            for &(l, col, w) in &at[s..s + q.len()] {
                let (id, a, b) = (first + (base as u64 + l as u64), col as usize, (col + w as u32) as usize);
                match &mut seg {
                    Some(g) if g.id == id => g.end = g.end.max(b),
                    _ => {
                        out.segs.extend(seg.take());
                        seg = Some(Seg { id, start: a, end: b, m });
                    }
                }
            }
            out.segs.extend(seg);
            s += q.len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(cols: usize, rows: usize, bytes: &str) -> Term {
        let mut t = Term::new(cols, rows);
        vte::Parser::new().advance(&mut t, bytes.as_bytes());
        t
    }

    fn spans(f: &Found) -> Vec<(u64, usize, usize)> {
        f.segs.iter().map(|s| (s.id, s.start, s.end)).collect()
    }

    #[test]
    fn smart_case() {
        let t = term(20, 3, "Hello hello\r\nHELLO");
        assert_eq!(find(&t, "hello").count, 3);
        assert_eq!(find(&t, "Hello").count, 1);
        assert_eq!(find(&t, "").count, 0);
    }

    #[test]
    fn finds_in_scrollback_with_stable_ids() {
        let mut t = term(10, 2, "");
        for i in 0..6 {
            vte::Parser::new().advance(&mut t, format!("n{i}\r\n").as_bytes());
        }
        t.history.max_lines = 3;
        vte::Parser::new().advance(&mut t, b"n9\r\n");
        let f = find(&t, "n9");
        assert_eq!(f.count, 1);
        assert_eq!(f.segs[0].id, t.first_id() + (t.total_lines() - 2) as u64);
        assert_eq!(find(&t, "n0").count, 0, "evicted");
    }

    #[test]
    fn wide_chars_count_two_cells() {
        let t = term(10, 2, "a世界b");
        assert_eq!(spans(&find(&t, "界b")), [(0, 3, 6)]);
    }

    #[test]
    fn cluster_is_one_cell() {
        let t = term(10, 2, "x\u{1F468}\u{200D}\u{1F469}y");
        let f = find(&t, "y");
        assert_eq!(f.segs[0].start, 3, "x + 2-wide cluster");
    }

    #[test]
    fn crosses_soft_wrap_as_two_segments() {
        let t = term(5, 3, "hello world");
        let f = find(&t, "o wo");
        assert_eq!(f.count, 1);
        assert_eq!(spans(&f), [(0, 4, 5), (1, 0, 3)]);
        assert_eq!(f.of(0).len(), 2);
    }

    #[test]
    fn hard_newline_does_not_join() {
        let t = term(5, 3, "ab\r\ncd");
        assert_eq!(find(&t, "bc").count, 0);
    }

    #[test]
    fn non_overlapping_and_capped() {
        let t = term(10, 2, "aaaa");
        assert_eq!(find(&t, "aa").count, 2);
        let mut t = Term::new(80, 40);
        t.history.max_lines = 100_000;
        let line = "x ".repeat(40);
        for _ in 0..400 {
            vte::Parser::new().advance(&mut t, format!("{line}\r\n").as_bytes());
        }
        let f = find(&t, "x");
        assert_eq!(f.count, MAX_MATCHES);
        assert!(f.capped);
    }
}
