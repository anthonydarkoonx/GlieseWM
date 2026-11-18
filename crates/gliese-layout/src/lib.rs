//! GlieseWM's layouts and the gap arithmetic (`PLAN.md` §4).
//!
//! Everything here is pure: rectangles in, rectangles out. A [`Layout`] keeps the shape of one workspace (a dwindle tree, a master
//! list) and turns it into tiles with [`Layout::arrange`]. Every split goes through [`split`], which keeps the space between two
//! tiles at exactly the inner gap; [`arrange`] applies the outer gaps and `smart_gaps` first.
//!
//! Ratios are fixed point ([`Ratio`], basis points): `core` has no `round` for floats, and integer arithmetic gives the same
//! tiles on every machine.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

#[cfg(feature = "dwindle")]
pub mod dwindle;
#[cfg(feature = "master")]
pub mod master;

/// A window as the layouts know it (the compositor's window id).
pub type WinId = u32;

/// A rectangle in screen pixels. `w` and `h` are never negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w: if w < 0 { 0 } else { w }, h: if h < 0 { 0 } else { h } }
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        !self.is_empty() && !o.is_empty() && self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }
    /// Shrink by `s` on each side (never below zero size).
    pub fn shrink(&self, s: Sides) -> Rect {
        Rect::new(self.x + s.left, self.y + s.top, self.w - s.left - s.right, self.h - s.top - s.bottom)
    }
    /// The length along an axis: the width for [`Axis::X`], the height for [`Axis::Y`].
    pub const fn len(&self, axis: Axis) -> i32 {
        match axis {
            Axis::X => self.w,
            Axis::Y => self.h,
        }
    }
    /// The centre (rounded down).
    pub const fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// Four sizes, CSS order in the config (`top right bottom left`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Sides {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Sides {
    pub const fn all(v: i32) -> Sides {
        Sides { top: v, right: v, bottom: v, left: v }
    }
    pub const fn new(top: i32, right: i32, bottom: i32, left: i32) -> Sides {
        Sides { top, right, bottom, left }
    }
}

/// The direction a split cuts along: [`Axis::X`] puts the two parts side by side (a vertical divider), [`Axis::Y`] one above the
/// other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    pub const fn flip(self) -> Axis {
        match self {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        }
    }
    /// The axis that cuts a rectangle along its longer side (a wide tile splits side by side).
    pub const fn longer_side(r: &Rect) -> Axis {
        if r.w >= r.h {
            Axis::X
        } else {
            Axis::Y
        }
    }
}

/// A ratio in basis points: 0..=10000 is 0.0..=1.0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ratio(pub u16);

impl Ratio {
    pub const HALF: Ratio = Ratio(5000);
    pub const ONE: u16 = 10000;
    /// The least a split's ratio may be (and `ONE - MIN` the most): 5%, so no tile can be resized away.
    pub const MIN: u16 = 500;

    /// Clamp to `MIN..=ONE-MIN`.
    pub const fn clamped(v: i32) -> Ratio {
        let lo = Ratio::MIN as i32;
        let hi = (Ratio::ONE - Ratio::MIN) as i32;
        Ratio(if v < lo {
            lo
        } else if v > hi {
            hi
        } else {
            v
        } as u16)
    }
    /// Add a change in basis points, clamped.
    pub const fn plus(self, d: i32) -> Ratio {
        Ratio::clamped(self.0 as i32 + d)
    }
    /// `len * self`, rounded to the nearest pixel.
    pub const fn of(self, len: i32) -> i32 {
        ((len as i64 * self.0 as i64 + Ratio::ONE as i64 / 2) / Ratio::ONE as i64) as i32
    }
}

/// Cut `r` in two along `axis`, `ratio` of the usable length to the first part, with exactly `gap` pixels between them.
///
/// The usable length is `len - gap`; the first part gets `ratio` of it (rounded), at least 1 and at most `usable - 1` pixels, and
/// the second part the rest. Returns `(first, second, squeezed)`: `squeezed` is true when `r` had no room for two parts of at least
/// one pixel with the full gap between them; the gap is then made smaller (and, below 2 pixels, the second part is empty).
pub fn split(r: Rect, ratio: Ratio, gap: i32, axis: Axis) -> (Rect, Rect, bool) {
    let len = r.len(axis);
    let mut gap = gap.max(0);
    let mut squeezed = false;
    if len - gap < 2 {
        squeezed = true;
        gap = (len - 2).max(0);
    }
    let usable = len - gap;
    let a = if usable >= 2 { ratio.of(usable).clamp(1, usable - 1) } else { usable };
    let b = (len - a - gap).max(0);
    match axis {
        Axis::X => (Rect::new(r.x, r.y, a, r.h), Rect::new(r.x + a + gap, r.y, b, r.h), squeezed),
        Axis::Y => (Rect::new(r.x, r.y, r.w, a), Rect::new(r.x, r.y + a + gap, r.w, b), squeezed),
    }
}

/// Cut `r` into `n` equal parts along `axis` with exactly `gap` pixels between neighbours; the pixels that do not divide evenly go
/// one each to the first parts. Pushes the parts to `out` in order and returns whether it was squeezed (see [`split`]).
pub fn stack(r: Rect, n: usize, gap: i32, axis: Axis, out: &mut Vec<Rect>) -> bool {
    if n == 0 {
        return false;
    }
    let n_i = n as i32;
    let len = r.len(axis);
    let mut gap = gap.max(0);
    let mut squeezed = false;
    if len - gap * (n_i - 1) < n_i {
        squeezed = true;
        gap = if n_i > 1 { ((len - n_i) / (n_i - 1)).max(0) } else { 0 };
    }
    let usable = (len - gap * (n_i - 1)).max(0);
    let (each, extra) = (usable / n_i, usable % n_i);
    let mut pos = match axis {
        Axis::X => r.x,
        Axis::Y => r.y,
    };
    for i in 0..n_i {
        let l = each + if i < extra { 1 } else { 0 };
        out.push(match axis {
            Axis::X => Rect::new(pos, r.y, l, r.h),
            Axis::Y => Rect::new(r.x, pos, r.w, l),
        });
        pos += l + gap;
    }
    squeezed
}

/// The gap settings `arrange` needs (`general` in `gliese.conf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gaps {
    /// Pixels between two tiles.
    pub inner: i32,
    /// Pixels between the tiles and the edges of the usable area.
    pub outer: Sides,
    /// A workspace with one tiled window gives it the whole area: no outer gap (and, for the compositor, no border).
    pub smart: bool,
}

impl Default for Gaps {
    fn default() -> Gaps {
        Gaps { inner: 6, outer: Sides::all(12), smart: false }
    }
}

/// What a layout is told by a dispatcher (`layoutmsg`, `togglesplit`, …). A layout ignores what it does not understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMsg {
    /// dwindle: flip the split the focused window is in.
    ToggleSplit,
    /// dwindle: make the focused window's split the other way round (first and second swap).
    SwapSplit,
    /// master: swap the focused window with the first master (or, if it is that one, with the first of the stack).
    SwapWithMaster,
    /// master: one more master window.
    AddMaster,
    /// master: one master window fewer (never below 1).
    RemoveMaster,
    /// master: change the master area's share by this many basis points.
    MasterFactor(i32),
    /// master: where the master area goes.
    Orientation(Orientation),
}

/// Where the master area sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

/// The tiles of one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arranged {
    pub tiles: Vec<(WinId, Rect)>,
    /// Some split had no room for its full gap or for a tile of at least a pixel (see [`split`]).
    pub squeezed: bool,
}

/// A tiling layout: the state of one workspace's tiled windows.
pub trait Layout {
    /// The name `gliese.conf` uses (`layout = dwindle`).
    fn name(&self) -> &'static str;
    /// A new tiled window. `focused` is the window that had the focus (the one a new window usually goes next to); `area` is the
    /// area it will be arranged in (some layouts decide the split's direction from it).
    fn insert(&mut self, id: WinId, focused: Option<WinId>, area: Rect, gap: i32);
    /// A window went (closed, floated, moved to another workspace). Unknown ids are ignored.
    fn remove(&mut self, id: WinId);
    fn contains(&self, id: WinId) -> bool;
    /// The windows in the layout's own order (the order `cyclenext` follows).
    fn windows(&self) -> Vec<WinId>;
    /// Tile rectangles for every window, inside `area`, with exactly `gap` pixels between neighbours.
    fn arrange(&self, area: Rect, gap: i32, out: &mut Arranged);
    /// Swap two windows' places (`movewindow`). Returns whether both were there.
    fn swap(&mut self, a: WinId, b: WinId) -> bool;
    /// Grow (positive) or shrink window `id` by `dx`, `dy` pixels, by moving the dividers next to it (`resizeactive`).
    fn resize(&mut self, id: WinId, dx: i32, dy: i32, area: Rect, gap: i32);
    /// A layout message; returns whether the layout changed.
    fn message(&mut self, msg: LayoutMsg, focused: Option<WinId>) -> bool;
}

/// Settings the layouts read (`dwindle { }` and `master { }` in `gliese.conf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutSettings {
    pub dwindle_split_ratio: Ratio,
    /// 0: the new window goes second (right or below), 1: first (left or above), 2: second, always.
    pub dwindle_force_split: u8,
    /// Keep each split's direction once made (otherwise it follows the tile's shape at every arrangement).
    pub dwindle_preserve_split: bool,
    pub master_mfact: Ratio,
    pub master_nmaster: u8,
    pub master_orientation: Orientation,
    pub master_new_on_top: bool,
}

impl Default for LayoutSettings {
    fn default() -> LayoutSettings {
        LayoutSettings {
            dwindle_split_ratio: Ratio::HALF,
            dwindle_force_split: 0,
            dwindle_preserve_split: true,
            master_mfact: Ratio(5500),
            master_nmaster: 1,
            master_orientation: Orientation::Left,
            master_new_on_top: false,
        }
    }
}

/// The names `layout = …` accepts, in the order they are tried.
pub const LAYOUTS: &[&str] = &[
    #[cfg(feature = "dwindle")]
    "dwindle",
    #[cfg(feature = "master")]
    "master",
];

/// A new, empty layout by name (`None` for a name this build does not have).
pub fn by_name(name: &str, s: &LayoutSettings) -> Option<Box<dyn Layout>> {
    match name {
        #[cfg(feature = "dwindle")]
        "dwindle" => Some(Box::new(dwindle::Dwindle::new(s))),
        #[cfg(feature = "master")]
        "master" => Some(Box::new(master::Master::new(s))),
        _ => None,
    }
}

/// The usable area for the tiles of a workspace with `tiled` tiled windows: `area` (the screen less `reserved`) less the outer
/// gaps, or all of it with `smart_gaps` and one window.
pub fn tile_area(area: Rect, g: &Gaps, tiled: usize) -> Rect {
    if g.smart && tiled == 1 {
        area
    } else {
        area.shrink(g.outer)
    }
}

/// Arrange a workspace: the outer gaps, then the layout with the inner gap.
pub fn arrange(layout: &dyn Layout, area: Rect, g: &Gaps) -> Arranged {
    let n = layout.windows().len();
    let mut out = Arranged::default();
    layout.arrange(tile_area(area, g, n), g.inner, &mut out);
    out
}
