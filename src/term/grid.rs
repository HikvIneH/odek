//! Cells, styles and the line store. A cell is 8 bytes: the character (or an
//! index into the grapheme-cluster table), a style index and flags. Styles are
//! interned per terminal, so a screen full of the same colour costs nothing
//! extra. Lines pushed into scrollback are trimmed of trailing blanks.

use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

pub mod attr {
    pub const BOLD: u16 = 1;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const HIDDEN: u16 = 1 << 5;
    pub const STRIKE: u16 = 1 << 6;
    pub const BLINK: u16 = 1 << 7;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
    /// OSC 8 hyperlink: index into `Term::link`, 0 = none.
    pub link: u16,
}

pub mod flag {
    /// First half of a double-width character.
    pub const WIDE: u16 = 1;
    /// Second half of a double-width character; draws nothing.
    pub const SPACER: u16 = 1 << 1;
    /// `ch` is an index into the cluster table, not a code point.
    pub const CLUSTER: u16 = 1 << 2;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(C)]
pub struct Cell {
    pub ch: u32,
    pub style: u16,
    pub flags: u16,
}

const _: () = assert!(size_of::<Cell>() == 8);

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ' as u32, style: 0, flags: 0 };

    pub fn blank(style: u16) -> Cell {
        Cell { ch: ' ' as u32, style, flags: 0 }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' ' as u32 && self.flags == 0 && self.style == 0
    }
}

#[derive(Clone, Default, Debug)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// The text continues on the next line (soft wrap).
    pub wrapped: bool,
}

impl Line {
    pub fn new(cols: usize, fill: Cell) -> Line {
        Line { cells: vec![fill; cols], wrapped: false }
    }

    /// A copy without trailing default blanks, sized exactly, for scrollback.
    pub fn trimmed(&self) -> Line {
        let end = self.cells.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1);
        Line { cells: self.cells[..end].to_vec(), wrapped: self.wrapped }
    }

    pub fn reset(&mut self, cols: usize, fill: Cell) {
        self.cells.clear();
        self.cells.resize(cols, fill);
        self.wrapped = false;
    }

    pub fn bytes(&self) -> usize {
        self.cells.capacity() * size_of::<Cell>() + size_of::<Line>()
    }
}

/// Style interning. Index 0 is always the default style.
pub struct Styles {
    list: Vec<Style>,
    map: HashMap<Style, u16>,
}

impl Default for Styles {
    fn default() -> Self {
        let mut map = HashMap::new();
        map.insert(Style::default(), 0);
        Styles { list: vec![Style::default()], map }
    }
}

impl Styles {
    pub fn intern(&mut self, s: Style) -> u16 {
        if let Some(&i) = self.map.get(&s) {
            return i;
        }
        // Full table: fall back to the default style rather than grow.
        if self.list.len() >= u16::MAX as usize {
            return 0;
        }
        let i = self.list.len() as u16;
        self.list.push(s);
        self.map.insert(s, i);
        i
    }

    pub fn get(&self, i: u16) -> Style {
        self.list.get(i as usize).copied().unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
}

/// Multi-code-point characters (emoji sequences, combining marks), interned.
#[derive(Default)]
pub struct Clusters {
    list: Vec<Box<str>>,
    map: HashMap<Box<str>, u32>,
}

impl Clusters {
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(s.into());
        self.map.insert(s.into(), i);
        i
    }

    pub fn get(&self, i: u32) -> &str {
        self.list.get(i as usize).map_or("", |s| s)
    }

    pub fn bytes(&self) -> usize {
        self.list.iter().map(|s| s.len() * 2 + 48).sum()
    }
}

/// Scrollback: trimmed lines, capped by count and by bytes, oldest dropped.
pub struct History {
    pub lines: VecDeque<Line>,
    bytes: usize,
    pub max_lines: usize,
    pub max_bytes: usize,
    /// Lines ever dropped from the front, so `evicted + index` is a stable id.
    pub evicted: u64,
}

impl History {
    pub fn new(max_lines: usize, max_bytes: usize) -> History {
        History { lines: VecDeque::new(), bytes: 0, max_lines, max_bytes, evicted: 0 }
    }

    pub fn push(&mut self, line: Line) {
        if self.max_lines == 0 {
            self.evicted += 1;
            return;
        }
        self.bytes += line.bytes();
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines || (self.bytes > self.max_bytes && self.lines.len() > 1) {
            if let Some(old) = self.lines.pop_front() {
                self.bytes -= old.bytes();
                self.evicted += 1;
            }
        }
    }

    pub fn pop_back(&mut self) -> Option<Line> {
        let line = self.lines.pop_back()?;
        self.bytes -= line.bytes();
        Some(line)
    }

    pub fn clear(&mut self) {
        self.evicted += self.lines.len() as u64;
        self.lines = VecDeque::new();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes + self.lines.capacity() * size_of::<Line>()
    }
}

/// The visible screen: exactly `rows` lines of exactly `cols` cells.
pub struct Grid {
    pub lines: Vec<Line>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Grid {
        Grid { lines: (0..rows).map(|_| Line::new(cols, Cell::BLANK)).collect() }
    }

    pub fn bytes(&self) -> usize {
        self.lines.iter().map(Line::bytes).sum()
    }
}

/// Plain text of a line: spacers skipped, clusters expanded, trailing spaces kept.
pub fn line_text(line: &Line, clusters: &Clusters, out: &mut String) {
    for c in &line.cells {
        if c.flags & flag::SPACER != 0 {
            continue;
        }
        if c.flags & flag::CLUSTER != 0 {
            out.push_str(clusters.get(c.ch));
        } else {
            out.push(char::from_u32(c.ch).unwrap_or(' '));
        }
    }
}
