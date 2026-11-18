//! The seam between the compositor and the window manager (`PLAN.md` §3.1): the compositor tells a [`Policy`] what happens, and
//! the policy acts through a [`Ctl`].
use crate::{Conf, Host, Program, Rect, State, WinId};
use alloc::string::String;
use alloc::vec::Vec;
use gliese_proto::{ev, state, want, Event};

/// What the policy may know about a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WinInfo {
    pub id: WinId,
    pub program: Program,
    /// The client's number for it.
    pub num: u8,
    pub parent: Option<WinId>,
    pub title: String,
    pub outer: Rect,
    pub border: i32,
    /// The size the client asked for when it made the window.
    pub requested: (u32, u32),
    /// Smallest and largest client size (0: no limit).
    pub min: (u32, u32),
    pub max: (u32, u32),
    /// It has drawn something (committed).
    pub mapped: bool,
    pub hidden: bool,
    pub state: u32,
}

/// What the window manager needs the desktop-role program (`gliesed`) to do: things that need the program manager or the file
/// system, which the compositor does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRequest {
    /// Start this program (or, if it runs, ask it for another window).
    Exec(String),
    /// Read the config file again and upload it.
    Reload,
    /// Switch the machine off.
    Exit,
}

/// The window manager. Every method has a default that does nothing, so a policy implements what it needs.
pub trait Policy {
    /// A window was made: place it (with [`Ctl::place`]) and decide the focus. A window the policy does not place gets its
    /// requested size, centred.
    fn created(&mut self, c: &mut Ctl<'_>, id: WinId);
    /// A window went (destroyed by its client, or its program ended).
    fn destroyed(&mut self, c: &mut Ctl<'_>, id: WinId);
    /// A key, before any program sees it; return true to keep it from the focused window. `mods`: `gliese_proto::keymap::mods`.
    fn key(&mut self, _c: &mut Ctl<'_>, _code: u16, _down: bool, _repeat: bool, _mods: u32) -> bool {
        false
    }
    /// A pointer button, before the window under the pointer sees it; return true to keep it.
    fn button(&mut self, _c: &mut Ctl<'_>, _button: u8, _down: bool, _x: i32, _y: i32, _mods: u32) -> bool {
        false
    }
    /// The pointer moved; return true to keep it from the windows (while the policy drags something).
    fn motion(&mut self, _c: &mut Ctl<'_>, _x: i32, _y: i32) -> bool {
        false
    }
    /// A window would like something (`gliese_proto::want`).
    fn wants(&mut self, _c: &mut Ctl<'_>, _id: WinId, _what: u32) {}
    fn title_changed(&mut self, _c: &mut Ctl<'_>, _id: WinId) {}
    fn tick(&mut self, _c: &mut Ctl<'_>, _now_ms: u64) {}
    /// A new configuration, as text (`file` names it in messages). On an error the policy keeps what it had and returns the
    /// first error's (line, column, message); it shows the error bar itself.
    fn config_text(&mut self, _c: &mut Ctl<'_>, _file: &str, _text: &str) -> Result<(), (u32, u32, String)> {
        Ok(())
    }
    /// What the desktop-role program has to do, taken (the compositor passes it on).
    fn take_agent_requests(&mut self) -> Vec<AgentRequest> {
        Vec::new()
    }
    /// The desktop role told the compositor a program's id (for rules that match on it).
    fn program_named(&mut self, _program: Program, _name: &str) {}
}

/// The policy's handle on the compositor.
pub struct Ctl<'a> {
    pub(crate) st: &'a mut State,
    pub(crate) host: &'a mut dyn Host,
}

impl Ctl<'_> {
    pub fn screen(&self) -> Rect {
        Rect::new(0, 0, self.st.screen.0, self.st.screen.1)
    }
    pub fn now_ms(&self) -> u64 {
        self.host.now_ms()
    }
    pub fn pointer(&self) -> (i32, i32) {
        self.st.pointer
    }
    pub fn focused(&self) -> Option<WinId> {
        self.st.focus
    }
    pub fn window_at(&self, x: i32, y: i32) -> Option<WinId> {
        self.st.window_at(x, y)
    }

    pub fn info(&self, id: WinId) -> Option<WinInfo> {
        let w = self.st.win(id)?;
        Some(WinInfo {
            id: w.id,
            program: self.st.sessions.get(w.sess).and_then(|s| s.as_ref()).map_or(0, |s| s.program),
            num: w.num,
            parent: w.parent,
            title: w.title.clone(),
            outer: w.outer,
            border: w.border,
            requested: w.requested,
            min: w.min,
            max: w.max,
            mapped: w.mapped,
            hidden: w.hidden,
            state: w.state,
        })
    }

    /// Every window, bottom to top.
    pub fn windows(&self) -> Vec<WinId> {
        self.st.windows.iter().map(|w| w.id).collect()
    }

    /// Put a window at `outer` (border included) with a `border` of that many pixels and these state bits. The client is asked
    /// for the new size (a CONFIGURE) only when its size or state changes; a move alone is immediate.
    pub fn place(&mut self, id: WinId, outer: Rect, border: i32, st: u32) {
        let Some(w) = self.st.win_mut(id) else { return };
        let border = border.clamp(0, (outer.w.min(outer.h) / 2 - 1).max(0));
        let old = w.outer;
        let visible = w.visible();
        w.outer = outer;
        w.border = border;
        let client = w.client_rect();
        let clamp = |v: i32, lo: u32, hi: u32| {
            let v = v.max(1) as u32;
            let v = if hi > 0 { v.min(hi) } else { v };
            // a minimum larger than the tile wins: the client keeps it and is drawn clipped (PLAN.md §4.2)
            v.max(lo).clamp(1, gliese_proto::MAX_SIDE)
        };
        let (cw, ch) = (clamp(client.w, w.min.0, w.max.0), clamp(client.h, w.min.1, w.max.1));
        let current = w.conf.map(|c| (c.w, c.h, c.state));
        if visible && old != outer {
            self.st.damage.add(old);
            self.st.damage.add(outer);
        }
        if current != Some((cw, ch, st)) {
            let serial = self.st.next_serial();
            let w = self.st.win_mut(id).expect("still there");
            w.conf = Some(Conf { serial, w: cw, h: ch, state: st });
            w.state = st;
            self.st.write_table(self.host, id);
            self.st.send_to(self.host, id, Event { kind: ev::CONFIGURE, serial, a: cw, b: ch, c: st, ..Default::default() });
        }
    }

    pub fn set_hidden(&mut self, id: WinId, hidden: bool) {
        let Some(w) = self.st.win_mut(id) else { return };
        if w.hidden != hidden {
            w.hidden = hidden;
            let o = w.outer;
            self.st.damage.add(o);
        }
    }

    /// Give the keyboard to a window (or to nobody). The windows hear about it (FOCUS), and their borders change colour.
    pub fn focus(&mut self, id: Option<WinId>) {
        let id = id.filter(|i| self.st.win(*i).is_some());
        let old = self.st.focus;
        if old == id {
            return;
        }
        self.st.focus = id;
        for (w, on) in [(old, 0), (id, 1)] {
            if let Some(w) = w {
                if let Some(o) = self.st.win(w).map(|x| x.outer) {
                    self.st.damage.add(o);
                }
                self.st.send_to(self.host, w, Event { kind: ev::FOCUS, a: on, ..Default::default() });
            }
        }
    }

    /// Put a window (and its dialogs) on top.
    pub fn raise(&mut self, id: WinId) {
        let Some(i) = self.st.win_index(id) else { return };
        let w = self.st.windows.remove(i);
        let o = w.outer;
        self.st.windows.push(w);
        // its dialogs stay above it
        let kids: Vec<WinId> = self.st.windows.iter().filter(|c| c.parent == Some(id)).map(|c| c.id).collect();
        for k in kids {
            self.raise(k);
        }
        self.st.damage.add(o);
    }

    /// Ask a window's program to close it (CLOSE_REQUEST); the program decides.
    pub fn close(&mut self, id: WinId) {
        self.st.send_to(self.host, id, Event { kind: ev::CLOSE_REQUEST, ..Default::default() });
    }

    /// The pointer moved in the policy's name (a drag): redraw the cursor.
    pub fn damage(&mut self, r: Rect) {
        self.st.damage.add(r);
    }

    /// Show the error bar with this text, or take it away.
    pub fn set_error(&mut self, text: Option<&str>) {
        let new = text.map(String::from);
        if self.st.error != new {
            self.st.error = new;
            let bar = crate::render::error_bar(self.st.screen.0);
            self.st.damage.add(bar);
        }
    }

    pub fn set_look(&mut self, l: crate::Look) {
        if self.st.look != l {
            self.st.look = l;
            let s = self.screen();
            self.st.damage.add(s);
        }
    }
}

/// A plain floating window manager: new windows at their requested size, centred (cascading), dialogs over their owner; click to
/// focus and raise; fullscreen on request. The compositor's own default, and the reference for the `Policy` contract in tests.
#[derive(Default)]
pub struct FloatingPolicy {
    placed: u32,
    /// Where a fullscreen window was before.
    restore: Vec<(WinId, Rect)>,
}

pub const FLOATING_BORDER: i32 = 2;

impl Policy for FloatingPolicy {
    fn created(&mut self, c: &mut Ctl<'_>, id: WinId) {
        let Some(i) = c.info(id) else { return };
        let (w, h) = (i.requested.0 as i32 + 2 * FLOATING_BORDER, i.requested.1 as i32 + 2 * FLOATING_BORDER);
        let s = c.screen();
        let around = i.parent.and_then(|p| c.info(p)).map_or(s, |p| p.outer);
        let step = if i.parent.is_some() { 0 } else { (self.placed % 8) as i32 * 24 };
        self.placed += 1;
        let (cx, cy) = around.center();
        let r = Rect::new(cx - w / 2 + step, cy - h / 2 + step, w, h);
        c.place(id, r, FLOATING_BORDER, 0);
        c.focus(Some(id));
        c.raise(id);
    }

    fn destroyed(&mut self, c: &mut Ctl<'_>, id: WinId) {
        self.restore.retain(|(w, _)| *w != id);
        if c.focused().is_none() {
            let top = c.windows().into_iter().rev().find(|w| c.info(*w).is_some_and(|i| i.mapped && !i.hidden));
            c.focus(top);
        }
    }

    fn button(&mut self, c: &mut Ctl<'_>, _button: u8, down: bool, x: i32, y: i32, _mods: u32) -> bool {
        if down {
            if let Some(w) = c.window_at(x, y) {
                c.focus(Some(w));
                c.raise(w);
            }
        }
        false
    }

    fn wants(&mut self, c: &mut Ctl<'_>, id: WinId, what: u32) {
        let Some(i) = c.info(id) else { return };
        match what {
            want::FULLSCREEN if i.state & state::FULLSCREEN == 0 => {
                self.restore.push((id, i.outer));
                let s = c.screen();
                c.place(id, s, 0, state::FULLSCREEN);
                c.raise(id);
            }
            want::NOT_FULLSCREEN if i.state & state::FULLSCREEN != 0 => {
                let back = self.restore.iter().position(|(w, _)| *w == id).map(|p| self.restore.remove(p).1);
                c.place(id, back.unwrap_or(i.outer), FLOATING_BORDER, 0);
            }
            _ => {}
        }
    }
}
