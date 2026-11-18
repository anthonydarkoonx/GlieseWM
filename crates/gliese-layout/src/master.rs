//! The master layout: the first `nmaster` windows share the master area, the others are stacked beside it.
use crate::{split, stack, Arranged, Axis, Layout, LayoutMsg, LayoutSettings, Orientation, Ratio, Rect, WinId};
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct Master {
    /// Masters first, then the stack.
    order: Vec<WinId>,
    nmaster: usize,
    mfact: Ratio,
    orientation: Orientation,
    new_on_top: bool,
}

impl Master {
    pub fn new(s: &LayoutSettings) -> Master {
        Master {
            order: Vec::new(),
            nmaster: (s.master_nmaster as usize).max(1),
            mfact: s.master_mfact,
            orientation: s.master_orientation,
            new_on_top: s.master_new_on_top,
        }
    }

    fn masters(&self) -> usize {
        self.nmaster.min(self.order.len())
    }

    /// (master area, stack area(s)) for `n` windows; the centre orientation has two stack columns.
    fn areas(&self, area: Rect, gap: i32, out: &mut Arranged) -> (Rect, Rect, Option<Rect>) {
        let stacked = self.order.len() - self.masters();
        if stacked == 0 {
            return (area, Rect::default(), None);
        }
        match self.orientation {
            Orientation::Left => {
                let (m, s, sq) = split(area, self.mfact, gap, Axis::X);
                out.squeezed |= sq;
                (m, s, None)
            }
            Orientation::Right => {
                let (s, m, sq) = split(area, Ratio(Ratio::ONE - self.mfact.0), gap, Axis::X);
                out.squeezed |= sq;
                (m, s, None)
            }
            Orientation::Top => {
                let (m, s, sq) = split(area, self.mfact, gap, Axis::Y);
                out.squeezed |= sq;
                (m, s, None)
            }
            Orientation::Bottom => {
                let (s, m, sq) = split(area, Ratio(Ratio::ONE - self.mfact.0), gap, Axis::Y);
                out.squeezed |= sq;
                (m, s, None)
            }
            Orientation::Center if stacked == 1 => {
                // one stack window: master on the left of it, as `left` (a lone side column would look lost)
                let (m, s, sq) = split(area, self.mfact, gap, Axis::X);
                out.squeezed |= sq;
                (m, s, None)
            }
            Orientation::Center => {
                // left stack | master | right stack: the master gets mfact, the sides share the rest
                let (left, rest, sq1) = split(area, Ratio((Ratio::ONE - self.mfact.0) / 2), gap, Axis::X);
                let share = Ratio::ONE as i64 * self.mfact.0 as i64 / (self.mfact.0 as i64 + (Ratio::ONE - self.mfact.0) as i64 / 2);
                let (m, right, sq2) = split(rest, Ratio(share.clamp(0, Ratio::ONE as i64) as u16), gap, Axis::X);
                out.squeezed |= sq1 | sq2;
                (m, right, Some(left))
            }
        }
    }

    /// The axis the master and stack columns stack their windows along.
    fn stacking_axis(&self) -> Axis {
        match self.orientation {
            Orientation::Top | Orientation::Bottom => Axis::X,
            _ => Axis::Y,
        }
    }
}

impl Layout for Master {
    fn name(&self) -> &'static str {
        "master"
    }

    fn insert(&mut self, id: WinId, _focused: Option<WinId>, _area: Rect, _gap: i32) {
        if self.contains(id) {
            return;
        }
        if self.new_on_top {
            self.order.insert(0, id);
        } else {
            self.order.push(id);
        }
    }

    fn remove(&mut self, id: WinId) {
        self.order.retain(|w| *w != id);
    }

    fn contains(&self, id: WinId) -> bool {
        self.order.contains(&id)
    }

    fn windows(&self) -> Vec<WinId> {
        self.order.clone()
    }

    fn arrange(&self, area: Rect, gap: i32, out: &mut Arranged) {
        if self.order.is_empty() {
            return;
        }
        let (m_area, s_area, left) = self.areas(area, gap, out);
        let ax = self.stacking_axis();
        let nm = self.masters();
        let mut rects = Vec::new();
        out.squeezed |= stack(m_area, nm, gap, ax, &mut rects);
        let rest = &self.order[nm..];
        match left {
            None => {
                out.squeezed |= stack(s_area, rest.len(), gap, ax, &mut rects);
            }
            Some(left) => {
                // alternate: the first stack window goes right, the next left, …
                let right_n = rest.len().div_ceil(2);
                let (mut r, mut l) = (Vec::new(), Vec::new());
                out.squeezed |= stack(s_area, right_n, gap, ax, &mut r);
                out.squeezed |= stack(left, rest.len() - right_n, gap, ax, &mut l);
                let (mut ri, mut li) = (r.into_iter(), l.into_iter());
                for i in 0..rest.len() {
                    rects.push(if i % 2 == 0 { ri.next() } else { li.next() }.expect("one rectangle a window"));
                }
            }
        }
        out.tiles.extend(self.order.iter().copied().zip(rects));
    }

    fn swap(&mut self, a: WinId, b: WinId) -> bool {
        let (Some(i), Some(j)) = (self.order.iter().position(|w| *w == a), self.order.iter().position(|w| *w == b)) else { return false };
        self.order.swap(i, j);
        true
    }

    fn resize(&mut self, id: WinId, dx: i32, dy: i32, area: Rect, gap: i32) {
        let Some(i) = self.order.iter().position(|w| *w == id) else { return };
        let in_master = i < self.masters();
        let (delta, along) = match self.orientation {
            Orientation::Top | Orientation::Bottom => (dy, area.h),
            _ => (dx, area.w),
        };
        if delta == 0 || self.order.len() <= self.masters() {
            return;
        }
        let usable = (along - gap).max(1);
        let d = (delta as i64 * Ratio::ONE as i64 / usable as i64) as i32;
        // a master grows by growing mfact; a stack window by shrinking it (centre: both sides count)
        let d = if self.orientation == Orientation::Center && !in_master { d * 2 } else { d };
        self.mfact = self.mfact.plus(if in_master { d } else { -d });
    }

    fn message(&mut self, msg: LayoutMsg, focused: Option<WinId>) -> bool {
        match msg {
            LayoutMsg::SwapWithMaster => {
                let Some(f) = focused.and_then(|f| self.order.iter().position(|w| *w == f)) else { return false };
                let target = if f == 0 { 1 } else { 0 };
                if target >= self.order.len() {
                    return false;
                }
                self.order.swap(f, target);
                true
            }
            LayoutMsg::AddMaster => {
                self.nmaster += 1;
                true
            }
            LayoutMsg::RemoveMaster if self.nmaster > 1 => {
                self.nmaster -= 1;
                true
            }
            LayoutMsg::MasterFactor(d) => {
                self.mfact = self.mfact.plus(d);
                true
            }
            LayoutMsg::Orientation(o) => {
                self.orientation = o;
                true
            }
            _ => false,
        }
    }
}
