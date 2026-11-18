//! GlieseWM's window manager (`PLAN.md` §5-§6): a [`gliese_server::Policy`].
//!
//! Ten workspaces, each with its own layout (`gliese-layout`); tiled windows are arranged with the configured gaps, floating ones
//! keep their own rectangles above them, a fullscreen one covers the screen. Keys and buttons go through the binds of
//! `gliese.conf` (`gliese-config`) first, in the current submap; a bind runs a dispatcher ([`Action`]).
//!
//! What needs the program manager (starting a program, reading the config file, switching the machine off) is queued as an
//! [`AgentEvent`] for the desktop-role program, `gliesed`, which has the rights for it.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

mod dispatch;
mod mouse;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use gliese_config::Action;
use gliese_config::{default_config, keys, Config, Key, RuleAction};
use gliese_layout::{arrange, Gaps, Layout, Rect, Sides};
use gliese_proto::{keymap, state, want};
use gliese_server::{Ctl, Look, Policy, Program, WinId};

pub const WORKSPACES: usize = 10;

pub use gliese_server::AgentRequest as AgentEvent;

pub(crate) struct Workspace {
    pub layout: Box<dyn Layout>,
    pub layout_name: String,
    pub fullscreen: Option<WinId>,
}

#[derive(Clone, Debug)]
pub(crate) struct Win {
    pub id: WinId,
    /// 0..WORKSPACES.
    pub ws: usize,
    pub floating: bool,
    pub pinned: bool,
    /// Where it goes when floating.
    pub float_rect: Rect,
    /// Rules were applied again once it had a title.
    pub titled: bool,
}

pub(crate) enum DragKind {
    Move,
    Resize,
}

pub(crate) struct Drag {
    pub id: WinId,
    pub kind: DragKind,
    pub button: u8,
    pub start: (i32, i32),
    pub last: (i32, i32),
    pub rect: Rect,
}

pub struct Gliese {
    pub(crate) cfg: Config,
    /// The gaps now (`setgaps` changes them until the next config).
    pub(crate) gaps_in: i32,
    pub(crate) gaps_out: Sides,
    pub(crate) ws: Vec<Workspace>,
    pub(crate) active: usize,
    pub(crate) previous: usize,
    pub(crate) wins: Vec<Win>,
    /// Most recently focused first.
    pub(crate) mru: Vec<WinId>,
    pub(crate) submap: Option<String>,
    /// Keys whose press ran a bind: their release is kept from the window too.
    pub(crate) held: Vec<u16>,
    pub(crate) drag: Option<Drag>,
    pub(crate) agent: Vec<AgentEvent>,
    /// Program ids by program (from the desktop role; used by `program:` rules).
    pub(crate) names: Vec<(Program, String)>,
    /// Workspaces have been arranged at least once (the first call with a `Ctl` sets the look).
    pub(crate) started: bool,
}

impl Gliese {
    pub fn new(cfg: Config) -> Gliese {
        let ws = (0..WORKSPACES).map(|n| Gliese::make_workspace(&cfg, n)).collect();
        Gliese {
            gaps_in: cfg.general.gaps_in,
            gaps_out: cfg.general.gaps_out,
            cfg,
            ws,
            active: 0,
            previous: 0,
            wins: Vec::new(),
            mru: Vec::new(),
            submap: None,
            held: Vec::new(),
            drag: None,
            agent: Vec::new(),
            names: Vec::new(),
            started: false,
        }
    }

    /// With the built-in default config.
    pub fn with_default() -> Gliese {
        Gliese::new(default_config())
    }

    fn layout_name_for(cfg: &Config, n: usize) -> String {
        cfg.workspace(n as u8 + 1).and_then(|w| w.layout.clone()).unwrap_or_else(|| cfg.general.layout.clone())
    }

    fn make_workspace(cfg: &Config, n: usize) -> Workspace {
        let name = Gliese::layout_name_for(cfg, n);
        let layout = gliese_layout::by_name(&name, &cfg.layouts).or_else(|| gliese_layout::by_name(gliese_layout::LAYOUTS[0], &cfg.layouts)).expect("a layout is built in");
        Workspace { layout, layout_name: name, fullscreen: None }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }
    /// The workspace shown (1..=10).
    pub fn active_workspace(&self) -> u8 {
        self.active as u8 + 1
    }
    /// The workspace a window is on (1..=10).
    pub fn workspace_of(&self, id: WinId) -> Option<u8> {
        self.win(id).map(|w| w.ws as u8 + 1)
    }
    pub fn is_floating(&self, id: WinId) -> bool {
        self.win(id).is_some_and(|w| w.floating)
    }
    pub fn submap(&self) -> Option<&str> {
        self.submap.as_deref()
    }
    /// What the desktop-role program has to do, taken (it is told with an event; `gliesed` collects these).
    pub fn take_agent_events(&mut self) -> Vec<AgentEvent> {
        core::mem::take(&mut self.agent)
    }
    /// The id of a program (for `program:` rules), as the desktop role reports it.
    pub fn set_program_name(&mut self, program: Program, name: &str) {
        self.names.retain(|(p, _)| *p != program);
        self.names.push((program, name.to_string()));
    }

    pub(crate) fn win(&self, id: WinId) -> Option<&Win> {
        self.wins.iter().find(|w| w.id == id)
    }
    pub(crate) fn win_mut(&mut self, id: WinId) -> Option<&mut Win> {
        self.wins.iter_mut().find(|w| w.id == id)
    }

    /// Replace the configuration: the look, the gaps, and (when the layout or its settings changed) each workspace's layout, with
    /// its windows put back in the same order.
    pub fn set_config(&mut self, c: &mut Ctl<'_>, cfg: Config) {
        let rebuild = cfg.layouts != self.cfg.layouts;
        self.cfg = cfg;
        self.gaps_in = self.cfg.general.gaps_in;
        self.gaps_out = self.cfg.general.gaps_out;
        for n in 0..WORKSPACES {
            let name = Gliese::layout_name_for(&self.cfg, n);
            if rebuild || name != self.ws[n].layout_name {
                self.rebuild_layout(n, &name);
            }
        }
        if let Some(s) = &self.submap {
            if !self.cfg.binds.iter().any(|b| b.submap.as_ref() == Some(s)) {
                self.submap = None;
            }
        }
        self.apply_look(c);
        self.relayout(c);
    }

    pub(crate) fn rebuild_layout(&mut self, n: usize, name: &str) {
        let Some(mut layout) = gliese_layout::by_name(name, &self.cfg.layouts) else { return };
        let order = self.ws[n].layout.windows();
        let mut last = None;
        for id in order {
            layout.insert(id, last, Rect::default(), self.gaps_in);
            last = Some(id);
        }
        self.ws[n].layout = layout;
        self.ws[n].layout_name = name.into();
    }

    fn apply_look(&mut self, c: &mut Ctl<'_>) {
        let g = &self.cfg.general;
        c.set_look(Look { background: g.background & 0xFF_FFFF, active_border: g.active_border & 0xFF_FFFF, inactive_border: g.inactive_border & 0xFF_FFFF });
    }

    /// The area tiles go in: the screen less `reserved`.
    pub(crate) fn usable(&self, c: &Ctl<'_>) -> Rect {
        c.screen().shrink(self.cfg.general.reserved)
    }

    /// The gaps of workspace `n` (its overrides, else the global ones).
    pub(crate) fn gaps(&self, n: usize) -> Gaps {
        let o = self.cfg.workspace(n as u8 + 1);
        Gaps {
            inner: o.and_then(|w| w.gaps_in).unwrap_or(self.gaps_in),
            outer: o.and_then(|w| w.gaps_out).unwrap_or(self.gaps_out),
            smart: self.cfg.general.smart_gaps,
        }
    }

    /// Whether window `id` is shown: on the active workspace, or pinned.
    fn shown(&self, w: &Win) -> bool {
        w.ws == self.active || w.pinned
    }

    /// Put every window where it belongs: tiles of the active workspace arranged, floating windows at their rectangles above them,
    /// the fullscreen window over everything; windows of other workspaces hidden.
    pub(crate) fn relayout(&mut self, c: &mut Ctl<'_>) {
        if !self.started {
            self.started = true;
            self.apply_look(c);
        }
        let n = self.active;
        let area = self.usable(c);
        let gaps = self.gaps(n);
        let arranged = arrange(&*self.ws[n].layout, area, &gaps);
        let border = self.cfg.general.border_size;
        let lone = gaps.smart && arranged.tiles.len() == 1;
        let fullscreen = self.ws[n].fullscreen;
        for (id, r) in &arranged.tiles {
            if Some(*id) != fullscreen {
                c.place(*id, *r, if lone { 0 } else { border }, state::TILED);
            }
        }
        let wins = self.wins.clone();
        for w in &wins {
            c.set_hidden(w.id, !self.shown(w));
        }
        // floating windows above the tiles, the focused one above the other floating ones
        let mut floating: Vec<&Win> = wins.iter().filter(|w| w.floating && self.shown(w) && Some(w.id) != fullscreen).collect();
        floating.sort_by_key(|w| core::cmp::Reverse(self.mru.iter().position(|m| *m == w.id).unwrap_or(usize::MAX)));
        for w in floating {
            c.place(w.id, w.float_rect, border, 0);
            c.raise(w.id);
        }
        if let Some(f) = fullscreen {
            let s = c.screen();
            c.place(f, s, 0, state::FULLSCREEN);
            c.raise(f);
        }
    }

    /// Give the focus to a window (it becomes the most recent; a floating one comes to the top).
    pub(crate) fn focus(&mut self, c: &mut Ctl<'_>, id: Option<WinId>) {
        c.focus(id);
        if let Some(id) = id {
            self.mru.retain(|m| *m != id);
            self.mru.insert(0, id);
            if self.is_floating(id) && self.ws[self.active].fullscreen.is_none() {
                c.raise(id);
            }
        }
    }

    /// The most recently focused window shown on the active workspace.
    pub(crate) fn recent_on_active(&self) -> Option<WinId> {
        self.mru.iter().copied().find(|id| self.win(*id).is_some_and(|w| self.shown(w)))
    }

    /// What the rules say for a window with this title and program.
    fn rules_for(&self, title: &str, program: Program) -> Vec<RuleAction> {
        let name = self.names.iter().find(|(p, _)| *p == program).map_or("", |(_, n)| n.as_str());
        self.cfg.rules.iter().filter(|r| r.matches(title, name)).map(|r| r.action.clone()).collect()
    }

    /// A floating rectangle for a window of client size `size`, centred on `around`.
    fn centred(&self, around: Rect, size: (u32, u32)) -> Rect {
        let b = self.cfg.general.border_size;
        let (w, h) = (size.0 as i32 + 2 * b, size.1 as i32 + 2 * b);
        let (cx, cy) = around.center();
        Rect::new(cx - w / 2, cy - h / 2, w, h)
    }

    /// Take a window out of whatever holds it (its layout, or the floating set).
    pub(crate) fn detach(&mut self, id: WinId) {
        for ws in &mut self.ws {
            ws.layout.remove(id);
            if ws.fullscreen == Some(id) {
                ws.fullscreen = None;
            }
        }
    }

    /// Put a window into workspace `n`, tiled (if there is room for another tile) or floating.
    pub(crate) fn attach(&mut self, c: &mut Ctl<'_>, id: WinId, n: usize, floating: bool) {
        let mut floating = floating;
        if !floating {
            let focus = c.focused().filter(|f| self.ws[n].layout.contains(*f));
            let area = self.usable(c);
            let gaps = self.gaps(n);
            self.ws[n].layout.insert(id, focus, area, gaps.inner);
            // no room for a usable tile: it floats instead (PLAN.md §3.2)
            let a = arrange(&*self.ws[n].layout, area, &gaps);
            let min = self.cfg.misc.min_tile;
            if a.tiles.len() > 1 && (a.squeezed || a.tiles.iter().any(|(_, r)| r.w < min || r.h < min)) {
                self.ws[n].layout.remove(id);
                floating = true;
            }
        }
        if let Some(w) = self.win_mut(id) {
            w.ws = n;
            w.floating = floating;
        }
    }

    /// The rules again, once the window has its title: they may float it, or send it elsewhere.
    fn apply_rules(&mut self, c: &mut Ctl<'_>, id: WinId, first: bool) {
        let Some(info) = c.info(id) else { return };
        let actions = self.rules_for(&info.title, info.program);
        let Some(w) = self.win(id).cloned() else { return };
        let mut floating = w.floating;
        let mut ws = w.ws;
        let mut rect = w.float_rect;
        let mut fullscreen = false;
        let mut pinned = w.pinned;
        let mut silent = false;
        for a in &actions {
            match a {
                RuleAction::Float => floating = true,
                RuleAction::Tile => floating = false,
                RuleAction::Fullscreen => fullscreen = true,
                RuleAction::Pin => {
                    pinned = true;
                    floating = true;
                }
                RuleAction::Center => rect = self.centred(self.usable(c), (info.requested.0, info.requested.1)),
                RuleAction::Size(sw, sh) => {
                    let (cx, cy) = rect.center();
                    let b = self.cfg.general.border_size;
                    rect = Rect::new(cx - (sw + 2 * b) / 2, cy - (sh + 2 * b) / 2, sw + 2 * b, sh + 2 * b);
                }
                RuleAction::Workspace { n, silent: s } => {
                    ws = *n as usize - 1;
                    silent = *s;
                }
            }
        }
        if first && actions.is_empty() {
            return;
        }
        if floating != w.floating || ws != w.ws {
            self.detach(id);
            if let Some(x) = self.win_mut(id) {
                x.float_rect = rect;
            }
            self.attach(c, id, ws, floating);
        }
        if let Some(x) = self.win_mut(id) {
            x.float_rect = rect;
            x.pinned = pinned;
        }
        if fullscreen {
            self.ws[ws].fullscreen = Some(id);
        }
        if ws != self.active && !silent {
            self.switch_to(c, ws);
        }
    }

    /// Show workspace `n`.
    pub(crate) fn switch_to(&mut self, c: &mut Ctl<'_>, n: usize) {
        if n == self.active || n >= WORKSPACES {
            return;
        }
        self.previous = self.active;
        self.active = n;
        self.relayout(c);
        let f = self.recent_on_active();
        self.focus(c, f);
    }

    /// Parse and apply a config; on an error keep the old one and show the error bar.
    pub fn load(&mut self, c: &mut Ctl<'_>, file: &str, text: &str) -> Result<(), (u32, u32, String)> {
        match gliese_config::parse(file, text, None) {
            Ok(cfg) => {
                c.set_error(None);
                self.set_config(c, cfg);
                Ok(())
            }
            Err(errs) => {
                let e = &errs[0];
                let more = if errs.len() > 1 { format!(" (and {} more)", errs.len() - 1) } else { String::new() };
                c.set_error(Some(&format!("{e}{more}")));
                Err((e.line, e.col, e.msg.clone()))
            }
        }
    }

    /// Run a key through the binds. Returns whether a bind took it.
    fn bind_key(&mut self, c: &mut Ctl<'_>, code: u16, down: bool, repeat: bool, mods: u8) -> bool {
        let key = Key::Code(code);
        if !down {
            // a modifier's own bit is already gone when it is released; `bindr = SUPER, SUPER` means it was held
            let m = mods | keys::modifier_of(code).unwrap_or(0);
            if let Some(b) = self.cfg.bind_for(self.submap.as_deref(), m, key, true).cloned() {
                self.held.retain(|h| *h != code);
                self.run(c, &b.action);
                return true;
            }
            if let Some(i) = self.held.iter().position(|h| *h == code) {
                self.held.remove(i);
                return true;
            }
            return false;
        }
        let Some(b) = self.cfg.bind_for(self.submap.as_deref(), mods, key, false).cloned() else {
            return false;
        };
        if repeat && !b.flags.repeat {
            return true; // (held down: the bind ran once; the repeats are not the window's either)
        }
        if !self.held.contains(&code) {
            self.held.push(code);
        }
        self.run(c, &b.action);
        true
    }
}

/// `gliese_proto::keymap::mods` (what the compositor reports) to `gliese_config::mods` (what binds say). The bits are the same.
fn bind_mods(m: u32) -> u8 {
    (m & (keymap::mods::SHIFT | keymap::mods::CTRL | keymap::mods::ALT | keymap::mods::SUPER)) as u8
}

impl Policy for Gliese {
    fn created(&mut self, c: &mut Ctl<'_>, id: WinId) {
        let Some(info) = c.info(id) else { return };
        let around = info.parent.and_then(|p| c.info(p)).map_or(self.usable(c), |p| p.outer);
        // dialogs, and windows that cannot be resized, float
        let fixed = info.min != (0, 0) && info.min == info.max;
        let floating = info.parent.is_some() || fixed;
        let rect = self.centred(around, info.requested);
        self.wins.push(Win { id, ws: self.active, floating, pinned: false, float_rect: rect, titled: false });
        let n = self.active;
        self.attach(c, id, n, floating);
        self.apply_rules(c, id, true);
        self.relayout(c);
        if self.win(id).is_some_and(|w| w.ws == self.active) {
            self.focus(c, Some(id));
        }
    }

    fn destroyed(&mut self, c: &mut Ctl<'_>, id: WinId) {
        self.detach(id);
        self.wins.retain(|w| w.id != id);
        self.mru.retain(|m| *m != id);
        if self.drag.as_ref().is_some_and(|d| d.id == id) {
            self.drag = None;
        }
        self.relayout(c);
        if c.focused().is_none() {
            let f = self.recent_on_active();
            self.focus(c, f);
        }
    }

    fn key(&mut self, c: &mut Ctl<'_>, code: u16, down: bool, repeat: bool, mods: u32) -> bool {
        self.bind_key(c, code, down, repeat, bind_mods(mods))
    }

    fn button(&mut self, c: &mut Ctl<'_>, button: u8, down: bool, x: i32, y: i32, mods: u32) -> bool {
        self.mouse_button(c, button, down, x, y, bind_mods(mods))
    }

    fn motion(&mut self, c: &mut Ctl<'_>, x: i32, y: i32) -> bool {
        self.mouse_motion(c, x, y)
    }

    fn wants(&mut self, c: &mut Ctl<'_>, id: WinId, what: u32) {
        match what {
            want::FULLSCREEN | want::NOT_FULLSCREEN => {
                let Some(n) = self.win(id).map(|w| w.ws) else { return };
                let is = self.ws[n].fullscreen == Some(id);
                if (what == want::FULLSCREEN) != is {
                    self.ws[n].fullscreen = if is { None } else { Some(id) };
                    self.relayout(c);
                }
            }
            want::FLOAT | want::TILE => {
                let floating = what == want::FLOAT;
                if self.is_floating(id) != floating {
                    self.toggle_floating(c, id);
                }
            }
            _ => {}
        }
    }

    fn title_changed(&mut self, c: &mut Ctl<'_>, id: WinId) {
        let Some(w) = self.win(id) else { return };
        // the first title of a window that has not drawn yet: the rules may want it elsewhere
        let mapped = c.info(id).is_some_and(|i| i.mapped);
        if !w.titled && !mapped {
            if let Some(w) = self.win_mut(id) {
                w.titled = true;
            }
            self.apply_rules(c, id, false);
            self.relayout(c);
        }
    }

    fn config_text(&mut self, c: &mut Ctl<'_>, file: &str, text: &str) -> Result<(), (u32, u32, String)> {
        self.load(c, file, text)
    }

    fn take_agent_requests(&mut self) -> Vec<AgentEvent> {
        self.take_agent_events()
    }

    fn program_named(&mut self, program: Program, name: &str) {
        self.set_program_name(program, name);
    }
}
