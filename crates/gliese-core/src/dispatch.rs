//! The dispatchers: what each [`Action`] does.
use crate::{AgentEvent, Gliese, WORKSPACES};
use alloc::vec::Vec;
use gliese_config::{Action, Dir, Ws};
use gliese_layout::Rect;
use gliese_server::{Ctl, WinId};

/// How far `movewindow` nudges a floating window, and how far a dialog-sized window must be from the edge.
const NUDGE: i32 = 40;

impl Gliese {
    pub(crate) fn run(&mut self, c: &mut Ctl<'_>, a: &Action) {
        let focused = c.focused();
        match a {
            Action::Exec(p) => self.agent.push(AgentEvent::Exec(p.clone())),
            Action::Reload => self.agent.push(AgentEvent::Reload),
            Action::Exit => self.agent.push(AgentEvent::Exit),
            Action::KillActive => {
                if let Some(f) = focused {
                    c.close(f);
                }
            }
            Action::ToggleFloating => {
                if let Some(f) = focused {
                    self.toggle_floating(c, f);
                }
            }
            Action::Fullscreen => {
                if let Some(f) = focused {
                    let n = self.win(f).map_or(self.active, |w| w.ws);
                    self.ws[n].fullscreen = if self.ws[n].fullscreen == Some(f) { None } else { Some(f) };
                    self.relayout(c);
                }
            }
            Action::Pin => {
                if let Some(w) = focused.and_then(|f| self.win_mut(f)) {
                    if w.floating {
                        w.pinned = !w.pinned;
                    }
                }
                self.relayout(c);
            }
            Action::CenterWindow => {
                let area = self.usable(c);
                if let Some(w) = focused.and_then(|f| self.win_mut(f)).filter(|w| w.floating) {
                    let (cx, cy) = area.center();
                    w.float_rect = Rect::new(cx - w.float_rect.w / 2, cy - w.float_rect.h / 2, w.float_rect.w, w.float_rect.h);
                }
                self.relayout(c);
            }
            Action::MoveFocus(d) => {
                if let Some(t) = self.neighbour(c, focused, *d, false) {
                    self.focus(c, Some(t));
                }
            }
            Action::MoveWindow(d) => self.move_window(c, focused, *d),
            Action::CycleNext { prev } => {
                let order = self.cycle_order();
                if order.is_empty() {
                    return;
                }
                let i = focused.and_then(|f| order.iter().position(|w| *w == f));
                let n = order.len();
                let next = match (i, prev) {
                    (None, _) => 0,
                    (Some(i), false) => (i + 1) % n,
                    (Some(i), true) => (i + n - 1) % n,
                };
                self.focus(c, Some(order[next]));
            }
            Action::FocusUrgent => {}
            Action::ResizeActive(dx, dy) => {
                if let Some(f) = focused {
                    self.resize_window(c, f, *dx, *dy);
                }
            }
            Action::Workspace(t) => {
                let n = self.target(*t);
                self.switch_to(c, n);
            }
            Action::MoveToWorkspace { ws, silent } => {
                let n = self.target(*ws);
                let Some(f) = focused else { return };
                let Some(w) = self.win(f).cloned() else { return };
                if n == w.ws {
                    return;
                }
                self.detach(f);
                self.attach(c, f, n, w.floating);
                if *silent {
                    self.relayout(c);
                    let next = self.recent_on_active();
                    self.focus(c, next);
                } else {
                    self.switch_to(c, n);
                    self.focus(c, Some(f));
                }
            }
            Action::LayoutMsg(m) => {
                let n = self.active;
                if self.ws[n].layout.message(*m, focused) {
                    self.relayout(c);
                }
            }
            Action::Layout(name) => {
                let n = self.active;
                self.rebuild_layout(n, name);
                self.relayout(c);
            }
            Action::SetGaps { inner, outer } => {
                self.gaps_in = inner.apply(self.gaps_in);
                let o = &mut self.gaps_out;
                o.top = outer.apply(o.top);
                o.right = outer.apply(o.right);
                o.bottom = outer.apply(o.bottom);
                o.left = outer.apply(o.left);
                self.relayout(c);
            }
            Action::Submap(name) => self.submap = if name == "reset" { None } else { Some(name.clone()) },
            // (bindm only: started by a button, see mouse.rs)
            Action::MouseMove | Action::MouseResize => {}
        }
    }

    /// Workspace index (0-based) for a target.
    fn target(&self, t: Ws) -> usize {
        match t {
            Ws::Number(n) => n as usize - 1,
            Ws::Next => (self.active + 1) % WORKSPACES,
            Ws::Prev => (self.active + WORKSPACES - 1) % WORKSPACES,
            Ws::Previous => self.previous,
            Ws::Empty => (0..WORKSPACES).find(|n| !self.wins.iter().any(|w| w.ws == *n)).unwrap_or(self.active),
        }
    }

    /// The windows shown on the active workspace in cycling order: the tiles in the layout's order, then the floating ones.
    fn cycle_order(&self) -> Vec<WinId> {
        let mut v = self.ws[self.active].layout.windows();
        v.extend(self.wins.iter().filter(|w| w.floating && (w.ws == self.active || w.pinned)).map(|w| w.id));
        v
    }

    /// The nearest shown window in a direction from `from` (by geometry, so every layout gets it): one that overlaps it across the
    /// direction first, then the closest edge. `tiled_only`: among the tiles.
    pub(crate) fn neighbour(&self, c: &Ctl<'_>, from: Option<WinId>, d: Dir, tiled_only: bool) -> Option<WinId> {
        let f = c.info(from?)?.outer;
        let candidates = self.cycle_order().into_iter().filter(|id| Some(*id) != from && (!tiled_only || !self.is_floating(*id)));
        let mut best: Option<((bool, i32, i32), WinId)> = None;
        for id in candidates {
            let Some(r) = c.info(id).map(|i| i.outer) else { continue };
            let (fc, rc) = (f.center(), r.center());
            // in that direction at all, and how far (edge to edge), and how far off the line
            let (ahead, dist, off, overlap) = match d {
                Dir::Left => (rc.0 < fc.0, f.x - r.right(), (rc.1 - fc.1).abs(), r.y < f.bottom() && f.y < r.bottom()),
                Dir::Right => (rc.0 > fc.0, r.x - f.right(), (rc.1 - fc.1).abs(), r.y < f.bottom() && f.y < r.bottom()),
                Dir::Up => (rc.1 < fc.1, f.y - r.bottom(), (rc.0 - fc.0).abs(), r.x < f.right() && f.x < r.right()),
                Dir::Down => (rc.1 > fc.1, r.y - f.bottom(), (rc.0 - fc.0).abs(), r.x < f.right() && f.x < r.right()),
            };
            if !ahead {
                continue;
            }
            let key = (!overlap, dist.max(0), off);
            if best.as_ref().is_none_or(|(k, _)| key < *k) {
                best = Some((key, id));
            }
        }
        best.map(|(_, id)| id)
    }

    fn move_window(&mut self, c: &mut Ctl<'_>, focused: Option<WinId>, d: Dir) {
        let Some(f) = focused else { return };
        if self.is_floating(f) {
            let (dx, dy) = match d {
                Dir::Left => (-NUDGE, 0),
                Dir::Right => (NUDGE, 0),
                Dir::Up => (0, -NUDGE),
                Dir::Down => (0, NUDGE),
            };
            if let Some(w) = self.win_mut(f) {
                w.float_rect = Rect::new(w.float_rect.x + dx, w.float_rect.y + dy, w.float_rect.w, w.float_rect.h);
            }
            self.relayout(c);
            return;
        }
        if let Some(t) = self.neighbour(c, Some(f), d, true) {
            let n = self.active;
            self.ws[n].layout.swap(f, t);
            self.relayout(c);
        }
    }

    pub(crate) fn resize_window(&mut self, c: &mut Ctl<'_>, id: WinId, dx: i32, dy: i32) {
        if self.is_floating(id) {
            if let Some(w) = self.win_mut(id) {
                let r = w.float_rect;
                w.float_rect = Rect::new(r.x, r.y, (r.w + dx).max(32), (r.h + dy).max(32));
            }
        } else {
            let n = self.active;
            let area = self.usable(c);
            let g = self.gaps(n);
            let a = gliese_layout::tile_area(area, &g, self.ws[n].layout.windows().len());
            self.ws[n].layout.resize(id, dx, dy, a, g.inner);
        }
        self.relayout(c);
    }

    pub(crate) fn toggle_floating(&mut self, c: &mut Ctl<'_>, id: WinId) {
        let Some(w) = self.win(id).cloned() else { return };
        self.detach(id);
        if !w.floating {
            // it floats where it was, a little smaller, so it is clearly not a tile any more
            if let Some(o) = c.info(id).map(|i| i.outer) {
                let r = Rect::new(o.x + o.w / 10, o.y + o.h / 10, o.w * 8 / 10, o.h * 8 / 10);
                if let Some(x) = self.win_mut(id) {
                    x.float_rect = r;
                }
            }
        }
        self.attach(c, id, w.ws, !w.floating);
        if let Some(x) = self.win_mut(id) {
            x.pinned = x.pinned && x.floating;
        }
        self.relayout(c);
    }
}
