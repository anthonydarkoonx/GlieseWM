//! Drawing the screen: only damaged areas, back to front; windows are opaque, so each pixel is written once per window that covers
//! it. The pointer is drawn in software, last.
use crate::{intersect, Host, Rect, Server, State, Window};
use alloc::vec::Vec;

/// The arrow: `#` outline, `.` fill, 12 x 19, hot spot at the top left.
const ARROW: [&str; 19] = [
    "#           ",
    "##          ",
    "#.#         ",
    "#..#        ",
    "#...#       ",
    "#....#      ",
    "#.....#     ",
    "#......#    ",
    "#.......#   ",
    "#........#  ",
    "#.........# ",
    "#..........#",
    "#......#####",
    "#...#..#    ",
    "#..# #..#   ",
    "#.#  #..#   ",
    "##    #..#  ",
    "      #..#  ",
    "       ##   ",
];
pub(crate) const CURSOR_W: i32 = 12;
pub(crate) const CURSOR_H: i32 = 19;

impl State {
    pub(crate) fn cursor_rect(&self) -> Option<Rect> {
        let over = self.grab.or(self.pointer_over).and_then(|id| self.win(id));
        if over.is_some_and(|w| w.cursor == gliese_proto::cursor::NONE) {
            return None;
        }
        Some(Rect::new(self.pointer.0, self.pointer.1, CURSOR_W, CURSOR_H))
    }
    pub(crate) fn cursor_moved(&self) -> bool {
        self.cursor_rect() != self.cursor_drawn
    }
}

fn fill(fb: &mut [u32], stride: i32, r: Rect, color: u32) {
    for y in r.y..r.bottom() {
        let row = (y * stride) as usize;
        fb[row + r.x as usize..row + r.right() as usize].fill(color);
    }
}

impl Server {
    /// Bring `fb` (the screen, `stride` pixels a row) up to date. Returns the rectangles that changed, to present.
    pub fn render(&mut self, host: &mut dyn Host, fb: &mut [u32], stride: i32) -> Vec<Rect> {
        let screen = Rect::new(0, 0, self.st.screen.0, self.st.screen.1);
        // the pointer: where it was and where it is now
        let cur = self.st.cursor_rect();
        if cur != self.st.cursor_drawn {
            if let Some(r) = self.st.cursor_drawn {
                self.st.damage.add(r);
            }
            if let Some(r) = cur {
                self.st.damage.add(r);
            }
        }
        let damage: Vec<Rect> = core::mem::take(&mut self.st.damage.rects).iter().map(|r| intersect(r, &screen)).filter(|r| !r.is_empty()).collect();
        for r in &damage {
            self.paint(host, fb, stride, *r);
        }
        if let Some(text) = &self.st.error {
            let bar = error_bar(self.st.screen.0);
            if damage.iter().any(|d| d.overlaps(&bar)) {
                draw_error_bar(fb, stride, &screen, bar, text);
            }
        }
        if let Some(c) = cur {
            if damage.iter().any(|d| d.overlaps(&c)) {
                draw_cursor(fb, stride, &screen, c);
            }
        }
        self.st.cursor_drawn = cur;
        self.frames_done(host);
        self.st.flush_wakes(host);
        damage
    }

    fn paint(&mut self, host: &mut dyn Host, fb: &mut [u32], stride: i32, clip: Rect) {
        fill(fb, stride, clip, self.st.look.background);
        let focus = self.st.focus;
        let look = self.st.look;
        for w in self.st.windows.iter().filter(|w| w.visible() && w.outer.overlaps(&clip)) {
            let border = if focus == Some(w.id) { look.active_border } else { look.inactive_border };
            let pool = w.shown.and_then(|b| self.st.pool_shm(w.sess, b.pool));
            draw_window(host, fb, stride, clip, w, pool, border, look.background);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_window(host: &mut dyn Host, fb: &mut [u32], stride: i32, clip: Rect, w: &Window, pool: Option<crate::ShmId>, border: u32, background: u32) {
    let client = w.client_rect();
    // the border: the outer rectangle less the client area, as four bands
    let o = w.outer;
    let b = w.border;
    for band in [
        Rect::new(o.x, o.y, o.w, b),
        Rect::new(o.x, o.bottom() - b, o.w, b),
        Rect::new(o.x, o.y + b, b, o.h - 2 * b),
        Rect::new(o.right() - b, o.y + b, b, o.h - 2 * b),
    ] {
        let r = intersect(&band, &clip);
        if !r.is_empty() {
            fill(fb, stride, r, border);
        }
    }
    let area = intersect(&client, &clip);
    if area.is_empty() {
        return;
    }
    let Some(buf) = w.shown else {
        fill(fb, stride, area, background);
        return;
    };
    let (bw, bh) = (buf.w as i32, buf.h as i32);
    let base = buf.offset as usize * 1024 + w.front as usize * buf.w as usize * buf.h as usize;
    // the part of the client area the buffer covers (a buffer of the old size while a resize is under way is drawn clipped, the rest
    // is background)
    let covered = intersect(&area, &Rect::new(client.x, client.y, bw, bh));
    let Some(px) = pool.and_then(|s| host.shm(s)) else {
        fill(fb, stride, area, background);
        return;
    };
    // the part of the client area the buffer does not cover (a buffer of the old size while a resize is under way) is background
    if covered != area {
        fill(fb, stride, area, background);
    }
    for y in covered.y..covered.bottom() {
        let src = base + ((y - client.y) * bw + (covered.x - client.x)) as usize;
        let dst = (y * stride + covered.x) as usize;
        let n = covered.w as usize;
        match px.get(src..src + n) {
            Some(s) => fb[dst..dst + n].copy_from_slice(s),
            None => fb[dst..dst + n].fill(background),
        }
    }
}

fn draw_cursor(fb: &mut [u32], stride: i32, screen: &Rect, c: Rect) {
    for (dy, row) in ARROW.iter().enumerate() {
        for (dx, ch) in row.bytes().enumerate() {
            let (x, y) = (c.x + dx as i32, c.y + dy as i32);
            if !screen.contains(x, y) {
                continue;
            }
            let color = match ch {
                b'#' => 0x000000,
                b'.' => 0xFFFFFF,
                _ => continue,
            };
            fb[(y * stride + x) as usize] = color;
        }
    }
}

/// Where the error bar goes: across the top, tall enough for one line of the 5x9 font at twice its size.
pub(crate) fn error_bar(screen_w: i32) -> Rect {
    Rect::new(0, 0, screen_w, metallium_gfx::font::H * 2 + 10)
}

/// A dark red, unlike anything a window is likely to be, with a light line under it.
pub const BAR_BG: u32 = 0x7A1C24;
const BAR_LINE: u32 = 0xF2B8BE;
const BAR_FG: u32 = 0xFFFFFF;

fn draw_error_bar(fb: &mut [u32], stride: i32, screen: &Rect, bar: Rect, text: &str) {
    let bar = intersect(&bar, screen);
    fill(fb, stride, bar, BAR_BG);
    fill(fb, stride, intersect(&Rect::new(bar.x, bar.bottom() - 2, bar.w, 2), &bar), BAR_LINE);
    let scale = 2;
    let (mut x, y) = (8, 5);
    for ch in text.bytes() {
        if x + metallium_gfx::font::W * scale > bar.right() - 8 {
            break;
        }
        // (the font has printable ASCII; anything else shows as '?')
        let g = metallium_gfx::font::glyph(ch).or_else(|| metallium_gfx::font::glyph(b'?'));
        if let Some(rows) = g {
            for (gy, row) in rows.iter().enumerate() {
                for gx in 0..metallium_gfx::font::W {
                    if row & (1 << (metallium_gfx::font::W - 1 - gx)) != 0 {
                        let r = Rect::new(x + gx * scale, y + gy as i32 * scale, scale, scale);
                        fill(fb, stride, intersect(&r, &bar), BAR_FG);
                    }
                }
            }
        }
        x += metallium_gfx::font::ADVANCE * scale;
    }
}
