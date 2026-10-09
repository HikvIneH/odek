//! The terminal state machine: `vte` parses bytes, `Term` applies them to the
//! grid. No AppKit here, so it is tested headless.

use std::collections::HashMap;

use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

use super::grid::{Cell, Clusters, Color, Grid, History, Line, Style, Styles, attr, flag, line_text};

pub const DEFAULT_SCROLLBACK: usize = 10_000;
pub const DEFAULT_SCROLLBACK_BYTES: usize = 8 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MouseMode {
    #[default]
    Off,
    /// 1000: press and release.
    Click,
    /// 1002: plus motion while a button is held.
    Drag,
    /// 1003: all motion.
    Motion,
}

#[derive(Clone, Copy, Debug)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub autowrap: bool,
    pub show_cursor: bool,
    pub bracketed_paste: bool,
    pub sync: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
    pub focus_events: bool,
    pub insert: bool,
    pub origin: bool,
    pub newline: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            app_keypad: false,
            autowrap: true,
            show_cursor: true,
            bracketed_paste: false,
            sync: false,
            mouse: MouseMode::Off,
            mouse_sgr: false,
            focus_events: false,
            insert: false,
            origin: false,
            newline: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Bell,
    Title(String),
    Cwd(String),
    Notify(String),
    Clipboard(String),
}

#[derive(Clone, Copy, Default)]
struct Cursor {
    row: usize,
    col: usize,
    pen: Style,
    /// Last column was written; the next printable wraps first.
    pending_wrap: bool,
}

#[derive(Clone, Copy, Default)]
struct Saved {
    cursor: Cursor,
    origin: bool,
    dec_graphics: bool,
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    main: Grid,
    alt: Grid,
    pub alt_active: bool,
    pub history: History,
    pub styles: Styles,
    pub clusters: Clusters,
    links: Vec<Box<str>>,
    link_map: HashMap<Box<str>, u16>,
    cursor: Cursor,
    saved_main: Saved,
    saved_alt: Saved,
    top: usize,
    bottom: usize,
    tabs: Vec<bool>,
    pub modes: Modes,
    pub cursor_shape: CursorShape,
    pub title: String,
    pub events: Vec<Event>,
    /// Bytes to send back to the program (device reports).
    pub reply: Vec<u8>,
    /// Rows changed since the view last drew.
    pub dirty: Vec<bool>,
    /// The whole screen moved (scroll); redraw everything.
    pub all_dirty: bool,
    /// Background and foreground reported to OSC 10/11 queries.
    pub report_fg: (u8, u8, u8),
    pub report_bg: (u8, u8, u8),
    dec_graphics: bool,
    last_char: Option<char>,
    /// The previous character was a zero-width joiner: the next one joins too.
    join_next: bool,
    /// The last resize reflowed the main screen: line ids changed, so the
    /// view should drop its scroll anchor. Cleared by the view.
    pub reflowed: bool,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Term {
        let (cols, rows) = (cols.max(2), rows.max(1));
        Term {
            cols,
            rows,
            main: Grid::new(cols, rows),
            alt: Grid::new(cols, rows),
            alt_active: false,
            history: History::new(DEFAULT_SCROLLBACK, DEFAULT_SCROLLBACK_BYTES),
            styles: Styles::default(),
            clusters: Clusters::default(),
            links: Vec::new(),
            link_map: HashMap::new(),
            cursor: Cursor::default(),
            saved_main: Saved::default(),
            saved_alt: Saved::default(),
            top: 0,
            bottom: rows - 1,
            tabs: default_tabs(cols),
            modes: Modes::default(),
            cursor_shape: CursorShape::Block,
            title: String::new(),
            events: Vec::new(),
            reply: Vec::new(),
            dirty: vec![true; rows],
            all_dirty: true,
            report_fg: (0xE6, 0xED, 0xF3),
            report_bg: (0x0D, 0x11, 0x17),
            dec_graphics: false,
            last_char: None,
            join_next: false,
            reflowed: false,
        }
    }

    // ---- reading (for the view and tests) ----

    pub fn grid(&self) -> &Grid {
        if self.alt_active { &self.alt } else { &self.main }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    pub fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col)
    }

    /// Number of lines addressable: scrollback (main screen only) + screen.
    pub fn total_lines(&self) -> usize {
        if self.alt_active {
            self.rows
        } else {
            self.history.len() + self.rows
        }
    }

    /// Line by index into `0..total_lines()`.
    pub fn line(&self, i: usize) -> &Line {
        if self.alt_active {
            &self.alt.lines[i]
        } else if i < self.history.len() {
            &self.history.lines[i]
        } else {
            &self.main.lines[i - self.history.len()]
        }
    }

    /// Stable id of the first line in `0..total_lines()` (scrollback eviction shifts indices).
    pub fn first_id(&self) -> u64 {
        if self.alt_active { 0 } else { self.history.evicted }
    }

    #[cfg_attr(not(feature = "selftest"), allow(dead_code))]
    pub fn screen_text(&self) -> String {
        let mut s = String::new();
        for line in &self.grid().lines {
            line_text(line, &self.clusters, &mut s);
            let end = s.trim_end_matches(' ').len();
            s.truncate(end);
            s.push('\n');
        }
        s
    }

    /// Bytes held by this terminal's grids, scrollback and tables.
    #[cfg_attr(not(feature = "selftest"), allow(dead_code))]
    pub fn mem_bytes(&self) -> usize {
        self.main.bytes()
            + self.alt.bytes()
            + self.history.bytes()
            + self.clusters.bytes()
            + self.styles.len() * 24
            + self.links.iter().map(|l| l.len() * 2 + 48).sum::<usize>()
    }

    /// Target of an OSC 8 hyperlink id (see `Style::link`).
    pub fn link(&self, id: u16) -> Option<&str> {
        self.links.get((id as usize).checked_sub(1)?).map(|s| &**s)
    }

    fn intern_link(&mut self, uri: &str) -> u16 {
        if let Some(&i) = self.link_map.get(uri) {
            return i;
        }
        // Bounded so a program can't grow the table without limit.
        if uri.len() > MAX_LINK_LEN || self.links.len() >= MAX_LINKS {
            return 0;
        }
        self.links.push(uri.into());
        let i = self.links.len() as u16;
        self.link_map.insert(uri.into(), i);
        i
    }

    pub fn take_dirty(&mut self) -> (bool, Vec<bool>) {
        let all = std::mem::take(&mut self.all_dirty);
        let rows = std::mem::replace(&mut self.dirty, vec![false; self.rows]);
        (all, rows)
    }

    // ---- resizing ----

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if cols == self.cols && rows == self.rows {
            return;
        }
        if cols != self.cols {
            self.reflow_main(cols, rows);
        }
        for grid in [&mut self.main, &mut self.alt] {
            for line in &mut grid.lines {
                fit_width(line, cols);
            }
        }
        // Main screen: shrink by dropping blank rows below the cursor first,
        // then pushing top rows into scrollback; grow by pulling them back.
        let alt = self.alt_active;
        let mut crow = if alt {
            self.saved_main.cursor.row
        } else {
            self.cursor.row
        };
        while self.main.lines.len() > rows {
            let last = self.main.lines.len() - 1;
            if last > crow && self.main.lines[last].cells.iter().all(Cell::is_blank) {
                self.main.lines.pop();
            } else {
                let line = self.main.lines.remove(0);
                self.history.push(line.trimmed());
                crow = crow.saturating_sub(1);
            }
        }
        while self.main.lines.len() < rows {
            if !alt && let Some(mut line) = self.history.pop_back() {
                fit_width(&mut line, cols);
                self.main.lines.insert(0, line);
                crow += 1;
            } else {
                self.main.lines.push(Line::new(cols, Cell::BLANK));
            }
        }
        self.alt.lines.resize_with(rows, || Line::new(cols, Cell::BLANK));
        if alt {
            self.saved_main.cursor.row = crow.min(rows - 1);
        } else {
            self.cursor.row = crow.min(rows - 1);
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.pending_wrap = false;
        for s in [&mut self.saved_main, &mut self.saved_alt] {
            s.cursor.row = s.cursor.row.min(rows - 1);
            s.cursor.col = s.cursor.col.min(cols - 1);
        }
        self.dirty = vec![true; rows];
        self.all_dirty = true;
    }

    /// Re-wrap scrollback and the main screen to `cols`: soft-wrapped runs are
    /// joined into logical lines and split again. The cursor follows its
    /// character and the screen stays anchored to the bottom.
    fn reflow_main(&mut self, cols: usize, rows: usize) {
        let alt = self.alt_active;
        let cur = if alt { self.saved_main.cursor } else { self.cursor };
        let old_cols = self.cols;
        let hist = self.history.take();
        let cursor_idx = hist.len() + cur.row;
        let screen = std::mem::take(&mut self.main.lines);
        let mut src = hist.into_iter().chain(screen).enumerate().peekable();

        let mut out: Vec<Line> = Vec::new();
        let mut buf: Vec<Cell> = Vec::new();
        let mut cur_off: Option<usize> = None;
        let mut cursor_at = (0usize, 0usize);
        while let Some((idx, mut line)) = src.next() {
            let wrapped = line.wrapped;
            if wrapped && line.cells.len() < old_cols {
                line.cells.resize(old_cols, Cell::BLANK);
            }
            // A wide character that did not fit left a blank pad: not content.
            if wrapped
                && line.cells.last().is_some_and(Cell::is_blank)
                && src
                    .peek()
                    .is_some_and(|(_, n)| n.cells.first().is_some_and(|c| c.flags & flag::WIDE != 0))
            {
                line.cells.pop();
            }
            if idx == cursor_idx {
                cur_off = Some(buf.len() + cur.col + usize::from(cur.pending_wrap));
            }
            buf.extend_from_slice(&line.cells);
            if wrapped && src.peek().is_some() {
                continue;
            }
            let len = buf.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1);
            let mut start = 0;
            loop {
                let mut end = (start + cols).min(len);
                if end > start && end < len && buf[end - 1].flags & flag::WIDE != 0 {
                    end -= 1;
                }
                let more_text = end < len;
                let limit = if more_text { end } else { start + cols };
                if let Some(o) = cur_off
                    && o >= start
                    && o < limit
                {
                    cursor_at = (out.len(), o - start);
                }
                let more_cursor = cur_off.is_some_and(|o| o >= limit);
                let last = !more_text && !more_cursor;
                let lo = start.min(len);
                let hi = end.max(lo);
                let cut = buf[lo..hi]
                    .iter()
                    .rposition(|c| !c.is_blank())
                    .map_or(0, |i| i + 1);
                out.push(Line {
                    cells: buf[lo..lo + cut].to_vec(),
                    wrapped: !last,
                });
                if last {
                    break;
                }
                start = limit;
            }
            cur_off = None;
            buf.clear();
            if buf.capacity() > 1 << 16 {
                buf.shrink_to(1 << 12);
            }
        }
        // Blank lines below the cursor are not content.
        while out.len() > cursor_at.0 + 1 && out.last().is_some_and(|l| l.cells.is_empty() && !l.wrapped) {
            out.pop();
        }
        let n = out.len();
        let top = n.saturating_sub(rows).min(cursor_at.0);
        for line in out.drain(..top) {
            self.history.push(line);
        }
        out.truncate(rows);
        for line in &mut out {
            line.cells.resize(cols, Cell::BLANK);
        }
        out.resize_with(rows, || Line::new(cols, Cell::BLANK));
        out.shrink_to_fit();
        self.main.lines = out;
        let (row, col) = (cursor_at.0 - top, cursor_at.1);
        let c = if alt {
            &mut self.saved_main.cursor
        } else {
            &mut self.cursor
        };
        c.row = row;
        c.col = col;
        c.pending_wrap = false;
        self.reflowed = true;
    }

    // ---- primitives ----

    fn touch(&mut self, row: usize) {
        if let Some(d) = self.dirty.get_mut(row) {
            *d = true;
        }
    }

    fn touch_range(&mut self, from: usize, to: usize) {
        for r in from..=to.min(self.rows - 1) {
            self.touch(r);
        }
    }

    /// Blank cell carrying the current background (erase uses it, like xterm).
    fn erase_cell(&mut self) -> Cell {
        let pen = self.cursor.pen;
        if pen.bg == Color::Default {
            Cell::BLANK
        } else {
            Cell::blank(self.styles.intern(Style {
                bg: pen.bg,
                ..Style::default()
            }))
        }
    }

    /// Scroll [top, bottom] up by n. Lines leaving the top of the main
    /// screen go to scrollback when `keep` (line feeds), not for deletions.
    fn scroll_up_in(&mut self, top: usize, bottom: usize, n: usize, keep: bool) {
        let n = n.min(bottom - top + 1);
        let fill = self.erase_cell();
        let cols = self.cols;
        let to_history = keep && top == 0 && !self.alt_active;
        for i in 0..n {
            if to_history {
                let line = self.main.lines[top + i].trimmed();
                self.history.push(line);
            }
        }
        let grid = self.grid_mut();
        grid.lines[top..=bottom].rotate_left(n);
        for line in &mut grid.lines[bottom + 1 - n..=bottom] {
            line.reset(cols, fill);
        }
        if top == 0 && bottom == self.rows - 1 {
            self.all_dirty = true;
        }
        self.touch_range(top, bottom);
    }

    fn scroll_down_in(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom - top + 1);
        let fill = self.erase_cell();
        let cols = self.cols;
        let grid = self.grid_mut();
        grid.lines[top..=bottom].rotate_right(n);
        for line in &mut grid.lines[top..top + n] {
            line.reset(cols, fill);
        }
        self.touch_range(top, bottom);
    }

    fn linefeed(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.bottom {
            self.scroll_up_in(self.top, self.bottom, 1, true);
        } else if self.cursor.row < self.rows - 1 {
            self.cursor.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.top {
            self.scroll_down_in(self.top, self.bottom, 1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    fn goto(&mut self, row: usize, col: usize) {
        let (min, max) = if self.modes.origin {
            (self.top, self.bottom)
        } else {
            (0, self.rows - 1)
        };
        self.cursor.row = (row + if self.modes.origin { self.top } else { 0 }).clamp(min, max);
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    /// Clear cells [from, to) on `row`, fixing any wide character cut in half.
    fn erase_cells(&mut self, row: usize, from: usize, to: usize) {
        let fill = self.erase_cell();
        let cols = self.cols;
        let line = &mut self.grid_mut().lines[row];
        let to = to.min(cols);
        if from >= to {
            return;
        }
        if line.cells[from].flags & flag::SPACER != 0 && from > 0 {
            line.cells[from - 1] = fill;
        }
        if to < cols && line.cells[to].flags & flag::SPACER != 0 {
            line.cells[to] = fill;
        }
        line.cells[from..to].fill(fill);
        if to == cols {
            line.wrapped = false;
        }
        self.touch(row);
    }

    fn print_char(&mut self, c: char) {
        let c = if self.dec_graphics { dec_special(c) } else { c };
        let width = if (c as u32) < 0x7f {
            1
        } else {
            c.width().unwrap_or(0)
        };
        // Skin tones and the parts of a ZWJ sequence belong to the emoji before.
        let joins = std::mem::take(&mut self.join_next) || is_emoji_modifier(c);
        if width == 0 || (joins && self.last_char.is_some()) {
            self.join_next = c == '\u{200D}';
            self.combine(c);
            return;
        }
        if self.cursor.pending_wrap && self.modes.autowrap {
            let row = self.cursor.row;
            self.grid_mut().lines[row].wrapped = true;
            self.cursor.col = 0;
            self.linefeed();
        }
        if width == 2 && self.cursor.col == self.cols - 1 {
            if self.modes.autowrap {
                let (row, col) = (self.cursor.row, self.cursor.col);
                self.erase_cells(row, col, col + 1);
                self.grid_mut().lines[row].wrapped = true;
                self.cursor.col = 0;
                self.linefeed();
            } else {
                return;
            }
        }
        let (row, col) = (self.cursor.row, self.cursor.col);
        let cols = self.cols;
        if self.modes.insert {
            let line = &mut self.grid_mut().lines[row];
            line.cells[col..].rotate_right(width);
        }
        // Overwriting half of a wide character clears the other half.
        self.erase_cells(row, col, (col + width).min(cols));
        let style = self.styles.intern(self.cursor.pen);
        let line = &mut self.grid_mut().lines[row];
        if width == 2 {
            line.cells[col] = Cell {
                ch: c as u32,
                style,
                flags: flag::WIDE,
            };
            line.cells[col + 1] = Cell {
                ch: ' ' as u32,
                style,
                flags: flag::SPACER,
            };
        } else {
            line.cells[col] = Cell {
                ch: c as u32,
                style,
                flags: 0,
            };
        }
        self.touch(row);
        self.last_char = Some(c);
        if col + width >= cols {
            self.cursor.col = cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.col = col + width;
        }
    }

    /// Zero-width character: attach to the previous cell's cluster.
    fn combine(&mut self, c: char) {
        let row = self.cursor.row;
        let mut col = if self.cursor.pending_wrap {
            self.cursor.col
        } else if self.cursor.col > 0 {
            self.cursor.col - 1
        } else {
            return;
        };
        let cell = self.grid().lines[row].cells[col];
        if cell.flags & flag::SPACER != 0 && col > 0 {
            col -= 1;
        }
        let cell = self.grid().lines[row].cells[col];
        let mut s = if cell.flags & flag::CLUSTER != 0 {
            self.clusters.get(cell.ch).to_string()
        } else {
            char::from_u32(cell.ch).map(String::from).unwrap_or_default()
        };
        s.push(c);
        let idx = self.clusters.intern(&s);
        let cell = &mut self.grid_mut().lines[row].cells[col];
        cell.ch = idx;
        cell.flags |= flag::CLUSTER;
        self.touch(row);
    }

    fn save_cursor(&mut self) {
        let saved = Saved {
            cursor: self.cursor,
            origin: self.modes.origin,
            dec_graphics: self.dec_graphics,
        };
        if self.alt_active {
            self.saved_alt = saved;
        } else {
            self.saved_main = saved;
        }
    }

    fn restore_cursor(&mut self) {
        let saved = if self.alt_active {
            self.saved_alt
        } else {
            self.saved_main
        };
        self.cursor = saved.cursor;
        self.cursor.row = self.cursor.row.min(self.rows - 1);
        self.cursor.col = self.cursor.col.min(self.cols - 1);
        self.modes.origin = saved.origin;
        self.dec_graphics = saved.dec_graphics;
    }

    fn set_alt(&mut self, on: bool, save: bool, clear: bool) {
        if on == self.alt_active {
            return;
        }
        if on {
            if save {
                self.save_cursor();
            }
            self.alt_active = true;
            if clear {
                let cols = self.cols;
                for line in &mut self.alt.lines {
                    line.reset(cols, Cell::BLANK);
                }
            }
        } else {
            self.alt_active = false;
            if save {
                self.restore_cursor();
            }
        }
        self.top = 0;
        self.bottom = self.rows - 1;
        self.dirty = vec![true; self.rows];
        self.all_dirty = true;
    }

    fn reset(&mut self) {
        let (cols, rows) = (self.cols, self.rows);
        let history = std::mem::replace(&mut self.history, History::new(0, 0));
        let title = std::mem::take(&mut self.title);
        let (fg, bg) = (self.report_fg, self.report_bg);
        *self = Term::new(cols, rows);
        self.history = history;
        self.title = title;
        self.report_fg = fg;
        self.report_bg = bg;
    }

    fn set_mode(&mut self, private: bool, mode: u16, on: bool) {
        if !private {
            match mode {
                4 => self.modes.insert = on,
                20 => self.modes.newline = on,
                _ => {}
            }
            return;
        }
        match mode {
            1 => self.modes.app_cursor = on,
            6 => {
                self.modes.origin = on;
                self.goto(0, 0);
            }
            7 => self.modes.autowrap = on,
            25 => {
                self.modes.show_cursor = on;
                self.touch(self.cursor.row);
            }
            47 => self.set_alt(on, false, false),
            1047 => self.set_alt(on, false, on),
            1048 => {
                if on {
                    self.save_cursor()
                } else {
                    self.restore_cursor()
                }
            }
            1049 => self.set_alt(on, true, true),
            1000 => self.modes.mouse = if on { MouseMode::Click } else { MouseMode::Off },
            1002 => self.modes.mouse = if on { MouseMode::Drag } else { MouseMode::Off },
            1003 => self.modes.mouse = if on { MouseMode::Motion } else { MouseMode::Off },
            1004 => self.modes.focus_events = on,
            1006 => self.modes.mouse_sgr = on,
            2004 => self.modes.bracketed_paste = on,
            2026 => {
                self.modes.sync = on;
                if !on {
                    self.all_dirty = true;
                }
            }
            _ => {}
        }
    }

    fn mode_state(&self, private: bool, mode: u16) -> u8 {
        let b = |v: bool| if v { 1 } else { 2 };
        if !private {
            return match mode {
                4 => b(self.modes.insert),
                20 => b(self.modes.newline),
                _ => 0,
            };
        }
        match mode {
            1 => b(self.modes.app_cursor),
            6 => b(self.modes.origin),
            7 => b(self.modes.autowrap),
            25 => b(self.modes.show_cursor),
            47 | 1047 | 1049 => b(self.alt_active),
            1000 => b(self.modes.mouse == MouseMode::Click),
            1002 => b(self.modes.mouse == MouseMode::Drag),
            1003 => b(self.modes.mouse == MouseMode::Motion),
            1004 => b(self.modes.focus_events),
            1006 => b(self.modes.mouse_sgr),
            2004 => b(self.modes.bracketed_paste),
            2026 => b(self.modes.sync),
            _ => 0,
        }
    }

    fn sgr(&mut self, params: &Params) {
        let mut pen = self.cursor.pen;
        let mut it = params.iter().peekable();
        if params.is_empty() {
            pen = Style::default();
        }
        while let Some(p) = it.next() {
            let code = p[0];
            match code {
                0 => pen = Style::default(),
                1 => pen.attrs |= attr::BOLD,
                2 => pen.attrs |= attr::DIM,
                3 => pen.attrs |= attr::ITALIC,
                4 => {
                    if p.get(1).copied().unwrap_or(1) == 0 {
                        pen.attrs &= !attr::UNDERLINE;
                    } else {
                        pen.attrs |= attr::UNDERLINE;
                    }
                }
                5 | 6 => pen.attrs |= attr::BLINK,
                7 => pen.attrs |= attr::INVERSE,
                8 => pen.attrs |= attr::HIDDEN,
                9 => pen.attrs |= attr::STRIKE,
                21 => pen.attrs |= attr::UNDERLINE,
                22 => pen.attrs &= !(attr::BOLD | attr::DIM),
                23 => pen.attrs &= !attr::ITALIC,
                24 => pen.attrs &= !attr::UNDERLINE,
                25 => pen.attrs &= !attr::BLINK,
                27 => pen.attrs &= !attr::INVERSE,
                28 => pen.attrs &= !attr::HIDDEN,
                29 => pen.attrs &= !attr::STRIKE,
                30..=37 => pen.fg = Color::Indexed((code - 30) as u8),
                39 => pen.fg = Color::Default,
                40..=47 => pen.bg = Color::Indexed((code - 40) as u8),
                49 => pen.bg = Color::Default,
                90..=97 => pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => pen.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 | 58 => {
                    // Colon form carries everything in one param; semicolon
                    // form spreads it over the following ones.
                    let rest: Vec<u16> = if p.len() > 1 {
                        p[1..].to_vec()
                    } else {
                        let mut v = Vec::new();
                        if let Some(kind) = it.next() {
                            v.push(kind[0]);
                            let n = if kind[0] == 5 {
                                1
                            } else if kind[0] == 2 {
                                3
                            } else {
                                0
                            };
                            for _ in 0..n {
                                if let Some(x) = it.next() {
                                    v.push(x[0]);
                                }
                            }
                        }
                        v
                    };
                    let color = match rest.as_slice() {
                        [5, i, ..] => Some(Color::Indexed(*i as u8)),
                        // 38:2:<colourspace>:r:g:b
                        [2, _, r, g, b, ..] if p.len() > 5 => Some(Color::Rgb(*r as u8, *g as u8, *b as u8)),
                        [2, r, g, b, ..] => Some(Color::Rgb(*r as u8, *g as u8, *b as u8)),
                        _ => None,
                    };
                    if let Some(color) = color {
                        match code {
                            38 => pen.fg = color,
                            48 => pen.bg = color,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        // SGR never touches the hyperlink; OSC 8 owns it.
        pen.link = self.cursor.pen.link;
        self.cursor.pen = pen;
    }

    fn osc_color_reply(&mut self, which: u8, rgb: (u8, u8, u8), bell: bool) {
        let (r, g, b) = rgb;
        let end = if bell { "\x07" } else { "\x1b\\" };
        let s = format!("\x1b]{which};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}{end}");
        self.reply.extend_from_slice(s.as_bytes());
    }
}

impl Perform for Term {
    fn print(&mut self, c: char) {
        self.print_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.events.push(Event::Bell),
            0x08 => {
                if self.cursor.pending_wrap {
                    self.cursor.pending_wrap = false;
                } else {
                    self.cursor.col = self.cursor.col.saturating_sub(1);
                }
            }
            0x09 => {
                let mut col = self.cursor.col + 1;
                while col < self.cols - 1 && !self.tabs[col] {
                    col += 1;
                }
                self.cursor.col = col.min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            0x0a..=0x0c => {
                self.linefeed();
                if self.modes.newline {
                    self.cursor.col = 0;
                }
            }
            0x0d => {
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            0x0e => self.dec_graphics = false,
            0x0f => self.dec_graphics = false,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let ps: Vec<u16> = params.iter().map(|p| p[0]).collect();
        // Parameter n, with 0 or missing meaning `default`.
        let arg = |i: usize, default: u16| -> usize {
            match ps.get(i) {
                Some(&0) | None => default as usize,
                Some(&v) => v as usize,
            }
        };
        let private = intermediates.first() == Some(&b'?');
        let (row, col) = (self.cursor.row, self.cursor.col);
        match (intermediates, action) {
            ([], '@') => {
                let n = arg(0, 1).min(self.cols - col);
                let fill = self.erase_cell();
                let line = &mut self.grid_mut().lines[row];
                line.cells[col..].rotate_right(n);
                line.cells[col..col + n].fill(fill);
                self.touch(row);
            }
            ([], 'A') => {
                let min = if row >= self.top { self.top } else { 0 };
                self.cursor.row = row.saturating_sub(arg(0, 1)).max(min);
                self.cursor.pending_wrap = false;
            }
            ([], 'B') | ([], 'e') => {
                let max = if row <= self.bottom {
                    self.bottom
                } else {
                    self.rows - 1
                };
                self.cursor.row = (row + arg(0, 1)).min(max);
                self.cursor.pending_wrap = false;
            }
            ([], 'C') | ([], 'a') => {
                self.cursor.col = (col + arg(0, 1)).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            ([], 'D') => {
                self.cursor.col = col.saturating_sub(arg(0, 1));
                self.cursor.pending_wrap = false;
            }
            ([], 'E') => {
                let max = if row <= self.bottom {
                    self.bottom
                } else {
                    self.rows - 1
                };
                self.cursor.row = (row + arg(0, 1)).min(max);
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            ([], 'F') => {
                let min = if row >= self.top { self.top } else { 0 };
                self.cursor.row = row.saturating_sub(arg(0, 1)).max(min);
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            ([], 'G') | ([], '`') => {
                self.cursor.col = (arg(0, 1) - 1).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            ([], 'H') | ([], 'f') => self.goto(arg(0, 1) - 1, arg(1, 1) - 1),
            ([], 'I') => {
                for _ in 0..arg(0, 1) {
                    self.execute(0x09);
                }
            }
            ([], 'J') | ([b'?'], 'J') => {
                let rows = self.rows;
                match ps.first().copied().unwrap_or(0) {
                    0 => {
                        self.erase_cells(row, col, self.cols);
                        for r in row + 1..rows {
                            self.erase_cells(r, 0, self.cols);
                        }
                    }
                    1 => {
                        for r in 0..row {
                            self.erase_cells(r, 0, self.cols);
                        }
                        self.erase_cells(row, 0, col + 1);
                    }
                    2 => {
                        for r in 0..rows {
                            self.erase_cells(r, 0, self.cols);
                        }
                    }
                    3 if !self.alt_active => {
                        self.history.clear();
                        self.all_dirty = true;
                    }
                    _ => {}
                }
            }
            ([], 'K') | ([b'?'], 'K') => match ps.first().copied().unwrap_or(0) {
                0 => self.erase_cells(row, col, self.cols),
                1 => self.erase_cells(row, 0, col + 1),
                2 => self.erase_cells(row, 0, self.cols),
                _ => {}
            },
            ([], 'L') => {
                if (self.top..=self.bottom).contains(&row) {
                    self.scroll_down_in(row, self.bottom, arg(0, 1));
                    self.cursor.col = 0;
                }
            }
            ([], 'M') => {
                if (self.top..=self.bottom).contains(&row) {
                    self.scroll_up_in(row, self.bottom, arg(0, 1), false);
                    self.cursor.col = 0;
                }
            }
            ([], 'P') => {
                let n = arg(0, 1).min(self.cols - col);
                let fill = self.erase_cell();
                let line = &mut self.grid_mut().lines[row];
                line.cells[col..].rotate_left(n);
                let len = line.cells.len();
                line.cells[len - n..].fill(fill);
                self.touch(row);
            }
            ([], 'S') => self.scroll_up_in(self.top, self.bottom, arg(0, 1), false),
            ([], 'T') => self.scroll_down_in(self.top, self.bottom, arg(0, 1)),
            ([], 'X') => self.erase_cells(row, col, col + arg(0, 1)),
            ([], 'Z') => {
                for _ in 0..arg(0, 1) {
                    let mut c = self.cursor.col;
                    while c > 0 {
                        c -= 1;
                        if self.tabs[c] {
                            break;
                        }
                    }
                    self.cursor.col = c;
                }
            }
            ([], 'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..arg(0, 1).min(65535) {
                        self.print_char(c);
                    }
                }
            }
            ([], 'c') => {
                if ps.first().copied().unwrap_or(0) == 0 {
                    self.reply.extend_from_slice(b"\x1b[?62;22c");
                }
            }
            ([b'>'], 'c') => self.reply.extend_from_slice(b"\x1b[>1;10;0c"),
            ([], 'd') => {
                self.cursor.row = (arg(0, 1) - 1).min(self.rows - 1);
                self.cursor.pending_wrap = false;
            }
            ([], 'g') => match ps.first().copied().unwrap_or(0) {
                0 => self.tabs[col] = false,
                3 => self.tabs.fill(false),
                _ => {}
            },
            ([], 'h') | ([b'?'], 'h') => {
                for &m in &ps {
                    self.set_mode(private, m, true);
                }
            }
            ([], 'l') | ([b'?'], 'l') => {
                for &m in &ps {
                    self.set_mode(private, m, false);
                }
            }
            ([], 'm') => self.sgr(params),
            ([], 'n') => match ps.first().copied().unwrap_or(0) {
                5 => self.reply.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let r = if self.modes.origin { row - self.top } else { row };
                    let s = format!("\x1b[{};{}R", r + 1, col + 1);
                    self.reply.extend_from_slice(s.as_bytes());
                }
                _ => {}
            },
            ([], 'r') => {
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows as u16) - 1;
                if top < bottom && bottom < self.rows {
                    self.top = top;
                    self.bottom = bottom;
                    self.goto(0, 0);
                }
            }
            ([], 's') => self.save_cursor(),
            ([], 'u') => self.restore_cursor(),
            ([b' '], 'q') => {
                self.cursor_shape = match ps.first().copied().unwrap_or(0) {
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => CursorShape::Block,
                };
                self.touch(row);
            }
            ([b'!'], 'p') => {
                self.modes = Modes::default();
                self.cursor.pen = Style::default();
                self.top = 0;
                self.bottom = self.rows - 1;
            }
            ([b'$'], 'p') | ([b'?', b'$'], 'p') => {
                let m = ps.first().copied().unwrap_or(0);
                let state = self.mode_state(private, m);
                let q = if private { "?" } else { "" };
                let s = format!("\x1b[{q}{m};{state}$y");
                self.reply.extend_from_slice(s.as_bytes());
            }
            ([], 't') if ps.first() == Some(&18) => {
                let s = format!("\x1b[8;{};{}t", self.rows, self.cols);
                self.reply.extend_from_slice(s.as_bytes());
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([b'#'], b'8') => {
                let cols = self.cols;
                for line in &mut self.grid_mut().lines {
                    line.reset(
                        cols,
                        Cell {
                            ch: 'E' as u32,
                            style: 0,
                            flags: 0,
                        },
                    );
                }
                self.all_dirty = true;
                self.dirty.fill(true);
            }
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.linefeed();
                self.cursor.col = 0;
            }
            ([], b'H') => {
                let col = self.cursor.col;
                self.tabs[col] = true;
            }
            ([], b'M') => self.reverse_index(),
            ([], b'c') => self.reset(),
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([b'('], b'0') => self.dec_graphics = true,
            ([b'('], _) => self.dec_graphics = false,
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell: bool) {
        let Some(&first) = params.first() else { return };
        let text = |i: usize| {
            params
                .get(i)
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .unwrap_or_default()
        };
        match first {
            b"0" | b"2" => {
                // Titles may contain ';', which vte splits on.
                let title = params[1..]
                    .iter()
                    .map(|p| String::from_utf8_lossy(p))
                    .collect::<Vec<_>>()
                    .join(";");
                if title != self.title {
                    self.title = title.clone();
                    self.events.push(Event::Title(title));
                }
            }
            b"7" => {
                let url = text(1);
                let path = url
                    .strip_prefix("file://")
                    .map(|rest| rest.find('/').map_or("", |i| &rest[i..]));
                if let Some(path) = path {
                    self.events.push(Event::Cwd(percent_decode(path)));
                }
            }
            // OSC 9;4 is a progress report, not a notification.
            b"9" if params.get(1).is_some_and(|p| *p != b"4") => self.events.push(Event::Notify(text(1))),
            b"777" if params.get(1) == Some(&&b"notify"[..]) => {
                let msg = [text(2), text(3)]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(": ");
                self.events.push(Event::Notify(msg));
            }
            b"10" if params.get(1) == Some(&&b"?"[..]) => self.osc_color_reply(10, self.report_fg, bell),
            b"11" if params.get(1) == Some(&&b"?"[..]) => self.osc_color_reply(11, self.report_bg, bell),
            b"8" => {
                // The URI may contain ';', which vte splits on.
                let uri = params.get(2..).map(|p| {
                    p.iter()
                        .map(|p| String::from_utf8_lossy(p))
                        .collect::<Vec<_>>()
                        .join(";")
                });
                self.cursor.pen.link = uri.filter(|u| !u.is_empty()).map_or(0, |u| self.intern_link(&u));
            }
            b"52" => {
                if let Some(data) = params.get(2).filter(|d| **d != b"?")
                    && let Some(bytes) = base64_decode(data)
                {
                    self.events
                        .push(Event::Clipboard(String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
            _ => {}
        }
    }
}

const MAX_LINKS: usize = 4096;
const MAX_LINK_LEN: usize = 2048;

fn is_emoji_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0 && c > 0).collect()
}

fn fit_width(line: &mut Line, cols: usize) {
    if line.cells.len() > cols {
        line.cells.truncate(cols);
        if let Some(last) = line.cells.last_mut()
            && last.flags & flag::WIDE != 0
        {
            *last = Cell::BLANK;
        }
        line.wrapped = false;
    } else {
        line.cells.resize(cols, Cell::BLANK);
    }
}

/// DEC special graphics (line drawing), used after `ESC ( 0`.
fn dec_special(c: char) -> char {
    match c {
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'q' => '─',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'a' => '▒',
        '`' => '◆',
        'f' => '°',
        'g' => '±',
        '~' => '·',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        _ => c,
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64_decode(data: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in data {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Term, bytes: &[u8]) {
        let mut p = vte::Parser::new();
        p.advance(t, bytes);
    }

    fn screen(t: &Term) -> Vec<String> {
        t.screen_text().lines().map(str::to_string).collect()
    }

    #[test]
    fn osc8_links_style_cells() {
        let mut t = Term::new(20, 2);
        run(
            &mut t,
            b"a\x1b]8;id=1;https://x.io/a;b\x1b\\bc\x1b[1m\x1b[0md\x1b]8;;\x1b\\e",
        );
        let id = |c: usize| t.styles.get(t.grid().lines[0].cells[c].style).link;
        assert_eq!(id(0), 0);
        assert_ne!(id(1), 0);
        assert_eq!(id(1), id(2));
        // SGR 0 keeps the link, bold or not.
        assert_eq!(id(3), id(1));
        assert_eq!(id(4), 0);
        assert_eq!(t.link(id(1)), Some("https://x.io/a;b"));
        assert_eq!(t.link(0), None);
    }

    #[test]
    fn osc8_table_is_bounded_and_deduped() {
        let mut t = Term::new(20, 2);
        run(&mut t, b"\x1b]8;;u\x1b\\x\x1b]8;;u\x1b\\y");
        assert_eq!(t.links.len(), 1);
        for i in 0..MAX_LINKS + 10 {
            run(&mut t, format!("\x1b]8;;http://h/{i}\x1b\\").as_bytes());
        }
        assert_eq!(t.links.len(), MAX_LINKS);
        assert_eq!(t.cursor.pen.link, 0);
    }

    #[test]
    fn prints_and_wraps() {
        let mut t = Term::new(5, 3);
        run(&mut t, b"hello world");
        assert_eq!(screen(&t), ["hello", " worl", "d"]);
        assert!(t.grid().lines[0].wrapped);
    }

    #[test]
    fn scrolls_into_history_trimmed() {
        let mut t = Term::new(10, 2);
        run(&mut t, b"a\r\nb\r\nc\r\nd");
        assert_eq!(screen(&t), ["c", "d"]);
        assert_eq!(t.history.len(), 2);
        assert_eq!(t.history.lines[0].cells.len(), 1, "trailing blanks trimmed");
    }

    #[test]
    fn history_is_capped() {
        let mut t = Term::new(10, 2);
        t.history.max_lines = 3;
        for i in 0..20 {
            run(&mut t, format!("{i}\r\n").as_bytes());
        }
        assert_eq!(t.history.len(), 3);
        assert_eq!(t.history.evicted, 16);
    }

    #[test]
    fn shrinking_the_history_cap_evicts_oldest() {
        let mut t = Term::new(10, 2);
        for i in 0..20 {
            run(&mut t, format!("{i}\r\n").as_bytes());
        }
        let before = t.history.len();
        t.history.set_limits(5, 1 << 20);
        assert_eq!(t.history.len(), 5);
        assert_eq!(t.history.evicted, (before - 5) as u64);
    }

    #[test]
    fn cursor_moves_and_erase() {
        let mut t = Term::new(10, 3);
        run(&mut t, b"abcdef\x1b[1;3H\x1b[K");
        assert_eq!(screen(&t)[0], "ab");
        run(&mut t, b"\x1b[2J\x1b[2;4Hx");
        assert_eq!(screen(&t), ["", "   x", ""]);
    }

    #[test]
    fn wide_chars_and_clusters() {
        let mut t = Term::new(6, 2);
        run(&mut t, "日本x".as_bytes());
        let l = &t.grid().lines[0];
        assert_eq!(l.cells[0].flags, flag::WIDE);
        assert_eq!(l.cells[1].flags, flag::SPACER);
        assert_eq!(screen(&t)[0], "日本x");
        // Overwriting half a wide char clears the other half.
        run(&mut t, b"\x1b[1;2Hy");
        assert_eq!(screen(&t)[0], " y本x");
        run(&mut t, "\r\ne\u{301}👍🏽x".as_bytes());
        assert_eq!(screen(&t)[1], "e\u{301}👍🏽x");
        assert_eq!(t.cursor_pos(), (1, 4), "skin tone joins, no extra cells");
        let family = "👨\u{200D}👩\u{200D}👧";
        run(&mut t, format!("\r\n{family}y").as_bytes());
        assert_eq!(t.cursor_pos(), (1, 3), "ZWJ sequence is one wide cell");
    }

    #[test]
    fn sgr_styles_are_interned() {
        let mut t = Term::new(20, 1);
        run(
            &mut t,
            b"\x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[38:5:200mx\x1b[1;31my",
        );
        let l = &t.grid().lines[0];
        let red = t.styles.get(l.cells[0].style);
        assert_eq!(red.fg, Color::Indexed(1));
        assert_eq!(red.attrs, attr::BOLD);
        assert_eq!(t.styles.get(l.cells[4].style).fg, Color::Rgb(1, 2, 3));
        assert_eq!(t.styles.get(l.cells[7].style).fg, Color::Indexed(200));
        assert_eq!(l.cells[0].style, l.cells[8].style);
    }

    #[test]
    fn alt_screen_keeps_main() {
        let mut t = Term::new(10, 2);
        run(&mut t, b"main\x1b[?1049hALT");
        assert_eq!(screen(&t)[0], "    ALT", "cursor is not homed");
        run(&mut t, b"\x1b[?1049l");
        assert_eq!(screen(&t)[0], "main");
        assert_eq!(t.cursor_pos(), (0, 4));
    }

    #[test]
    fn scroll_region_and_lines() {
        let mut t = Term::new(5, 4);
        run(&mut t, b"1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[2;1H\x1b[M");
        assert_eq!(screen(&t), ["1", "3", "", "4"]);
        assert_eq!(t.history.len(), 0, "region scroll never feeds history");
        run(&mut t, b"\x1b[2;1H\x1b[L");
        assert_eq!(screen(&t), ["1", "", "3", "4"]);
    }

    #[test]
    fn replies() {
        let mut t = Term::new(10, 5);
        run(&mut t, b"\x1b[3;4H\x1b[6n\x1b[?2026$p\x1b]11;?\x07");
        let r = String::from_utf8(t.reply.clone()).unwrap();
        assert_eq!(r, "\x1b[3;4R\x1b[?2026;2$y\x1b]11;rgb:0d0d/1111/1717\x07");
    }

    #[test]
    fn osc_title_cwd_notify() {
        let mut t = Term::new(10, 2);
        run(
            &mut t,
            b"\x1b]0;a;b\x07\x1b]7;file://mac/Users/x/My%20Dir\x1b\\\x1b]9;done\x07\x1b]9;4;1;50\x07",
        );
        assert_eq!(
            t.events,
            [
                Event::Title("a;b".into()),
                Event::Cwd("/Users/x/My Dir".into()),
                Event::Notify("done".into())
            ]
        );
    }

    #[test]
    fn resize_moves_rows_through_history() {
        let mut t = Term::new(10, 4);
        run(&mut t, b"1\r\n2\r\n3\r\n4");
        t.resize(10, 2);
        assert_eq!(screen(&t), ["3", "4"]);
        assert_eq!(t.history.len(), 2);
        t.resize(10, 4);
        assert_eq!(screen(&t), ["1", "2", "3", "4"]);
        assert_eq!(t.cursor_pos(), (3, 1));
    }

    #[test]
    fn eight_bytes_per_cell() {
        let mut t = Term::new(200, 50);
        for i in 0..20_000 {
            run(&mut t, format!("line {i} {}\r\n", "x".repeat(80)).as_bytes());
        }
        assert_eq!(t.history.len(), DEFAULT_SCROLLBACK);
        // ~90 visible chars per line × 8 bytes, plus overhead.
        assert!(t.mem_bytes() < 9 << 20, "{} bytes", t.mem_bytes());
    }

    fn all_text(t: &Term) -> Vec<String> {
        (0..t.total_lines())
            .map(|i| {
                let mut s = String::new();
                line_text(t.line(i), &t.clusters, &mut s);
                s.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn reflow_narrow_splits_and_wide_rejoins() {
        let mut t = Term::new(20, 5);
        run(&mut t, b"0123456789abcdefghij0123456789\r\nok");
        assert_eq!(screen(&t)[..3], ["0123456789abcdefghij", "0123456789", "ok"]);
        t.resize(10, 5);
        assert_eq!(screen(&t)[..4], ["0123456789", "abcdefghij", "0123456789", "ok"]);
        assert!(t.grid().lines[0].wrapped && t.grid().lines[1].wrapped && !t.grid().lines[2].wrapped);
        t.resize(30, 5);
        assert_eq!(screen(&t)[..2], ["0123456789abcdefghij0123456789", "ok"]);
        assert!(t.reflowed);
    }

    #[test]
    fn reflow_keeps_cursor_on_character() {
        let mut t = Term::new(20, 5);
        run(&mut t, b"$ echo hello world foo");
        assert_eq!(t.cursor_pos(), (1, 2));
        t.resize(8, 5);
        let (r, c) = t.cursor_pos();
        assert_eq!(t.line(t.total_lines() - t.rows + r).cells[c - 1].ch, 'o' as u32);
        t.resize(40, 5);
        assert_eq!(t.cursor_pos(), (0, 22));
        // Cursor past the text keeps its offset.
        let mut t = Term::new(20, 5);
        run(&mut t, b"$ \x1b[3C");
        assert_eq!(t.cursor_pos(), (0, 5));
        t.resize(4, 5);
        assert_eq!(t.cursor_pos(), (1, 1));
        t.resize(20, 5);
        assert_eq!(t.cursor_pos(), (0, 5));
    }

    #[test]
    fn reflow_wide_char_not_split() {
        let mut t = Term::new(5, 4);
        run(&mut t, "abcd中文x".as_bytes());
        assert_eq!(screen(&t)[..2], ["abcd", "中文x"]);
        t.resize(3, 4);
        for i in 0..t.total_lines() {
            let l = t.line(i);
            assert!(
                l.cells.last().is_none_or(|c| c.flags & flag::WIDE == 0),
                "line {i}"
            );
        }
        assert_eq!(all_text(&t).concat(), "abcd中文x");
        t.resize(10, 4);
        assert_eq!(screen(&t)[0], "abcd中文x");
        assert_eq!(t.total_lines(), 4);
    }

    #[test]
    fn reflow_across_scrollback() {
        let mut t = Term::new(10, 3);
        run(&mut t, b"aaaaaaaaaabbbbbbbbbbcc\r\n1\r\n2\r\n3\r\n4");
        assert!(t.history.len() >= 2);
        t.resize(5, 3);
        let before = all_text(&t);
        assert_eq!(before[..5], ["aaaaa", "aaaaa", "bbbbb", "bbbbb", "cc"]);
        t.resize(30, 3);
        let after = all_text(&t);
        assert_eq!(after[0], "aaaaaaaaaabbbbbbbbbbcc");
        assert_eq!(after[1..], ["1", "2", "3", "4"]);
        assert_eq!(t.cursor_pos(), (2, 1));
    }

    #[test]
    fn reflow_respects_history_caps() {
        let mut t = Term::new(40, 4);
        t.history.max_lines = 50;
        for i in 0..100 {
            run(&mut t, format!("{i} {}\r\n", "y".repeat(35)).as_bytes());
        }
        t.resize(10, 4);
        assert!(t.history.len() <= 50);
        assert_eq!(t.main.lines.len(), 4);
        assert!(t.main.lines.iter().all(|l| l.cells.len() == 10));
        let bytes: usize = t.history.lines.iter().map(Line::bytes).sum();
        assert!(t.history.bytes() >= bytes);
        assert!(
            t.history
                .lines
                .iter()
                .all(|l| l.cells.last().is_none_or(|c| !c.is_blank()))
        );
    }

    #[test]
    fn alt_screen_is_not_reflowed() {
        let mut t = Term::new(10, 3);
        run(&mut t, b"\x1b[?1049h0123456789ab");
        assert!(t.alt_active);
        t.resize(5, 3);
        assert_eq!(screen(&t)[0], "01234");
        assert_eq!(screen(&t)[1], "ab");
        run(&mut t, b"\x1b[?1049l");
        assert!(!t.alt_active);
    }

    #[test]
    #[ignore]
    fn reflow_ten_thousand_lines_timing() {
        let mut t = Term::new(120, 40);
        t.history.max_lines = 20_000;
        t.history.max_bytes = 1 << 30;
        for i in 0..10_000 {
            run(&mut t, format!("{i} {}\r\n", "z".repeat(150)).as_bytes());
        }
        let n = t.history.len();
        let t0 = std::time::Instant::now();
        t.resize(80, 40);
        let a = t0.elapsed();
        t.resize(120, 40);
        let b = t0.elapsed() - a;
        eprintln!("history {n} -> narrow {a:?}, wide {b:?}");
        assert!(a.as_millis() < 500 && b.as_millis() < 500);
    }
}
