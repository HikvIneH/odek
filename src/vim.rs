//! Vim keys: a small modal engine over an abstract text buffer.
//!
//! Positions are UTF-16 code-unit offsets (what NSTextView uses). The engine
//! knows nothing about AppKit; `app.rs` adapts NSTextView to `Buffer`, and the
//! tests use an in-memory buffer.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Esc,
    Enter,
    Backspace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual { line: bool },
    Command(String),
    Search { forward: bool, text: String },
}

/// What the editor around the engine should do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Not a Vim key in this mode: let the text view handle it.
    PassThrough,
    Save,
    Close {
        force: bool,
    },
    SaveClose,
    NextTab,
    PrevTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    Center,
    Top,
    Bottom,
}

pub trait Buffer {
    fn len(&self) -> usize;
    fn at(&self, i: usize) -> u16;
    fn slice(&self, start: usize, end: usize) -> String;
    /// Start of the current selection (the caret when nothing is selected).
    fn cursor(&self) -> usize;
    fn set_cursor(&mut self, i: usize);
    /// Show `[start, end)` selected.
    fn set_selection(&mut self, start: usize, end: usize);
    fn replace(&mut self, start: usize, end: usize, text: &str);
    fn undo(&mut self);
    fn redo(&mut self);
    /// Lines visible on screen, for Ctrl-d/u/f/b.
    fn page_lines(&self) -> usize;
    fn scroll(&mut self, _pos: usize, _how: Scroll) {}
    /// Next match of `needle` strictly after (or before) `from`, wrapping.
    fn find(&self, needle: &str, from: usize, forward: bool) -> Option<usize> {
        let hay: Vec<u16> = (0..self.len()).map(|i| self.at(i)).collect();
        let pat: Vec<u16> = needle.encode_utf16().collect();
        if pat.is_empty() || pat.len() > hay.len() {
            return None;
        }
        let hits: Vec<usize> = (0..=hay.len() - pat.len())
            .filter(|&i| hay[i..i + pat.len()] == pat[..])
            .collect();
        if forward {
            hits.iter().find(|&&i| i > from).or(hits.first()).copied()
        } else {
            hits.iter().rev().find(|&&i| i < from).or(hits.last()).copied()
        }
    }
    /// Offset where 0-based line `n` starts (clamped to the last line).
    fn line_offset(&self, n: usize) -> usize {
        let mut line = 0;
        let mut start = 0;
        for i in 0..self.len() {
            if line == n {
                break;
            }
            if self.at(i) == NL {
                line += 1;
                start = i + 1;
            }
        }
        start
    }
    fn clipboard(&self) -> Option<String> {
        None
    }
    fn set_clipboard(&mut self, _text: &str) {}
    /// One level of indentation for `>>` / `<<`.
    fn indent_unit(&self) -> String {
        "    ".into()
    }
}

const NL: u16 = b'\n' as u16;

fn is_space(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16 || c == NL || c == b'\r' as u16
}

/// 0 = whitespace, 1 = word (alphanumeric or _), 2 = punctuation.
fn class(c: u16) -> u8 {
    if is_space(c) {
        0
    } else if char::from_u32(c as u32).is_some_and(|ch| ch.is_alphanumeric() || ch == '_') || c > 0x7F {
        1
    } else {
        2
    }
}

fn line_start(b: &dyn Buffer, i: usize) -> usize {
    let mut j = i.min(b.len());
    while j > 0 && b.at(j - 1) != NL {
        j -= 1;
    }
    j
}

/// Offset of the line's `\n` (or the buffer end).
fn line_end(b: &dyn Buffer, i: usize) -> usize {
    let mut j = i.min(b.len());
    while j < b.len() && b.at(j) != NL {
        j += 1;
    }
    j
}

fn first_nonblank(b: &dyn Buffer, i: usize) -> usize {
    let (mut j, end) = (line_start(b, i), line_end(b, i));
    while j < end && is_space(b.at(j)) {
        j += 1;
    }
    j
}

/// Normal mode never rests on a line's `\n` unless the line is empty.
fn clamp_normal(b: &dyn Buffer, i: usize) -> usize {
    let i = i.min(b.len());
    let (ls, le) = (line_start(b, i), line_end(b, i));
    if i >= le && le > ls { le - 1 } else { i }
}

fn line_index(b: &dyn Buffer, i: usize) -> usize {
    (0..i.min(b.len())).filter(|&k| b.at(k) == NL).count()
}

fn word_fwd(b: &dyn Buffer, i: usize) -> usize {
    let n = b.len();
    if i >= n {
        return n;
    }
    let mut j = i;
    let c = class(b.at(j));
    if c != 0 {
        while j < n && class(b.at(j)) == c {
            j += 1;
        }
    }
    while j < n && class(b.at(j)) == 0 {
        // An empty line is a word of its own.
        if b.at(j) == NL && j + 1 < n && b.at(j + 1) == NL {
            return j + 1;
        }
        j += 1;
    }
    j
}

fn word_end(b: &dyn Buffer, i: usize) -> usize {
    let n = b.len();
    let mut j = i + 1;
    while j < n && class(b.at(j)) == 0 {
        j += 1;
    }
    if j >= n {
        return n.saturating_sub(1);
    }
    let c = class(b.at(j));
    while j + 1 < n && class(b.at(j + 1)) == c {
        j += 1;
    }
    j
}

fn word_back(b: &dyn Buffer, i: usize) -> usize {
    if i == 0 {
        return 0;
    }
    let mut j = i - 1;
    while j > 0 && class(b.at(j)) == 0 {
        j -= 1;
    }
    let c = class(b.at(j));
    while j > 0 && class(b.at(j - 1)) == c {
        j -= 1;
    }
    j
}

fn paragraph(b: &dyn Buffer, i: usize, forward: bool) -> usize {
    let n = b.len();
    let blank = |k: usize| line_start(b, k) == line_end(b, k);
    let on_blank = blank(i);
    let mut j = if forward { line_end(b, i) } else { line_start(b, i) };
    if forward {
        // From a blank line, skip the run of blanks first.
        while on_blank && j < n && blank(j + 1) {
            j = line_end(b, j + 1);
        }
        while j < n {
            j += 1;
            if blank(j) {
                return j;
            }
            j = line_end(b, j);
        }
        n
    } else {
        while on_blank && j > 0 && blank(j - 1) {
            j = line_start(b, j - 1);
        }
        while j > 0 {
            j = line_start(b, j - 1);
            if blank(j) {
                return j;
            }
        }
        0
    }
}

fn matching_bracket(b: &dyn Buffer, i: usize) -> Option<usize> {
    let pairs = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];
    let le = line_end(b, i);
    let start = (i..le).find(|&k| {
        pairs
            .iter()
            .any(|&(o, c)| b.at(k) == o as u16 || b.at(k) == c as u16)
    })?;
    let ch = b.at(start);
    for (o, c) in pairs {
        let (o, c) = (o as u16, c as u16);
        let mut depth = 0i32;
        if ch == o {
            for k in start..b.len() {
                depth += (b.at(k) == o) as i32 - (b.at(k) == c) as i32;
                if depth == 0 {
                    return Some(k);
                }
            }
        } else if ch == c {
            for k in (0..=start).rev() {
                depth += (b.at(k) == c) as i32 - (b.at(k) == o) as i32;
                if depth == 0 {
                    return Some(k);
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
struct Motion {
    to: usize,
    linewise: bool,
    inclusive: bool,
    keep_goal: bool,
}

impl Motion {
    fn excl(to: usize) -> Motion {
        Motion {
            to,
            linewise: false,
            inclusive: false,
            keep_goal: false,
        }
    }
    fn incl(to: usize) -> Motion {
        Motion {
            to,
            linewise: false,
            inclusive: true,
            keep_goal: false,
        }
    }
    fn lines(to: usize) -> Motion {
        Motion {
            to,
            linewise: true,
            inclusive: false,
            keep_goal: false,
        }
    }
}

#[derive(Default, Clone)]
struct Register {
    text: String,
    linewise: bool,
}

pub struct Vim {
    pub mode: Mode,
    /// Shown in the status bar until the next key (errors, "N lines yanked").
    pub message: Option<String>,
    count: Option<usize>,
    /// Pending operator and the count typed before it.
    op: Option<(char, usize)>,
    /// Waiting for a second key: g z f t F T r.
    prefix: Option<char>,
    anchor: usize,
    cur: usize,
    goal: Option<usize>,
    reg: Register,
    last_find: Option<(char, char)>,
    last_search: Option<(String, bool)>,
}

impl Default for Vim {
    fn default() -> Self {
        Vim::new()
    }
}

impl Vim {
    pub fn new() -> Vim {
        Vim {
            mode: Mode::Normal,
            message: None,
            count: None,
            op: None,
            prefix: None,
            anchor: 0,
            cur: 0,
            goal: None,
            reg: Register::default(),
            last_find: None,
            last_search: None,
        }
    }

    /// Back to normal mode with nothing pending (new tab, Vim switched on).
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.clear_pending();
        self.goal = None;
    }

    fn clear_pending(&mut self) {
        self.count = None;
        self.op = None;
        self.prefix = None;
    }

    /// Block cursor in normal mode, thin caret otherwise.
    pub fn block_cursor(&self) -> bool {
        matches!(self.mode, Mode::Normal)
    }

    /// Status-bar text.
    pub fn status(&self) -> String {
        if let Some(m) = &self.message {
            return m.clone();
        }
        match &self.mode {
            Mode::Insert => "-- INSERT --".into(),
            Mode::Visual { line: false } => "-- VISUAL --".into(),
            Mode::Visual { line: true } => "-- VISUAL LINE --".into(),
            Mode::Command(s) => format!(":{s}"),
            Mode::Search { forward, text } => format!("{}{text}", if *forward { '/' } else { '?' }),
            Mode::Normal => {
                let mut s = String::from("NORMAL");
                let pending: String = [
                    self.count.map(|c| c.to_string()),
                    self.op
                        .map(|(o, c)| if c > 1 { format!("{c}{o}") } else { o.to_string() }),
                    self.prefix.map(String::from),
                ]
                .into_iter()
                .flatten()
                .collect();
                if !pending.is_empty() {
                    s.push_str("  ");
                    s.push_str(&pending);
                }
                s
            }
        }
    }

    pub fn handle(&mut self, key: Key, b: &mut dyn Buffer) -> Effect {
        self.message = None;
        match self.mode.clone() {
            Mode::Insert => {
                if matches!(key, Key::Esc | Key::Ctrl('[') | Key::Ctrl('c')) {
                    self.mode = Mode::Normal;
                    let c = b.cursor();
                    b.set_cursor(if c > line_start(b, c) { c - 1 } else { c });
                    Effect::None
                } else {
                    Effect::PassThrough
                }
            }
            Mode::Command(text) => self.command_line(key, text, b),
            Mode::Search { forward, text } => self.search_line(key, forward, text, b),
            Mode::Normal | Mode::Visual { .. } => self.normal(key, b),
        }
    }

    fn visual(&self) -> Option<bool> {
        match self.mode {
            Mode::Visual { line } => Some(line),
            _ => None,
        }
    }

    fn pos(&self, b: &dyn Buffer) -> usize {
        if self.visual().is_some() {
            self.cur
        } else {
            b.cursor()
        }
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }

    fn show_visual(&self, b: &mut dyn Buffer) {
        let (s, e) = (self.anchor.min(self.cur), self.anchor.max(self.cur));
        if self.visual() == Some(true) {
            let le = line_end(b, e);
            b.set_selection(line_start(b, s), (le + 1).min(b.len()));
        } else {
            b.set_selection(s, (e + 1).min(b.len()).max(s));
        }
    }

    fn enter_insert(&mut self, b: &mut dyn Buffer, at: usize) {
        b.set_cursor(at);
        self.mode = Mode::Insert;
    }

    fn normal(&mut self, key: Key, b: &mut dyn Buffer) -> Effect {
        if key == Key::Esc || key == Key::Ctrl('[') || key == Key::Ctrl('c') {
            if self.visual().is_some() {
                let c = clamp_normal(b, self.cur);
                self.mode = Mode::Normal;
                b.set_cursor(c);
            }
            self.clear_pending();
            return Effect::None;
        }
        let pos = self.pos(b);

        if let Some(p) = self.prefix.take() {
            let Key::Char(ch) = key else {
                self.clear_pending();
                return Effect::None;
            };
            match p {
                'f' | 't' | 'F' | 'T' => {
                    self.last_find = Some((p, ch));
                    let n = self.take_count();
                    match self.find_char(b, pos, p, ch, n) {
                        Some(m) => return self.finish_motion(m, b),
                        None => {
                            self.clear_pending();
                            return Effect::None;
                        }
                    }
                }
                'r' => {
                    let n = self.take_count();
                    let le = line_end(b, pos);
                    if pos + n <= le {
                        b.replace(pos, pos + n, &ch.to_string().repeat(n));
                        b.set_cursor(pos + n - 1);
                    }
                    return Effect::None;
                }
                'g' => match ch {
                    'g' => {
                        let line = self.count.take().unwrap_or(1).max(1) - 1;
                        let to = first_nonblank(b, b.line_offset(line));
                        return self.finish_motion(Motion::lines(to), b);
                    }
                    't' => {
                        self.clear_pending();
                        return Effect::NextTab;
                    }
                    'T' => {
                        self.clear_pending();
                        return Effect::PrevTab;
                    }
                    _ => {
                        self.clear_pending();
                        return Effect::None;
                    }
                },
                'z' => {
                    let how = match ch {
                        'z' => Scroll::Center,
                        't' => Scroll::Top,
                        'b' => Scroll::Bottom,
                        _ => return Effect::None,
                    };
                    b.scroll(pos, how);
                    return Effect::None;
                }
                _ => return Effect::None,
            }
        }

        let ch = match key {
            Key::Char(c) => c,
            Key::Ctrl(c) => return self.ctrl(c, pos, b),
            Key::Enter => 'j',
            Key::Backspace => 'h',
            Key::Esc => return Effect::None,
        };

        if ch.is_ascii_digit() && (ch != '0' || self.count.is_some()) {
            let d = ch as usize - '0' as usize;
            self.count = Some(
                self.count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(d)
                    .min(100_000),
            );
            return Effect::None;
        }

        if let Some(m) = self.motion(ch, pos, b) {
            return self.finish_motion(m, b);
        }

        match ch {
            'f' | 't' | 'F' | 'T' | 'r' | 'g' | 'z' => {
                self.prefix = Some(ch);
                Effect::None
            }
            'd' | 'c' | 'y' | '>' | '<' => self.operator(ch, b),
            _ if self.op.is_some() => {
                self.clear_pending();
                Effect::None
            }
            _ => self.command(ch, pos, b),
        }
    }

    fn ctrl(&mut self, c: char, pos: usize, b: &mut dyn Buffer) -> Effect {
        let page = b.page_lines().max(2);
        let lines = match c {
            'd' | 'u' => page / 2,
            'f' | 'b' => page.saturating_sub(2).max(1),
            'r' => {
                for _ in 0..self.take_count() {
                    b.redo();
                }
                let c = clamp_normal(b, b.cursor());
                b.set_cursor(c);
                return Effect::None;
            }
            _ => {
                self.clear_pending();
                return Effect::None;
            }
        };
        let down = matches!(c, 'd' | 'f');
        let to = self.vertical(b, pos, lines, down);
        self.clear_pending();
        let m = Motion {
            to,
            linewise: true,
            inclusive: false,
            keep_goal: true,
        };
        self.finish_motion(m, b)
    }

    /// Same column on a line `n` lines away.
    fn vertical(&mut self, b: &dyn Buffer, pos: usize, n: usize, down: bool) -> usize {
        let ls = line_start(b, pos);
        let goal = *self.goal.get_or_insert(pos - ls);
        let mut start = ls;
        for _ in 0..n {
            if down {
                let le = line_end(b, start);
                if le >= b.len() {
                    break;
                }
                start = le + 1;
            } else {
                if start == 0 {
                    break;
                }
                start = line_start(b, start - 1);
            }
        }
        let le = line_end(b, start);
        (start + goal).min(if le > start { le - 1 } else { start })
    }

    fn find_char(&self, b: &dyn Buffer, pos: usize, kind: char, ch: char, n: usize) -> Option<Motion> {
        let target = ch as u32;
        let (ls, le) = (line_start(b, pos), line_end(b, pos));
        let mut p = pos;
        for _ in 0..n {
            let mut found = None;
            if matches!(kind, 'f' | 't') {
                for k in (p + 1).min(le)..le {
                    if b.at(k) as u32 == target {
                        found = Some(k);
                        break;
                    }
                }
            } else {
                for k in (ls..p).rev() {
                    if b.at(k) as u32 == target {
                        found = Some(k);
                        break;
                    }
                }
            }
            p = found?;
        }
        Some(match kind {
            'f' => Motion::incl(p),
            't' => Motion::incl(p - 1),
            'F' => Motion::excl(p),
            _ => Motion::excl(p + 1),
        })
    }

    fn motion(&mut self, ch: char, pos: usize, b: &mut dyn Buffer) -> Option<Motion> {
        let n_raw = self.count;
        // 2dw and d2w both mean "two words".
        let n = n_raw.unwrap_or(1).max(1) * self.op.map_or(1, |o| o.1);
        let (ls, le) = (line_start(b, pos), line_end(b, pos));
        let for_op = self.op.is_some();
        let op = self.op.map(|o| o.0);
        let m = match ch {
            'h' => Motion::excl(pos.saturating_sub(n).max(ls)),
            'l' | ' ' => {
                let last = if for_op {
                    le
                } else if le > ls {
                    le - 1
                } else {
                    ls
                };
                Motion::excl((pos + n).min(last))
            }
            'j' | 'k' => {
                let to = self.vertical(b, pos, n, ch == 'j');
                Motion {
                    to,
                    linewise: true,
                    inclusive: false,
                    keep_goal: true,
                }
            }
            'w' | 'W' => {
                // cw on a word behaves like ce (Vim's long-standing quirk).
                if op == Some('c') && class(b.at(pos.min(b.len().saturating_sub(1)))) != 0 && pos < b.len() {
                    let mut to = pos;
                    for k in 0..n {
                        to = if k == 0 && pos + 1 < b.len() && class(b.at(pos + 1)) != class(b.at(pos)) {
                            pos
                        } else {
                            word_end(b, if k == 0 { pos.saturating_sub(0) } else { to })
                        };
                        if k == 0 && to < pos {
                            to = pos;
                        }
                    }
                    Motion::incl(to.min(le.saturating_sub(1)).max(pos))
                } else {
                    let mut to = pos;
                    for _ in 0..n {
                        to = word_fwd(b, to);
                    }
                    // An operator never eats the line break after the last word.
                    if for_op && to > le {
                        to = le;
                    }
                    Motion::excl(to)
                }
            }
            'e' | 'E' => {
                let mut to = pos;
                for _ in 0..n {
                    to = word_end(b, to);
                }
                Motion::incl(to)
            }
            'b' | 'B' => {
                let mut to = pos;
                for _ in 0..n {
                    to = word_back(b, to);
                }
                Motion::excl(to)
            }
            '0' => Motion::excl(ls),
            '^' => Motion::excl(first_nonblank(b, pos)),
            '$' => {
                let mut end = le;
                for _ in 1..n {
                    if end >= b.len() {
                        break;
                    }
                    end = line_end(b, end + 1);
                }
                let line_s = line_start(b, end);
                if for_op {
                    Motion::excl(end)
                } else {
                    Motion::incl(if end > line_s { end - 1 } else { end })
                }
            }
            'G' => {
                let to = match n_raw {
                    Some(line) => b.line_offset(line.max(1) - 1),
                    None => line_start(b, b.len()),
                };
                Motion::lines(first_nonblank(b, to))
            }
            '{' | '}' => {
                let mut to = pos;
                for _ in 0..n {
                    to = paragraph(b, to, ch == '}');
                }
                Motion::excl(to)
            }
            '%' => Motion::incl(matching_bracket(b, pos)?),
            ';' | ',' => {
                let (kind, c) = self.last_find?;
                let kind = if ch == ',' {
                    match kind {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    }
                } else {
                    kind
                };
                self.find_char(b, pos, kind, c, n)?
            }
            'n' | 'N' => {
                let (needle, fwd) = self.last_search.clone()?;
                let fwd = if ch == 'N' { !fwd } else { fwd };
                let mut to = pos;
                for _ in 0..n {
                    to = match b.find(&needle, to, fwd) {
                        Some(t) => t,
                        None => {
                            self.message = Some(format!("Pattern not found: {needle}"));
                            return None;
                        }
                    };
                }
                Motion::excl(to)
            }
            '*' | '#' => {
                let (mut s, mut e) = (pos, pos);
                while s > 0 && class(b.at(s - 1)) == 1 {
                    s -= 1;
                }
                while e < b.len() && class(b.at(e)) == 1 {
                    e += 1;
                }
                if e == s {
                    return None;
                }
                let word = b.slice(s, e);
                self.last_search = Some((word.clone(), ch == '*'));
                Motion::excl(b.find(&word, pos, ch == '*')?)
            }
            _ => return None,
        };
        Some(m)
    }

    fn finish_motion(&mut self, m: Motion, b: &mut dyn Buffer) -> Effect {
        if !m.keep_goal {
            self.goal = None;
        }
        self.count = None;
        if let Some((op, _)) = self.op.take() {
            let from = b.cursor();
            return self.apply(op, from, m.to, m.linewise, m.inclusive, b);
        }
        if self.visual().is_some() {
            self.cur = m.to.min(b.len());
            self.show_visual(b);
        } else {
            let c = clamp_normal(b, m.to);
            b.set_cursor(c);
        }
        Effect::None
    }

    fn operator(&mut self, op: char, b: &mut dyn Buffer) -> Effect {
        if let Some(line) = self.visual() {
            let (s, e) = (self.anchor.min(self.cur), self.anchor.max(self.cur));
            self.mode = Mode::Normal;
            self.clear_pending();
            let linewise = line || matches!(op, '>' | '<');
            return self.apply(op, s, e, linewise, true, b);
        }
        let n = self.count.take().unwrap_or(1).max(1);
        match self.op {
            // dd, cc, yy, >>, <<: whole lines.
            Some((prev, pn)) if prev == op => {
                self.op = None;
                let total = pn * n;
                let pos = b.cursor();
                let mut last = pos;
                for _ in 1..total {
                    let le = line_end(b, last);
                    if le >= b.len() {
                        break;
                    }
                    last = le + 1;
                }
                self.apply(op, pos, last, true, false, b)
            }
            _ => {
                self.op = Some((op, n));
                Effect::None
            }
        }
    }

    fn yank(&mut self, text: String, linewise: bool, b: &mut dyn Buffer) {
        b.set_clipboard(&text);
        self.reg = Register { text, linewise };
    }

    /// Run operator `op` over `from`..`to`.
    fn apply(
        &mut self,
        op: char,
        from: usize,
        to: usize,
        linewise: bool,
        inclusive: bool,
        b: &mut dyn Buffer,
    ) -> Effect {
        let n = b.len();
        let (s, e) = (from.min(to), from.max(to));
        if linewise {
            let (ls, le) = (line_start(b, s), line_end(b, e));
            let lines = line_index(b, le) - line_index(b, ls) + 1;
            match op {
                'y' => {
                    let text = format!("{}\n", b.slice(ls, le));
                    self.yank(text, true, b);
                    b.set_cursor(clamp_normal(b, from.min(to).max(ls)));
                    if lines > 2 {
                        self.message = Some(format!("{lines} lines yanked"));
                    }
                }
                'd' => {
                    let text = format!("{}\n", b.slice(ls, le));
                    self.yank(text, true, b);
                    if le < n {
                        b.replace(ls, le + 1, "");
                    } else if ls > 0 {
                        b.replace(ls - 1, le, "");
                    } else {
                        b.replace(ls, le, "");
                    }
                    let at = ls.min(b.len());
                    let c = first_nonblank(b, if at == b.len() && at > 0 { at - 1 } else { at });
                    b.set_cursor(c);
                    if lines > 2 {
                        self.message = Some(format!("{lines} fewer lines"));
                    }
                }
                'c' => {
                    let text = format!("{}\n", b.slice(ls, le));
                    self.yank(text, true, b);
                    let indent = b.slice(ls, first_nonblank(b, ls));
                    b.replace(ls, le, &indent);
                    self.enter_insert(b, ls + indent.encode_utf16().count());
                }
                '>' | '<' => {
                    let unit = b.indent_unit();
                    let unit_len = unit.encode_utf16().count();
                    let mut start = ls;
                    for _ in 0..lines {
                        let le = line_end(b, start);
                        if op == '>' {
                            if le > start {
                                b.replace(start, start, &unit);
                            }
                        } else {
                            let ws = first_nonblank(b, start) - start;
                            let cut = if ws > 0 && b.at(start) == b'\t' as u16 {
                                1
                            } else {
                                ws.min(unit_len)
                            };
                            b.replace(start, start + cut, "");
                        }
                        let le = line_end(b, start);
                        if le >= b.len() {
                            break;
                        }
                        start = le + 1;
                    }
                    let c = first_nonblank(b, ls);
                    b.set_cursor(c);
                }
                _ => {}
            }
            return Effect::None;
        }
        let e = if inclusive { (e + 1).min(n) } else { e };
        let text = b.slice(s, e);
        match op {
            'y' => {
                self.yank(text, false, b);
                b.set_cursor(clamp_normal(b, s));
            }
            'd' => {
                self.yank(text, false, b);
                b.replace(s, e, "");
                let c = clamp_normal(b, s);
                b.set_cursor(c);
            }
            'c' => {
                self.yank(text, false, b);
                b.replace(s, e, "");
                self.enter_insert(b, s);
            }
            '>' | '<' => return self.apply(op, s, e.saturating_sub(1).max(s), true, false, b),
            _ => {}
        }
        Effect::None
    }

    fn command(&mut self, ch: char, pos: usize, b: &mut dyn Buffer) -> Effect {
        let n = self.take_count();
        let (ls, le) = (line_start(b, pos), line_end(b, pos));
        if let Some(line) = self.visual() {
            match ch {
                'o' => {
                    std::mem::swap(&mut self.anchor, &mut self.cur);
                    self.show_visual(b);
                }
                'x' => return self.operator('d', b),
                's' => return self.operator('c', b),
                'v' | 'V' => {
                    let want_line = ch == 'V';
                    if want_line == line {
                        self.mode = Mode::Normal;
                        b.set_cursor(clamp_normal(b, self.cur));
                    } else {
                        self.mode = Mode::Visual { line: want_line };
                        self.show_visual(b);
                    }
                }
                'J' => {
                    let (s, e) = (self.anchor.min(self.cur), self.anchor.max(self.cur));
                    let lines = line_index(b, e) - line_index(b, s) + 1;
                    self.mode = Mode::Normal;
                    b.set_cursor(s);
                    self.join(b, s, lines.max(2));
                }
                ':' => {
                    self.mode = Mode::Command(String::new());
                }
                _ => {}
            }
            return Effect::None;
        }
        match ch {
            'i' => self.enter_insert(b, pos),
            'a' => self.enter_insert(b, if pos < le { pos + 1 } else { pos }),
            'I' => {
                let at = first_nonblank(b, pos);
                self.enter_insert(b, at);
            }
            'A' => self.enter_insert(b, le),
            'o' => {
                let indent = b.slice(ls, first_nonblank(b, pos));
                b.replace(le, le, &format!("\n{indent}"));
                self.enter_insert(b, le + 1 + indent.encode_utf16().count());
            }
            'O' => {
                let indent = b.slice(ls, first_nonblank(b, pos));
                b.replace(ls, ls, &format!("{indent}\n"));
                self.enter_insert(b, ls + indent.encode_utf16().count());
            }
            'x' => {
                let e = (pos + n).min(le);
                if e > pos {
                    let text = b.slice(pos, e);
                    self.yank(text, false, b);
                    b.replace(pos, e, "");
                    let c = clamp_normal(b, pos);
                    b.set_cursor(c);
                }
            }
            'X' => {
                let s = pos.saturating_sub(n).max(ls);
                if s < pos {
                    let text = b.slice(s, pos);
                    self.yank(text, false, b);
                    b.replace(s, pos, "");
                    b.set_cursor(s);
                }
            }
            'D' => return self.apply('d', pos, le, false, false, b),
            'C' => return self.apply('c', pos, le, false, false, b),
            's' => return self.apply('c', pos, (pos + n).min(le), false, false, b),
            'S' => {
                self.op = Some(('c', 1));
                return self.operator('c', b);
            }
            'Y' => {
                self.op = Some(('y', n));
                return self.operator('y', b);
            }
            'p' | 'P' => self.paste(b, pos, ch == 'p', n),
            'J' => self.join(b, pos, n.max(2)),
            '~' => {
                let e = (pos + n).min(le);
                let flipped: String = b
                    .slice(pos, e)
                    .chars()
                    .map(|c| {
                        if c.is_lowercase() {
                            c.to_uppercase().next().unwrap_or(c)
                        } else {
                            c.to_lowercase().next().unwrap_or(c)
                        }
                    })
                    .collect();
                b.replace(pos, e, &flipped);
                let c = clamp_normal(b, e);
                b.set_cursor(c);
            }
            'u' => {
                for _ in 0..n {
                    b.undo();
                }
                let c = clamp_normal(b, b.cursor());
                b.set_cursor(c);
            }
            'v' | 'V' => {
                self.anchor = pos;
                self.cur = pos;
                self.mode = Mode::Visual { line: ch == 'V' };
                self.show_visual(b);
            }
            ':' => self.mode = Mode::Command(String::new()),
            '/' | '?' => {
                self.mode = Mode::Search {
                    forward: ch == '/',
                    text: String::new(),
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn paste(&mut self, b: &mut dyn Buffer, pos: usize, after: bool, n: usize) {
        if let Some(cb) = b.clipboard()
            && !cb.is_empty()
            && cb != self.reg.text
        {
            self.reg = Register {
                linewise: cb.ends_with('\n'),
                text: cb,
            };
        }
        let reg = self.reg.clone();
        if reg.text.is_empty() {
            return;
        }
        let text = reg.text.repeat(n);
        let len = text.encode_utf16().count();
        if reg.linewise {
            let le = line_end(b, pos);
            if after {
                if le < b.len() {
                    b.replace(le + 1, le + 1, &text);
                    let c = first_nonblank(b, le + 1);
                    b.set_cursor(c);
                } else {
                    let body = text.strip_suffix('\n').unwrap_or(&text);
                    b.replace(le, le, &format!("\n{body}"));
                    let c = first_nonblank(b, le + 1);
                    b.set_cursor(c);
                }
            } else {
                let ls = line_start(b, pos);
                b.replace(ls, ls, &text);
                let c = first_nonblank(b, ls);
                b.set_cursor(c);
            }
        } else {
            let le = line_end(b, pos);
            let at = if after && pos < le { pos + 1 } else { pos };
            b.replace(at, at, &text);
            b.set_cursor(at + len - 1);
        }
    }

    fn join(&mut self, b: &mut dyn Buffer, pos: usize, lines: usize) {
        let mut at = pos;
        for _ in 1..lines {
            let le = line_end(b, at);
            if le >= b.len() {
                break;
            }
            let mut j = le + 1;
            let next_end = line_end(b, j);
            while j < next_end && is_space(b.at(j)) {
                j += 1;
            }
            let sep = if j == next_end || b.at(j) == b')' as u16 {
                ""
            } else {
                " "
            };
            b.replace(le, j, sep);
            at = le;
        }
        b.set_cursor(at);
    }

    fn command_line(&mut self, key: Key, mut text: String, b: &mut dyn Buffer) -> Effect {
        match key {
            Key::Esc | Key::Ctrl('[') | Key::Ctrl('c') => self.mode = Mode::Normal,
            Key::Backspace => {
                if text.pop().is_none() {
                    self.mode = Mode::Normal;
                } else {
                    self.mode = Mode::Command(text);
                }
            }
            Key::Char(c) => {
                text.push(c);
                self.mode = Mode::Command(text);
            }
            Key::Enter => {
                self.mode = Mode::Normal;
                let c = clamp_normal(b, b.cursor());
                b.set_cursor(c);
                let cmd = text.trim();
                if let Ok(line) = cmd.parse::<usize>() {
                    let to = first_nonblank(b, b.line_offset(line.max(1) - 1));
                    b.set_cursor(to);
                    b.scroll(to, Scroll::Center);
                    return Effect::None;
                }
                return match cmd {
                    "w" | "write" => Effect::Save,
                    "q" | "quit" | "close" => Effect::Close { force: false },
                    "q!" | "quit!" => Effect::Close { force: true },
                    "wq" | "x" | "xit" => Effect::SaveClose,
                    "noh" | "nohlsearch" | "" => Effect::None,
                    _ => {
                        self.message = Some(format!("Not an editor command: {cmd}"));
                        Effect::None
                    }
                };
            }
            Key::Ctrl(_) => {}
        }
        Effect::None
    }

    fn search_line(&mut self, key: Key, forward: bool, mut text: String, b: &mut dyn Buffer) -> Effect {
        match key {
            Key::Esc | Key::Ctrl('[') | Key::Ctrl('c') => self.mode = Mode::Normal,
            Key::Backspace => {
                if text.pop().is_none() {
                    self.mode = Mode::Normal;
                } else {
                    self.mode = Mode::Search { forward, text };
                }
            }
            Key::Char(c) => {
                text.push(c);
                self.mode = Mode::Search { forward, text };
            }
            Key::Enter => {
                self.mode = Mode::Normal;
                if !text.is_empty() {
                    self.last_search = Some((text, forward));
                }
                if let Some(m) = self.motion('n', b.cursor(), b) {
                    return self.finish_motion(m, b);
                }
            }
            Key::Ctrl(_) => {}
        }
        Effect::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory buffer with snapshot undo.
    struct Mem {
        text: Vec<u16>,
        sel: (usize, usize),
        undo: Vec<Vec<u16>>,
        redo: Vec<Vec<u16>>,
        clip: Option<String>,
    }

    impl Mem {
        fn new(s: &str) -> Mem {
            Mem {
                text: s.encode_utf16().collect(),
                sel: (0, 0),
                undo: vec![],
                redo: vec![],
                clip: None,
            }
        }
        fn s(&self) -> String {
            String::from_utf16(&self.text).unwrap()
        }
    }

    impl Buffer for Mem {
        fn len(&self) -> usize {
            self.text.len()
        }
        fn at(&self, i: usize) -> u16 {
            self.text[i]
        }
        fn slice(&self, s: usize, e: usize) -> String {
            String::from_utf16(&self.text[s..e]).unwrap()
        }
        fn cursor(&self) -> usize {
            self.sel.0
        }
        fn set_cursor(&mut self, i: usize) {
            self.sel = (i, i);
        }
        fn set_selection(&mut self, s: usize, e: usize) {
            self.sel = (s, e);
        }
        fn replace(&mut self, s: usize, e: usize, t: &str) {
            self.undo.push(self.text.clone());
            self.redo.clear();
            self.text.splice(s..e, t.encode_utf16());
        }
        fn undo(&mut self) {
            if let Some(t) = self.undo.pop() {
                self.redo.push(std::mem::replace(&mut self.text, t));
            }
        }
        fn redo(&mut self) {
            if let Some(t) = self.redo.pop() {
                self.undo.push(std::mem::replace(&mut self.text, t));
            }
        }
        fn page_lines(&self) -> usize {
            10
        }
        fn clipboard(&self) -> Option<String> {
            self.clip.clone()
        }
        fn set_clipboard(&mut self, t: &str) {
            self.clip = Some(t.into());
        }
    }

    fn keys(v: &mut Vim, b: &mut Mem, ks: &str) -> Effect {
        let mut last = Effect::None;
        for c in ks.chars() {
            let k = match c {
                '⎋' => Key::Esc,
                '⏎' => Key::Enter,
                '⌫' => Key::Backspace,
                c => Key::Char(c),
            };
            last = v.handle(k, b);
            if last == Effect::PassThrough {
                // Insert mode: the text view would type it.
                let at = b.cursor();
                b.replace(at, at, &c.to_string());
                b.set_cursor(at + 1);
            }
        }
        last
    }

    fn run(text: &str, ks: &str) -> (String, usize) {
        let mut v = Vim::new();
        let mut b = Mem::new(text);
        keys(&mut v, &mut b, ks);
        (b.s(), b.cursor())
    }

    #[test]
    fn word_motions() {
        let mut v = Vim::new();
        let mut b = Mem::new("let foo = bar(1);\nnext line");
        keys(&mut v, &mut b, "w");
        assert_eq!(b.cursor(), 4);
        keys(&mut v, &mut b, "ww");
        assert_eq!(b.cursor(), 10);
        keys(&mut v, &mut b, "e");
        assert_eq!(b.cursor(), 12);
        keys(&mut v, &mut b, "b");
        assert_eq!(b.cursor(), 10);
        keys(&mut v, &mut b, "$");
        assert_eq!(b.cursor(), 16);
        keys(&mut v, &mut b, "w");
        assert_eq!(b.cursor(), 18);
        keys(&mut v, &mut b, "0");
        assert_eq!(b.cursor(), 18);
    }

    #[test]
    fn delete_change_yank() {
        assert_eq!(run("one two three", "dw").0, "two three");
        assert_eq!(run("one two three", "2dw").0, "three");
        assert_eq!(run("one two\nthree", "wdw").0, "one \nthree");
        assert_eq!(run("one two three", "cwuno⎋").0, "uno two three");
        assert_eq!(run("one two three", "wd$").0, "one ");
        assert_eq!(run("one two three", "wD").0, "one ");
        assert_eq!(run("abc", "x").0, "bc");
        assert_eq!(run("abc", "3x").0, "");
        assert_eq!(run("a b c", "fbd0").0, "b c");
        assert_eq!(run("a(b)c", "f(ldt)").0, "a()c");
        assert_eq!(run("a(b)c", "f(dt)").0, "a)c");
        assert_eq!(run("hello", "~~").0, "HEllo");
        assert_eq!(run("hello", "rj").0, "jello");
    }

    #[test]
    fn linewise_delete_yank_paste() {
        assert_eq!(run("a\nb\nc\n", "dd").0, "b\nc\n");
        assert_eq!(run("a\nb\nc", "jjdd").0, "a\nb");
        assert_eq!(run("a\nb\nc\n", "2dd").0, "c\n");
        assert_eq!(run("a\nb\nc\n", "ddp").0, "b\na\nc\n");
        assert_eq!(run("a\nb\nc", "yyjjp").0, "a\nb\nc\na");
        assert_eq!(run("a\nb\n", "yyP").0, "a\na\nb\n");
        assert_eq!(run("a\nb\nc\n", "dj").0, "c\n");
        assert_eq!(run("a\nb\nc\nd", "jdG").0, "a");
        assert_eq!(run("a\nb\nc\nd", "Gdgg").0, "");
        assert_eq!(run("  x\ny", "ccz⎋").0, "  z\ny");
    }

    #[test]
    fn charwise_paste_and_clipboard() {
        assert_eq!(run("ab", "xp").0, "ba");
        assert_eq!(run("one two", "yiw").0, "one two"); // unknown text object: no-op
        let mut v = Vim::new();
        let mut b = Mem::new("hi");
        keys(&mut v, &mut b, "yl");
        assert_eq!(b.clip.as_deref(), Some("h"));
        b.clip = Some("XY".into()); // copied in another app
        keys(&mut v, &mut b, "p");
        assert_eq!(b.s(), "hXYi");
    }

    #[test]
    fn vertical_keeps_column() {
        let mut v = Vim::new();
        let mut b = Mem::new("abcdef\nab\nabcdef");
        keys(&mut v, &mut b, "4l");
        keys(&mut v, &mut b, "j");
        assert_eq!(b.cursor(), 8); // clamped to "b"
        keys(&mut v, &mut b, "j");
        assert_eq!(b.cursor(), 14); // back to column 4
        keys(&mut v, &mut b, "gg");
        assert_eq!(b.cursor(), 0);
        keys(&mut v, &mut b, "3G");
        assert_eq!(b.cursor(), 10);
    }

    #[test]
    fn insert_modes() {
        assert_eq!(run("b", "iA⎋").0, "Ab");
        assert_eq!(run("b", "aZ⎋").0, "bZ");
        assert_eq!(run("  x", "Iy⎋").0, "  yx");
        assert_eq!(run("x\ny", "A;⎋").0, "x;\ny");
        assert_eq!(run("  x\ny", "oz⎋").0, "  x\n  z\ny");
        assert_eq!(run("x", "Oz⎋").0, "z\nx");
        let (t, c) = run("abc", "A⎋");
        assert_eq!((t.as_str(), c), ("abc", 2));
    }

    #[test]
    fn visual_mode() {
        assert_eq!(run("one two", "vlld").0, " two");
        assert_eq!(run("a\nb\nc\n", "Vjd").0, "c\n");
        assert_eq!(run("a\nb\n", "Vy").0, "a\nb\n");
        assert_eq!(run("a\nb", "VjJ").0, "a b");
        assert_eq!(run("x\ny", "Vj>").0, "    x\n    y");
    }

    #[test]
    fn join_indent_undo() {
        assert_eq!(run("a\n   b", "J").0, "a b");
        assert_eq!(run("x", ">>").0, "    x");
        assert_eq!(run("        x", "<<").0, "    x");
        assert_eq!(run("abc", "xxu").0, "bc");
        assert_eq!(run("abc", "xxu\u{0}").0, "bc");
        let mut v = Vim::new();
        let mut b = Mem::new("abc");
        keys(&mut v, &mut b, "xu");
        v.handle(Key::Ctrl('r'), &mut b);
        assert_eq!(b.s(), "bc");
    }

    #[test]
    fn search_and_brackets() {
        let mut v = Vim::new();
        let mut b = Mem::new("foo bar foo baz foo");
        keys(&mut v, &mut b, "/foo⏎");
        assert_eq!(b.cursor(), 8);
        keys(&mut v, &mut b, "n");
        assert_eq!(b.cursor(), 16);
        keys(&mut v, &mut b, "n");
        assert_eq!(b.cursor(), 0); // wraps
        keys(&mut v, &mut b, "N");
        assert_eq!(b.cursor(), 16);
        keys(&mut v, &mut b, "w*");
        assert_eq!(b.cursor(), 0);
        assert_eq!(run("f(a, (b))", "%").1, 8);
        assert_eq!(run("a\n\nb\nc\n\nd", "}").1, 2);
        assert_eq!(run("a\n\nb\nc\n\nd", "}}").1, 7);
    }

    #[test]
    fn command_line() {
        let mut v = Vim::new();
        let mut b = Mem::new("a\nb\nc");
        assert_eq!(keys(&mut v, &mut b, ":w⏎"), Effect::Save);
        assert_eq!(keys(&mut v, &mut b, ":q!⏎"), Effect::Close { force: true });
        assert_eq!(keys(&mut v, &mut b, ":wq⏎"), Effect::SaveClose);
        keys(&mut v, &mut b, ":3⏎");
        assert_eq!(b.cursor(), 4);
        keys(&mut v, &mut b, ":nope⏎");
        assert_eq!(v.status(), "Not an editor command: nope");
        keys(&mut v, &mut b, ":x⌫⌫");
        assert_eq!(v.mode, Mode::Normal);
        assert_eq!(keys(&mut v, &mut b, "gt"), Effect::NextTab);
    }

    #[test]
    fn status_shows_pending() {
        let mut v = Vim::new();
        let mut b = Mem::new("abc");
        keys(&mut v, &mut b, "2d");
        assert_eq!(v.status(), "NORMAL  2d");
        keys(&mut v, &mut b, "⎋i");
        assert_eq!(v.status(), "-- INSERT --");
        assert!(!v.block_cursor());
    }
}
