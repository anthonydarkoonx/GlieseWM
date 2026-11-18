//! The pointer: `bindm` drags (move and resize), click binds, click to focus, focus follows the pointer.
use crate::{Drag, DragKind, Gliese};
use gliese_config::{Action, Key};
use gliese_layout::Rect;
use gliese_server::Ctl;

impl Gliese {
    pub(crate) fn mouse_button(&mut self, c: &mut Ctl<'_>, button: u8, down: bool, x: i32, y: i32, mods: u8) -> bool {
        if !down {
            if self.drag.as_ref().is_some_and(|d| d.button == button) {
                let d = self.drag.take().expect("there");
                if let DragKind::Move = d.kind {
                    // a tile dropped on another tile: they swap places
                    if !self.is_floating(d.id) {
                        if let Some(t) = c.window_at(x, y).filter(|t| *t != d.id && !self.is_floating(*t)) {
                            let n = self.active;
                            self.ws[n].layout.swap(d.id, t);
                            self.relayout(c);
                        }
                    }
                }
                return true;
            }
            return false;
        }
        let submap = self.submap.clone();
        let under = c.window_at(x, y);
        // a drag
        let drag = self.cfg.binds.iter().find(|b| b.flags.mouse && b.submap == submap && b.mods == mods && b.key == Key::Mouse(button)).map(|b| b.action.clone());
        if let (Some(a), Some(id)) = (drag, under) {
            if self.ws[self.active].fullscreen != Some(id) {
                let rect = self.win(id).map_or(Rect::default(), |w| w.float_rect);
                let kind = if a == Action::MouseResize { DragKind::Resize } else { DragKind::Move };
                self.focus(c, Some(id));
                self.drag = Some(Drag { id, kind, button, start: (x, y), last: (x, y), rect });
                return true;
            }
        }
        // a click bind
        if let Some(b) = self.cfg.bind_for(submap.as_deref(), mods, Key::Mouse(button), false).cloned() {
            self.run(c, &b.action);
            return true;
        }
        // click to focus (the click itself goes on to the window)
        if under.is_some() && under != c.focused() {
            self.focus(c, under);
        }
        false
    }

    pub(crate) fn mouse_motion(&mut self, c: &mut Ctl<'_>, x: i32, y: i32) -> bool {
        if let Some(d) = self.drag.as_mut() {
            let (dx, dy) = (x - d.start.0, y - d.start.1);
            let (ix, iy) = (x - d.last.0, y - d.last.1);
            d.last = (x, y);
            let (id, start, resize) = (d.id, d.rect, matches!(d.kind, DragKind::Resize));
            let floating = self.is_floating(id);
            match (resize, floating) {
                (false, true) => {
                    if let Some(w) = self.win_mut(id) {
                        w.float_rect = Rect::new(start.x + dx, start.y + dy, start.w, start.h);
                    }
                    self.relayout(c);
                }
                (true, true) => {
                    if let Some(w) = self.win_mut(id) {
                        w.float_rect = Rect::new(start.x, start.y, (start.w + dx).max(32), (start.h + dy).max(32));
                    }
                    self.relayout(c);
                }
                // a tile resizes by moving its dividers, a little at a time
                (true, false) => self.resize_window(c, id, ix, iy),
                // a tile being moved stays put until it is dropped (on another tile, to swap)
                (false, false) => {}
            }
            // (the pointer itself moved: the compositor redraws it)
            return true;
        }
        if self.cfg.input.follow_mouse {
            if let Some(w) = c.window_at(x, y).filter(|w| Some(*w) != c.focused()) {
                c.focus(Some(w));
                self.mru.retain(|m| *m != w);
                self.mru.insert(0, w);
            }
        }
        false
    }
}
