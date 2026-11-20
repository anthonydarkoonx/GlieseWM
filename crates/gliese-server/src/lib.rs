//! GlieseWM's compositor (`PLAN.md` §3): sessions, windows and their double buffers, the configure handshake, input routing and
//! drawing the screen. What windows go where is not decided here but by a [`Policy`] (the tiling window manager, `gliese-core`;
//! [`FloatingPolicy`] until then and in tests).
//!
//! The system is behind [`Host`]: shared memory, waking clients and the task table (`gliese-os` on Metallium-OS, [`sim`] on the
//! development machine). The owner calls [`Server::request`] for each call on the display endpoint, [`Server::input`] for each input
//! event, [`Server::tick`] now and then, and [`Server::render`] to bring the screen up to date; `render` returns the rectangles that
//! changed (to present with `GPU_FLUSH2`).
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod input;
mod policy;
mod pools;
mod render;
mod requests;
#[cfg(feature = "sim")]
pub mod sim;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use gliese_proto::{session as S, Event};

pub use gliese_layout::Rect;
pub use input::Input;
pub use render::BAR_BG;
pub use policy::{AgentRequest, Ctl, FloatingPolicy, Policy, WinInfo};

pub type Task = u32;
pub type Program = u32;
/// A shared object, as the host names it.
pub type ShmId = u32;
/// A window, as the compositor and the policy name it (never reused while the compositor runs).
pub type WinId = u32;

/// What the compositor needs from the system.
pub trait Host {
    /// New zeroed shared memory, the compositor's (it pays for it and can destroy it).
    fn shm_create(&mut self, pages: usize) -> Result<ShmId, i64>;
    /// Give `task` a copy it may map and write; returns the slot it lands in, in that task.
    fn shm_grant(&mut self, shm: ShmId, task: Task) -> Result<u32, i64>;
    /// Destroy the object: every copy and mapping of it goes.
    fn shm_destroy(&mut self, shm: ShmId);
    /// The object, mapped here, as 32-bit words.
    fn shm(&mut self, shm: ShmId) -> Option<&mut [u32]>;
    /// Wake whoever sleeps on word `word` of the object.
    fn wake(&mut self, shm: ShmId, word: usize);
    /// The program a task belongs to (threads of one program share a session).
    fn program_of(&self, task: Task) -> Program;
    fn task_alive(&self, task: Task) -> bool;
    fn now_ms(&self) -> u64;
    /// The attention keys (Ctrl+Alt+Delete, Ctrl+Shift+Escape) were pressed. No program sees them.
    fn attention(&mut self) {}
    /// Switch the machine off (0) or restart it (1), if the compositor holds the right. Does not return when it works.
    fn power(&mut self, _what: u32) -> i64 {
        -1
    }
}

/// Limits (`PLAN.md` §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub sessions: usize,
    pub windows_total: usize,
    /// Pages all window buffers may hold together.
    pub buffer_pages: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { sessions: 32, windows_total: 256, buffer_pages: 32 * 256 }
    }
}

/// The look the compositor draws itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub background: u32,
    pub active_border: u32,
    pub inactive_border: u32,
}

impl Default for Look {
    fn default() -> Look {
        Look { background: 0x2E3440, active_border: 0x5E81AC, inactive_border: 0x3B4252 }
    }
}

/// A window's two buffers of `w` x `h`, `offset` pages into pool `pool` of its session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Buf {
    pub pool: u32,
    pub offset: u32,
    pub w: u32,
    pub h: u32,
    pub generation: u32,
}

impl Buf {
    pub fn pages(&self) -> usize {
        gliese_proto::buffer_pages(self.w, self.h)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Conf {
    pub serial: u32,
    pub w: u32,
    pub h: u32,
    pub state: u32,
}

pub(crate) struct Session {
    pub program: Program,
    pub tasks: Vec<Task>,
    pub page: ShmId,
    pub role: u32,
    pub wake: bool,
    pub pools: Vec<pools::Pool>,
    /// A config upload under way (the desktop role only).
    pub upload: Vec<u8>,
}

pub(crate) struct Window {
    pub id: WinId,
    pub sess: usize,
    pub num: u8,
    pub parent: Option<WinId>,
    pub title: String,
    /// On screen, border included.
    pub outer: Rect,
    pub border: i32,
    /// The size the client asked for at CREATE.
    pub requested: (u32, u32),
    /// The buffer on screen and which half of it.
    pub shown: Option<Buf>,
    pub front: u8,
    /// A buffer handed out by ACK_CONFIGURE, not committed yet.
    pub next: Option<Buf>,
    pub conf: Option<Conf>,
    /// Committed at least once.
    pub mapped: bool,
    /// Not drawn (another workspace).
    pub hidden: bool,
    pub min: (u32, u32),
    pub max: (u32, u32),
    pub state: u32,
    pub cursor: u32,
    pub frame_wanted: bool,
}

impl Window {
    pub fn visible(&self) -> bool {
        self.mapped && !self.hidden
    }
    /// Where the client's pixels go.
    pub fn client_rect(&self) -> Rect {
        let b = self.border;
        Rect::new(self.outer.x + b, self.outer.y + b, self.outer.w - 2 * b, self.outer.h - 2 * b)
    }
}

/// Damaged screen areas: a short list of rectangles, merged into their bounding box when it grows long.
#[derive(Clone, Debug, Default)]
pub(crate) struct Region {
    pub rects: Vec<Rect>,
}

impl Region {
    pub fn add(&mut self, r: Rect) {
        if r.is_empty() || self.rects.iter().any(|o| o.contains_rect(&r)) {
            return;
        }
        self.rects.retain(|o| !r.contains_rect(o));
        self.rects.push(r);
        if self.rects.len() > 32 {
            let b = self.bounds();
            self.rects.clear();
            self.rects.push(b);
        }
    }
    pub fn bounds(&self) -> Rect {
        let mut it = self.rects.iter();
        let Some(f) = it.next() else { return Rect::default() };
        let (mut x0, mut y0, mut x1, mut y1) = (f.x, f.y, f.right(), f.bottom());
        for r in it {
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.right());
            y1 = y1.max(r.bottom());
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// Clip `r` to `c`.
pub(crate) fn intersect(r: &Rect, c: &Rect) -> Rect {
    let (x0, y0) = (r.x.max(c.x), r.y.max(c.y));
    let (x1, y1) = (r.right().min(c.right()), r.bottom().min(c.bottom()));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// The compositor's state (the part a [`Ctl`] gives the policy access to).
pub struct State {
    pub(crate) screen: (i32, i32),
    pub(crate) sessions: Vec<Option<Session>>,
    /// Bottom to top.
    pub(crate) windows: Vec<Window>,
    pub(crate) next_id: WinId,
    pub(crate) serial: u32,
    pub(crate) focus: Option<WinId>,
    pub(crate) pointer: (i32, i32),
    pub(crate) pointer_over: Option<WinId>,
    /// The window that gets the pointer while a button is held (an implicit grab).
    pub(crate) grab: Option<WinId>,
    pub(crate) buttons: u8,
    pub(crate) mods: u32,
    pub(crate) damage: Region,
    pub(crate) cursor_drawn: Option<Rect>,
    pub(crate) look: Look,
    pub(crate) limits: Limits,
    pub(crate) manager: Option<Task>,
    /// The program the manager named for the desktop role (it may say so before the program says HELLO).
    pub(crate) desktop_program: Option<Program>,
    /// Pages of all pools together.
    pub(crate) pages_used: usize,
    pub(crate) next_pool: u32,
    /// Windows that asked for FRAME_DONE and committed since the last render.
    pub(crate) frames: Vec<WinId>,
    /// The error bar's text (a config that did not parse), drawn across the top of the screen.
    pub(crate) error: Option<String>,
    /// Requests for the desktop role that wait for it to connect.
    pub(crate) agent_queue: Vec<policy::AgentRequest>,
}

pub struct Server {
    pub(crate) st: State,
    pub(crate) policy: Box<dyn Policy>,
}

impl Server {
    pub fn new(screen_w: i32, screen_h: i32, policy: Box<dyn Policy>) -> Server {
        let mut st = State {
            screen: (screen_w, screen_h),
            sessions: Vec::new(),
            windows: Vec::new(),
            next_id: 1,
            serial: 0,
            focus: None,
            pointer: (screen_w / 2, screen_h / 2),
            pointer_over: None,
            grab: None,
            buttons: 0,
            mods: 0,
            damage: Region::default(),
            cursor_drawn: None,
            look: Look::default(),
            limits: Limits::default(),
            manager: None,
            desktop_program: None,
            pages_used: 0,
            next_pool: 0,
            frames: Vec::new(),
            error: None,
            agent_queue: Vec::new(),
        };
        st.damage.add(Rect::new(0, 0, screen_w, screen_h));
        Server { st, policy }
    }

    /// Only this task's MGR_* requests count (the program manager's, from the desktop contract's `manager` argument).
    pub fn set_manager(&mut self, task: Task) {
        self.st.manager = Some(task);
    }
    pub fn set_limits(&mut self, l: Limits) {
        self.st.limits = l;
    }
    pub fn set_look(&mut self, l: Look) {
        self.st.look = l;
        self.damage_all();
    }
    pub fn screen(&self) -> (i32, i32) {
        self.st.screen
    }
    pub fn damage_all(&mut self) {
        self.st.damage.add(Rect::new(0, 0, self.st.screen.0, self.st.screen.1));
    }
    pub fn needs_render(&self) -> bool {
        !self.st.damage.rects.is_empty() || !self.st.frames.is_empty() || self.st.cursor_moved()
    }
    pub fn focused(&self) -> Option<WinId> {
        self.st.focus
    }
    /// (id, program, the client's number for it, outer rect, visible) of every window, bottom to top.
    pub fn windows(&self) -> Vec<(WinId, Program, u8, Rect, bool)> {
        self.st.windows.iter().map(|w| (w.id, self.st.sessions[w.sess].as_ref().map_or(0, |s| s.program), w.num, w.outer, w.visible())).collect()
    }
    pub fn session_count(&self) -> usize {
        self.st.sessions.iter().flatten().count()
    }
    pub fn pages_used(&self) -> usize {
        self.st.pages_used
    }
    /// What the error bar says, if it is up.
    pub fn error(&self) -> Option<&str> {
        self.st.error.as_deref()
    }
    /// Requests for the desktop role that are waiting for it to connect.
    pub fn agent_queue(&self) -> &[AgentRequest] {
        &self.st.agent_queue
    }
        /// The program the desktop role belongs to, if the manager named one.
    pub fn desktop_program(&self) -> Option<Program> {
        self.st.desktop_program
    }

    /// Run the policy with a [`Ctl`] on the compositor.
    pub fn with_policy<R>(&mut self, host: &mut dyn Host, f: impl FnOnce(&mut dyn Policy, &mut Ctl<'_>) -> R) -> R {
        let mut c = Ctl { st: &mut self.st, host };
        let r = f(&mut *self.policy, &mut c);
        self.deliver_agent(host);
        self.st.flush_wakes(host);
        r
    }

    /// Pass what the window manager wants done to the desktop role (`gliesed`): EXEC and RELOAD events; switching off is the
    /// compositor's own (it holds the power right). Requests wait (a few) until the desktop role has connected.
    pub(crate) fn deliver_agent(&mut self, host: &mut dyn Host) {
        for r in self.policy.take_agent_requests() {
            if r == policy::AgentRequest::Exit {
                host.power(0);
                continue;
            }
            if self.st.agent_queue.len() < 16 {
                self.st.agent_queue.push(r);
            }
        }
        let Some(sess) = self.st.sessions.iter().position(|s| s.as_ref().is_some_and(|s| s.role == gliese_proto::role::DESKTOP)) else { return };
        for r in core::mem::take(&mut self.st.agent_queue) {
            let e = match r {
                policy::AgentRequest::Exec(id) => {
                    let [a, b, c, d] = gliese_proto::pack_id(&id);
                    Event { kind: gliese_proto::ev::EXEC, a, b, c, d, ..Default::default() }
                }
                _ => Event { kind: gliese_proto::ev::RELOAD, ..Default::default() },
            };
            self.st.send(host, sess, e);
        }
    }

    /// Now and then: notice programs that died, let the policy do its timed work, wake clients.
    pub fn tick(&mut self, host: &mut dyn Host) {
        let dead: Vec<usize> = self
            .st
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().filter(|s| !s.tasks.iter().any(|t| host.task_alive(*t))).map(|_| i))
            .collect();
        for i in dead {
            self.end_session(host, i);
        }
        let now = host.now_ms();
        self.with_policy(host, |p, c| p.tick(c, now));
    }

    pub(crate) fn end_session(&mut self, host: &mut dyn Host, i: usize) {
        let ids: Vec<WinId> = self.st.windows.iter().filter(|w| w.sess == i).map(|w| w.id).collect();
        for id in ids {
            self.destroy_window(host, id);
        }
        self.st.drop_pools(host, i);
        if let Some(s) = self.st.sessions[i].take() {
            host.shm_destroy(s.page);
        }
    }

    pub(crate) fn destroy_window(&mut self, host: &mut dyn Host, id: WinId) {
        let Some(i) = self.st.win_index(id) else { return };
        let w = self.st.windows.remove(i);
        for b in [w.shown, w.next].into_iter().flatten() {
            self.st.free_buf(host, w.sess, b.pool, b.offset, b.pages());
        }
        if w.visible() {
            self.st.damage.add(w.outer);
        }
        if self.st.focus == Some(id) {
            self.st.focus = None;
        }
        if self.st.pointer_over == Some(id) {
            self.st.pointer_over = None;
        }
        if self.st.grab == Some(id) {
            self.st.grab = None;
        }
        self.st.frames.retain(|f| *f != id);
        // its dialogs lose their owner
        for c in self.st.windows.iter_mut().filter(|c| c.parent == Some(id)) {
            c.parent = None;
        }
        self.with_policy(host, |p, c| p.destroyed(c, id));
    }
}

impl State {
    pub(crate) fn win_index(&self, id: WinId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }
    pub(crate) fn win(&self, id: WinId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }
    pub(crate) fn win_mut(&mut self, id: WinId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub(crate) fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    /// Queue an event for a session (the wake comes with [`State::flush_wakes`]).
    pub(crate) fn send(&mut self, host: &mut dyn Host, sess: usize, mut e: Event) {
        let Some(s) = self.sessions.get_mut(sess).and_then(|s| s.as_mut()) else { return };
        e.time = host.now_ms() as u32;
        let page_id = s.page;
        s.wake = true;
        let Some(page) = host.shm(page_id) else { return };
        let (head, tail) = (page[S::W_HEAD], page[S::W_TAIL]);
        if head.wrapping_sub(tail) as usize >= S::RING_EVENTS {
            // full: the client is not reading; it will resynchronise from the window table
            page[S::W_FLAGS] |= S::FLAG_OVERFLOW;
            return;
        }
        let at = S::W_RING + (head as usize % S::RING_EVENTS) * S::EVENT_WORDS;
        page[at..at + S::EVENT_WORDS].copy_from_slice(&e.to_words());
        page[S::W_HEAD] = head.wrapping_add(1);
    }

    pub(crate) fn send_to(&mut self, host: &mut dyn Host, id: WinId, mut e: Event) {
        let Some(w) = self.win(id) else { return };
        e.window = w.num as u32;
        let sess = w.sess;
        self.send(host, sess, e);
    }

    pub(crate) fn flush_wakes(&mut self, host: &mut dyn Host) {
        for s in self.sessions.iter_mut().flatten() {
            if s.wake {
                s.wake = false;
                if let Some(page) = host.shm(s.page) {
                    page[S::W_SEQ] = page[S::W_SEQ].wrapping_add(1);
                }
                host.wake(s.page, S::W_SEQ);
            }
        }
    }

    /// Write a window's entry in its session's table.
    pub(crate) fn write_table(&mut self, host: &mut dyn Host, id: WinId) {
        let Some(w) = self.win(id) else { return };
        let Some(page_id) = self.sessions.get(w.sess).and_then(|s| s.as_ref()).map(|s| s.page) else { return };
        let buf = w.next.or(w.shown);
        let (num, conf) = (w.num as usize, w.conf);
        let Some(page) = host.shm(page_id) else { return };
        if let Some(b) = buf {
            page[S::win(num, S::WIN_POOL)] = b.pool;
            page[S::win(num, S::WIN_OFFSET)] = b.offset;
            page[S::win(num, S::WIN_W)] = b.w;
            page[S::win(num, S::WIN_H)] = b.h;
            page[S::win(num, S::WIN_GENERATION)] = b.generation;
        }
        if let Some(c) = conf {
            page[S::win(num, S::WIN_CONF_SERIAL)] = c.serial;
            page[S::win(num, S::WIN_CONF_W)] = c.w;
            page[S::win(num, S::WIN_CONF_H)] = c.h;
            page[S::win(num, S::WIN_CONF_STATE)] = c.state;
        }
    }

    /// The topmost visible window at a point.
    pub(crate) fn window_at(&self, x: i32, y: i32) -> Option<WinId> {
        self.windows.iter().rev().find(|w| w.visible() && w.outer.contains(x, y)).map(|w| w.id)
    }
}
