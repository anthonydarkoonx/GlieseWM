//! Input routing: the attention keys first, then the policy (binds, drags), then the focused window (keys) or the window under the
//! pointer (pointer events, with an implicit grab while a button is held).
use crate::{Host, Server, WinId};
use gliese_proto::keymap::{self, mods};
use gliese_proto::{ev, Event};

/// An input event, the pointer already in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Key { code: u16, down: bool, repeat: bool },
    Motion { x: i32, y: i32 },
    /// 1 left, 2 right, 3 middle.
    Button { button: u8, down: bool },
    Wheel { clicks: i32 },
}

const KEY_ESC: u16 = 1;
const KEY_DELETE: u16 = 111;

impl Server {
    pub fn input(&mut self, host: &mut dyn Host, e: Input) {
        match e {
            Input::Key { code, down, repeat } => self.key(host, code, down, repeat),
            Input::Motion { x, y } => self.motion(host, x, y),
            Input::Button { button, down } => self.button(host, button, down),
            Input::Wheel { clicks } => {
                let (x, y) = self.st.pointer;
                if let Some(id) = self.st.grab.or_else(|| self.st.window_at(x, y)) {
                    self.st.send_to(host, id, Event { kind: ev::POINTER_WHEEL, a: clicks as u32, ..Default::default() });
                }
            }
        }
        self.st.flush_wakes(host);
    }

    fn key(&mut self, host: &mut dyn Host, code: u16, down: bool, repeat: bool) {
        if let Some(m) = keymap::modifier_of(code) {
            if down {
                self.st.mods |= m;
            } else {
                self.st.mods &= !m;
            }
        }
        if code == keymap::CAPS_LOCK && down && !repeat {
            self.st.mods ^= mods::CAPS;
        }
        let m = self.st.mods;
        // the attention keys: the system's, never a program's or a bind's
        let attention = (code == KEY_DELETE && m & mods::CTRL != 0 && m & mods::ALT != 0) || (code == KEY_ESC && m & mods::CTRL != 0 && m & mods::SHIFT != 0);
        if attention {
            if down && !repeat {
                host.attention();
            }
            return;
        }
        let bound = self.with_policy(host, |p, c| p.key(c, code, down, repeat, m & !mods::CAPS));
        if bound {
            return;
        }
        let Some(id) = self.st.focus else { return };
        let ch = if down { keymap::char_for(code, m).map_or(0, |c| c as u32) } else { 0 };
        let st = if !down { 0 } else if repeat { 2 } else { 1 };
        self.st.send_to(host, id, Event { kind: ev::KEY, a: code as u32, b: st, c: m, d: ch, ..Default::default() });
    }

    fn motion(&mut self, host: &mut dyn Host, x: i32, y: i32) {
        let (x, y) = (x.clamp(0, self.st.screen.0 - 1), y.clamp(0, self.st.screen.1 - 1));
        self.st.pointer = (x, y);
        if self.with_policy(host, |p, c| p.motion(c, x, y)) {
            return;
        }
        let target = self.st.grab.or_else(|| self.st.window_at(x, y));
        self.hover(host, target);
        if let Some(id) = target {
            let (cx, cy) = self.client_point(id, x, y);
            self.st.send_to(host, id, Event { kind: ev::POINTER_MOTION, a: cx as u32, b: cy as u32, ..Default::default() });
        }
    }

    /// Enter and leave events when the window under the pointer changes.
    fn hover(&mut self, host: &mut dyn Host, target: Option<WinId>) {
        if self.st.pointer_over == target {
            return;
        }
        if let Some(old) = self.st.pointer_over {
            self.st.send_to(host, old, Event { kind: ev::POINTER_LEAVE, ..Default::default() });
        }
        self.st.pointer_over = target;
        if let Some(id) = target {
            let (x, y) = self.st.pointer;
            let (cx, cy) = self.client_point(id, x, y);
            self.st.send_to(host, id, Event { kind: ev::POINTER_ENTER, a: cx as u32, b: cy as u32, ..Default::default() });
        }
    }

    /// A screen point in a window's client area (it may be outside it: on the border, or while grabbed).
    fn client_point(&self, id: WinId, x: i32, y: i32) -> (i32, i32) {
        self.st.win(id).map_or((0, 0), |w| {
            let c = w.client_rect();
            (x - c.x, y - c.y)
        })
    }

    fn button(&mut self, host: &mut dyn Host, button: u8, down: bool) {
        let (x, y) = self.st.pointer;
        let bit = 1u8.checked_shl(button as u32 - 1).unwrap_or(0);
        let m = self.st.mods & !mods::CAPS;
        if self.with_policy(host, |p, c| p.button(c, button, down, x, y, m)) {
            if !down {
                self.st.buttons &= !bit;
            }
            return;
        }
        let target = self.st.grab.or_else(|| self.st.window_at(x, y));
        if down {
            self.st.buttons |= bit;
            if self.st.grab.is_none() {
                self.st.grab = target;
            }
        } else {
            self.st.buttons &= !bit;
        }
        if let Some(id) = target {
            let (cx, cy) = self.client_point(id, x, y);
            self.st.send_to(host, id, Event { kind: ev::POINTER_BUTTON, a: button as u32, b: down as u32, c: cx as u32, d: cy as u32, ..Default::default() });
        }
        if self.st.buttons == 0 && self.st.grab.take().is_some() {
            // the grab ends: the pointer may be over another window now
            let now = self.st.window_at(x, y);
            self.hover(host, now);
        }
    }
}
