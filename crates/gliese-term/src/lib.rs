//! The terminal's screen (`PLAN.md` §8.1): what programs and the shell printed, as logical lines (wrapped to the window's width
//! when drawn, so a resize never loses text), a scrollback, and a small part of ANSI: colours (SGR 0, 1, 30-37, 39, 40-47, 49,
//! 90-97), clearing the screen (`ESC[2J`) and the rest of a line (`ESC[K`), carriage return and backspace.
//!
//! Drawing uses metal-gfx's 5x9 bitmap font at twice its size: a cell is [`CELL_W`] x [`CELL_H`] pixels.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod logo;

use alloc::vec;
use alloc::vec::Vec;
use metal_gfx::font;

pub const SCALE: i32 = 2;
pub const CELL_W: i32 = font::ADVANCE * SCALE;
pub const CELL_H: i32 = font::H * SCALE + 2;
/// Room around the text inside the window.
pub const PAD: i32 = 6;
/// Lines kept for scrolling back.
pub const SCROLLBACK: usize = 1000;

/// The 16 colours (Nord-like), and the screen's own two.
pub const PALETTE: [u32; 16] = [
    0x3B4252, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x88C0D0, 0xE5E9F0, //
    0x4C566A, 0xD08770, 0xB5D3A0, 0xF0D8A0, 0x9CB8D8, 0xC9A8C2, 0x8FBCBB, 0xECEFF4,
];
pub const BACKGROUND: u32 = 0x1D2128;
pub const FOREGROUND: u32 = 0xD8DEE9;
/// Colour index meaning "the default".
pub const DEFAULT: u8 = 0xFF;

/// One character cell: the byte (printable ASCII; anything else shows as `?`) and its colours (palette indices or [`DEFAULT`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: u8,
    pub fg: u8,
    pub bg: u8,
}

impl Cell {
    pub const fn plain(ch: u8) -> Cell {
        Cell { ch, fg: DEFAULT, bg: DEFAULT }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Esc {
    None,
    Escape,
    /// In `ESC [`: the numbers so far.
    Csi([u16; 4], usize, bool),
}

/// Columns the `fetch` logo takes beside its text (the logo is `logo::SIZE` pixels square).
pub const LOGO_COLS: usize = (logo::SIZE as i32 / CELL_W + 2) as usize;
/// Rows it takes.
pub const LOGO_ROWS: usize = logo::SIZE.div_ceil(CELL_H as usize);
/// The labels' column beside the logo (`KERNEL   …`): a line beside the logo that wraps goes on under the value, past this.
pub const LOGO_KEY_COLS: usize = 9;
/// Below this many columns the logo steps aside and its lines use the whole width.
pub const LOGO_MIN_COLS: usize = LOGO_COLS + LOGO_KEY_COLS + 12;

/// A row as drawn: `indent` blank columns, then cells.
#[derive(Clone, Copy, Debug)]
struct Row<'a> {
    indent: usize,
    cells: &'a [Cell],
}

impl Row<'_> {
    const EMPTY: Row<'static> = Row { indent: 0, cells: &[] };
    fn len(&self) -> usize {
        self.indent + self.cells.len()
    }
}

pub struct Term {
    /// Logical lines; the last is the one being written.
    lines: Vec<Vec<Cell>>,
    /// Lines dropped off the top of the scrollback so far (so a line keeps its number).
    dropped: usize,
    /// Logos drawn beside the text, by the number of the line they start at.
    logos: Vec<usize>,
    /// Where the next byte goes in the last line.
    col: usize,
    fg: u8,
    bg: u8,
    esc: Esc,
    /// How many wrapped rows the view is scrolled back (0: at the bottom).
    pub scroll: usize,
}

impl Default for Term {
    fn default() -> Term {
        Term::new()
    }
}

impl Term {
    pub fn new() -> Term {
        Term { lines: vec![Vec::new()], dropped: 0, logos: Vec::new(), col: 0, fg: DEFAULT, bg: DEFAULT, esc: Esc::None, scroll: 0 }
    }

    /// The columns and rows that fit a window of `w` x `h` pixels.
    pub fn grid_for(w: u32, h: u32) -> (usize, usize) {
        (((w as i32 - 2 * PAD) / CELL_W).max(1) as usize, ((h as i32 - 2 * PAD) / CELL_H).max(1) as usize)
    }

    pub fn clear(&mut self) {
        self.dropped += self.lines.len();
        self.logos.clear();
        self.lines = vec![Vec::new()];
        self.col = 0;
        self.scroll = 0;
    }

    /// The text of every line (for tests and for copying).
    pub fn text(&self) -> Vec<alloc::string::String> {
        self.lines.iter().map(|l| l.iter().map(|c| c.ch as char).collect()).collect()
    }

    fn put(&mut self, ch: u8) {
        let cell = Cell { ch, fg: self.fg, bg: self.bg };
        let line = self.lines.last_mut().expect("there is always a line");
        if self.col < line.len() {
            line[self.col] = cell;
        } else {
            while line.len() < self.col {
                line.push(Cell::plain(b' '));
            }
            line.push(cell);
        }
        self.col += 1;
    }

    fn newline(&mut self) {
        self.lines.push(Vec::new());
        self.col = 0;
        if self.lines.len() > SCROLLBACK {
            let extra = self.lines.len() - SCROLLBACK;
            self.lines.drain(..extra);
            self.dropped += extra;
            let first = self.dropped;
            self.logos.retain(|l| *l + LOGO_ROWS > first);
        }
    }

    /// Bytes from a program or the shell.
    pub fn write(&mut self, bytes: &[u8]) {
        self.scroll = 0;
        for &b in bytes {
            self.byte(b);
        }
    }

    pub fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
    }

    fn byte(&mut self, b: u8) {
        match self.esc {
            Esc::Escape => {
                self.esc = if b == b'[' { Esc::Csi([0; 4], 0, false) } else { Esc::None };
                return;
            }
            Esc::Csi(mut p, mut n, mut any) => {
                match b {
                    b'0'..=b'9' => {
                        if n < 4 {
                            p[n] = p[n].saturating_mul(10).saturating_add((b - b'0') as u16);
                        }
                        any = true;
                        self.esc = Esc::Csi(p, n, any);
                    }
                    b';' => {
                        n += 1;
                        self.esc = Esc::Csi(p, n, any);
                    }
                    b'?' => self.esc = Esc::Csi(p, n, any),
                    _ => {
                        self.esc = Esc::None;
                        self.csi(b, &p[..(n + 1).min(4)], any);
                    }
                }
                return;
            }
            Esc::None => {}
        }
        match b {
            0x1B => self.esc = Esc::Escape,
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => {
                // to the next stop, over what is there (padding a short line)
                let next = (self.col / 8 + 1) * 8;
                let line = self.lines.last_mut().expect("there is always a line");
                while line.len() < next {
                    line.push(Cell::plain(b' '));
                }
                self.col = next;
            }
            0x07 | 0x00 => {}
            0x20..=0x7E => self.put(b),
            _ if b >= 0x80 => {
                // (UTF-8: one '?' a character, for its first byte)
                if b & 0xC0 != 0x80 {
                    self.put(b'?');
                }
            }
            _ => {}
        }
    }

    fn csi(&mut self, cmd: u8, p: &[u16], any: bool) {
        match cmd {
            b'm' => {
                if !any {
                    self.fg = DEFAULT;
                    self.bg = DEFAULT;
                }
                for &v in p.iter().filter(|_| any) {
                    match v {
                        0 => {
                            self.fg = DEFAULT;
                            self.bg = DEFAULT;
                        }
                        30..=37 => self.fg = (v - 30) as u8,
                        90..=97 => self.fg = (v - 90 + 8) as u8,
                        39 => self.fg = DEFAULT,
                        40..=47 => self.bg = (v - 40) as u8,
                        100..=107 => self.bg = (v - 100 + 8) as u8,
                        49 => self.bg = DEFAULT,
                        _ => {}
                    }
                }
            }
            b'J' if p[0] == 2 || p[0] == 3 => self.clear(),
            b'K' => {
                let col = self.col;
                if let Some(l) = self.lines.last_mut() {
                    l.truncate(col);
                }
            }
            b'H' if !any => self.col = 0,
            _ => {}
        }
    }

    /// The `fetch` logo, beside the next [`LOGO_ROWS`] lines (which the caller indents by [`LOGO_COLS`]).
    pub fn logo(&mut self) {
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.newline();
        }
        self.logos.push(self.dropped + self.lines.len() - 1);
    }

    /// The rows as drawn at `cols` wide: every logical line wrapped, then `live` (the prompt and what is being typed) at the end.
    /// Returns the rows and where in them `live`'s cursor is (row, column).
    fn rows<'a>(&'a self, cols: usize, live: &'a [Cell], cursor: usize) -> (Vec<Row<'a>>, (usize, usize)) {
        let (rows, cur, _) = self.rows_and_starts(cols, live, cursor);
        (rows, cur)
    }

    /// Like `rows`, and the row each logical line starts at.
    fn rows_and_starts<'a>(&'a self, cols: usize, live: &'a [Cell], cursor: usize) -> (Vec<Row<'a>>, (usize, usize), Vec<usize>) {
        let mut rows: Vec<Row<'a>> = Vec::new();
        let mut starts = Vec::with_capacity(self.lines.len());
        let n = self.lines.len();
        let wide = cols >= LOGO_MIN_COLS;
        for (i, l) in self.lines.iter().enumerate() {
            starts.push(rows.len());
            // the last line is drawn together with `live` when it has text (a program's prompt with no newline yet)
            if i == n - 1 && l.is_empty() {
                break;
            }
            if l.is_empty() {
                rows.push(Row::EMPTY);
                continue;
            }
            let abs = self.dropped + i;
            if self.logos.iter().any(|s| (*s..*s + LOGO_ROWS).contains(&abs)) {
                // beside the logo: wrapped under the value (or, too narrow for the logo, without the logo's indent)
                let cells = if wide { l } else { l.get(LOGO_COLS..).unwrap_or(&[]) };
                let hang = if wide { LOGO_COLS + LOGO_KEY_COLS } else { LOGO_KEY_COLS };
                let hang = if cols > hang + 4 { hang } else { 0 };
                // word wrap: a row breaks after its last space, when it has one past the label
                let mut rest = cells;
                let mut indent = 0;
                loop {
                    let width = cols - indent;
                    if rest.len() <= width {
                        rows.push(Row { indent, cells: rest });
                        break;
                    }
                    let cut = rest[..width].iter().rposition(|c| c.ch == b' ').filter(|p| indent + p > hang).map_or(width, |p| p + 1);
                    rows.push(Row { indent, cells: &rest[..cut] });
                    rest = &rest[cut..];
                    while rest.first().is_some_and(|c| c.ch == b' ') {
                        rest = &rest[1..];
                    }
                    if rest.is_empty() {
                        break;
                    }
                    indent = hang;
                }
                continue;
            }
            for chunk in l.chunks(cols) {
                rows.push(Row { indent: 0, cells: chunk });
            }
        }
        let start = rows.len();
        if live.is_empty() {
            rows.push(Row::EMPTY);
        }
        for chunk in live.chunks(cols) {
            rows.push(Row { indent: 0, cells: chunk });
        }
        if !live.is_empty() && live.len().is_multiple_of(cols) && cursor == live.len() {
            rows.push(Row::EMPTY);
        }
        (rows, (start + cursor / cols, cursor % cols), starts)
    }

    /// Scroll the view back (positive) or forward, by rows.
    pub fn scroll_by(&mut self, d: i32, cols: usize) {
        let total = self.rows(cols, &[], 0).0.len();
        self.scroll = (self.scroll as i32 + d).clamp(0, total.saturating_sub(1) as i32) as usize;
    }

    /// Draw into a `w` x `h` buffer: the lines, then `live` with a block cursor at `cursor` (shown if `cursor_on`).
    pub fn draw(&self, px: &mut [u32], w: u32, h: u32, live: &[Cell], cursor: usize, cursor_on: bool) {
        let (cols, nrows) = Term::grid_for(w, h);
        px.fill(BACKGROUND);
        let (rows, (cr, cc), starts) = self.rows_and_starts(cols, live, cursor);
        let first = rows.len().saturating_sub(nrows + self.scroll);
        let last = (first + nrows).min(rows.len());
        // logos first (text never overlaps them: their lines are indented); one that starts above the view is drawn cut off; in a
        // window too narrow for logo and text side by side, no logo
        for &l in self.logos.iter().filter(|_| cols >= LOGO_MIN_COLS) {
            let Some(&row) = l.checked_sub(self.dropped).and_then(|i| starts.get(i)) else { continue };
            if row + LOGO_ROWS <= first || row >= last {
                continue;
            }
            let y = PAD + (row as i32 - first as i32) * CELL_H;
            draw_logo(px, w, h, PAD, y);
        }
        for (r, row) in rows[first..last].iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate().map(|(c, x)| (c + row.indent, x)) {
                let inverse = cursor_on && first + r == cr && c == cc && self.scroll == 0;
                draw_cell(px, w, h, c, r, cell, inverse);
            }
        }
        // the cursor past the end of the text
        if cursor_on && self.scroll == 0 && (first..last).contains(&cr) && rows[cr].len() <= cc {
            draw_cell(px, w, h, cc, cr - first, &Cell::plain(b' '), true);
        }
    }

    /// Draw a ready-made grid (the editor's screen): `cells[row][col]` = (character, inverse), and a cursor.
    pub fn draw_grid(px: &mut [u32], w: u32, h: u32, cells: &[Vec<(char, bool)>], cursor: Option<(usize, usize)>) {
        px.fill(BACKGROUND);
        for (r, row) in cells.iter().enumerate() {
            for (c, (ch, inv)) in row.iter().enumerate() {
                let b = if ch.is_ascii() && !ch.is_ascii_control() { *ch as u8 } else { b'?' };
                let inv = *inv != (cursor == Some((c, r)));
                draw_cell(px, w, h, c, r, &Cell::plain(b), inv);
            }
        }
        if let Some((c, r)) = cursor {
            if cells.get(r).is_none_or(|row| row.len() <= c) {
                draw_cell(px, w, h, c, r, &Cell::plain(b' '), true);
            }
        }
    }
}

/// The logo at (x, y), which may be partly above the buffer (it is drawn into a scratch square, then copied, clipped).
fn draw_logo(px: &mut [u32], w: u32, h: u32, x: i32, y: i32) {
    let n = logo::SIZE;
    let mut sq = vec![BACKGROUND; n * n];
    logo::draw(&mut sq, n, 0, 0, PALETTE[8], FOREGROUND, PALETTE[6]);
    for (dy, row) in sq.chunks(n).enumerate() {
        let ty = y + dy as i32;
        if ty < 0 || ty >= h as i32 {
            continue;
        }
        for (dx, c) in row.iter().enumerate() {
            let tx = x + dx as i32;
            if *c != BACKGROUND && tx >= 0 && tx < w as i32 {
                px[(ty * w as i32 + tx) as usize] = *c;
            }
        }
    }
}

fn color(i: u8, default: u32) -> u32 {
    PALETTE.get(i as usize).copied().unwrap_or(default)
}

fn draw_cell(px: &mut [u32], w: u32, h: u32, col: usize, row: usize, cell: &Cell, inverse: bool) {
    let (x0, y0) = (PAD + col as i32 * CELL_W, PAD + row as i32 * CELL_H);
    let (mut fg, mut bg) = (color(cell.fg, FOREGROUND), color(cell.bg, BACKGROUND));
    if inverse {
        core::mem::swap(&mut fg, &mut bg);
    }
    let (w, h) = (w as i32, h as i32);
    if x0 + CELL_W > w || y0 + CELL_H > h {
        return;
    }
    if bg != BACKGROUND {
        for y in y0..y0 + CELL_H {
            let row = (y * w) as usize;
            px[row + x0 as usize..row + (x0 + CELL_W) as usize].fill(bg);
        }
    }
    let Some(g) = font::glyph(cell.ch).or_else(|| font::glyph(b'?')) else { return };
    for (gy, bits) in g.iter().enumerate() {
        for gx in 0..font::W {
            if bits & (1 << (font::W - 1 - gx)) == 0 {
                continue;
            }
            for dy in 0..SCALE {
                let y = y0 + 1 + gy as i32 * SCALE + dy;
                let row = (y * w) as usize;
                let x = (x0 + gx * SCALE) as usize;
                px[row + x..row + x + SCALE as usize].fill(fg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn lines(t: &Term) -> Vec<String> {
        t.text()
    }

    #[test]
    fn text_newlines_and_carriage_returns() {
        let mut t = Term::new();
        t.write(b"hello\nworld\r\nab\rX");
        assert_eq!(lines(&t), ["hello", "world", "Xb"]);
        t.write(b"\tq");
        assert_eq!(lines(&t)[2], "Xb      q");
        t.write(b"\x08\x08Z");
        assert_eq!(lines(&t)[2], "Xb     Zq");
    }

    #[test]
    fn colours_and_clearing() {
        let mut t = Term::new();
        t.write(b"\x1b[31mred\x1b[0m plain \x1b[1;94mblue\x1b[m");
        let l = &t.lines[0];
        assert_eq!((l[0].ch, l[0].fg), (b'r', 1));
        assert_eq!(l[4].fg, DEFAULT);
        assert_eq!(l[11].fg, 12);
        t.write(b"\r\x1b[K");
        assert!(t.lines[0].is_empty());
        t.write(b"x\ny\x1b[2J");
        assert_eq!(lines(&t), [""]);
    }

    #[test]
    fn wrapping_is_done_when_drawing() {
        let mut t = Term::new();
        t.write(b"abcdefghij\n");
        let live: Vec<Cell> = b"$ ".iter().map(|b| Cell::plain(*b)).collect();
        let (rows, cur) = t.rows(4, &live, 2);
        assert_eq!(rows.len(), 4, "abcd efgh ij, then the prompt");
        assert_eq!(cur, (3, 2));
        let (rows, _) = t.rows(20, &live, 2);
        assert_eq!(rows.len(), 2, "wider: no wrap");
    }

    #[test]
    fn scrollback_is_bounded() {
        let mut t = Term::new();
        for i in 0..(SCROLLBACK + 50) {
            t.write(alloc::format!("{i}\n").as_bytes());
        }
        assert!(t.lines.len() <= SCROLLBACK);
        assert_eq!(t.text()[t.lines.len() - 2], alloc::format!("{}", SCROLLBACK + 49));
    }

    #[test]
    fn draws_into_a_buffer() {
        let (w, h) = (200u32, 100u32);
        let mut px = vec![0u32; (w * h) as usize];
        let mut t = Term::new();
        t.write(b"A");
        t.draw(&mut px, w, h, &[], 0, true);
        // the A's top row has its middle three columns lit
        let lit = px.iter().filter(|p| **p == FOREGROUND).count();
        assert!(lit > 20, "{lit} pixels of text");
        assert_eq!(Term::grid_for(1000, 744), (82, 36));
    }
}
