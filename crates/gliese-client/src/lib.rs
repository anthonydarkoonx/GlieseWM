//! The program side of GlieseWM's window protocol (`gliese-proto`).
//!
//! ```text
//! let mut c = Connection::connect(&mut t)?;
//! let win = c.create(&mut t, 640, 480, None)?;
//! loop {
//!     while let Some(e) = c.next_event(&mut t) {
//!         // CONFIGURE: the buffers already have the new size (it was acknowledged); draw everything
//!         if let Some(px) = c.back_buffer(&mut t, win) { /* draw */ }
//!         c.commit(&mut t, win, None, true)?;
//!     }
//!     c.wait(&mut t, 0);
//! }
//! ```
//!
//! The system is behind [`Transport`]: `IPC_CALL` on the display endpoint and shared memory on Metal OS (`gliese-os`), the
//! compositor called directly in the simulator (`gliese-server`'s `sim`). Every method takes the transport, so a connection holds no
//! borrow of it.
#![no_std]
#![forbid(unsafe_code)]

use gliese_proto::{self as p, ev, op, session as S, Event};

/// What a client needs from the system.
pub trait Transport {
    /// One request; the reply's first word is the status (as `i64`).
    fn call(&mut self, w: [u64; 3]) -> [u64; 3];
    /// A shared object the compositor granted (by slot), mapped, as words.
    fn shm(&mut self, slot: u32) -> Option<&mut [u32]>;
    /// Give a slot back (unmap it and drop the capability): the session page, when leaving.
    fn unmap(&mut self, slot: u32);
    /// Drop what is cached about a slot (its mapping) without touching the capability: the object in it was destroyed by the
    /// compositor (its mapping went with it) and the slot may hold a new one now.
    fn forget(&mut self, slot: u32);
    /// Sleep while word `word` of the object in `slot` equals `value` (0 ms: no limit).
    fn wait(&mut self, slot: u32, word: usize, value: u32, timeout_ms: u64);
}

#[derive(Clone, Copy, Debug)]
struct Win {
    /// The pool slot the buffers are in (0: none yet), and where in it (pages).
    slot: u32,
    offset: u32,
    w: u32,
    h: u32,
    generation: u32,
    /// Which of the two buffers is on screen (None before the first commit).
    front: Option<u8>,
    /// The last configure acknowledged.
    acked: u32,
    state: u32,
}

pub struct Connection {
    session: u32,
    windows: [Option<Win>; S::MAX_WINDOWS],
    seq_seen: u32,
    /// The pools seen: (slot, pool id).
    pools: [(u32, u32); p::MAX_POOLS * 2],
}

fn status(r: [u64; 3]) -> Result<[u64; 3], i64> {
    let s = r[0] as i64;
    if s < 0 {
        Err(s)
    } else {
        Ok(r)
    }
}

impl Connection {
    pub fn connect(t: &mut dyn Transport) -> Result<Connection, i64> {
        let r = status(t.call([p::w0(op::HELLO, 0, p::VERSION), 0, 0]))?;
        let session = r[1] as u32;
        let page = t.shm(session).ok_or(p::E_NOMEM)?;
        if page.len() < S::WORDS || page[S::W_MAGIC] != S::MAGIC {
            return Err(p::E_INVAL);
        }
        Ok(Connection { session, windows: [None; S::MAX_WINDOWS], seq_seen: 0, pools: [(0, 0); p::MAX_POOLS * 2] })
    }

    /// The screen size the compositor reported.
    pub fn screen(&self, t: &mut dyn Transport) -> (u32, u32) {
        t.shm(self.session).map_or((0, 0), |p| (p[S::W_SCREEN_W], p[S::W_SCREEN_H]))
    }

    /// A new window of about `w` x `h` (the window manager decides the real size: a CONFIGURE comes). `parent`: a dialog's owner.
    pub fn create(&mut self, t: &mut dyn Transport, w: u32, h: u32, parent: Option<u8>) -> Result<u8, i64> {
        let num = self.windows.iter().position(|w| w.is_none()).ok_or(p::E_FULL)? as u8;
        status(t.call([p::w0(op::CREATE, num, 0), p::size_word(w, h), parent.map_or(0, |p| p as u64 + 1)]))?;
        self.windows[num as usize] = Some(Win { slot: 0, offset: 0, w: 0, h: 0, generation: 0, front: None, acked: 0, state: 0 });
        Ok(num)
    }

    pub fn destroy(&mut self, t: &mut dyn Transport, num: u8) {
        if self.windows.get_mut(num as usize).and_then(Option::take).is_some() {
            t.call([p::w0(op::DESTROY, num, 0), 0, 0]);
        }
    }

    pub fn set_title(&mut self, t: &mut dyn Transport, num: u8, title: &str) -> Result<(), i64> {
        let n = {
            let page = t.shm(self.session).ok_or(p::E_NOMEM)?;
            p::scratch_write(page, title.as_bytes())
        };
        status(t.call([p::w0(op::SET_TITLE, num, n as u32), 0, 0])).map(|_| ())
    }

    pub fn set_limits(&mut self, t: &mut dyn Transport, num: u8, min: (u32, u32), max: (u32, u32)) -> Result<(), i64> {
        status(t.call([p::w0(op::SET_LIMITS, num, 0), p::size_word(min.0, min.1), p::size_word(max.0, max.1)])).map(|_| ())
    }

    pub fn want(&mut self, t: &mut dyn Transport, num: u8, what: u32) -> Result<(), i64> {
        status(t.call([p::w0(op::WANT, num, what), 0, 0])).map(|_| ())
    }

    pub fn set_cursor(&mut self, t: &mut dyn Transport, num: u8, shape: u32) -> Result<(), i64> {
        status(t.call([p::w0(op::SET_CURSOR, num, shape), 0, 0])).map(|_| ())
    }

    /// The size the window's buffers have now (0 x 0 before the first configure).
    pub fn size(&self, num: u8) -> (u32, u32) {
        self.windows.get(num as usize).and_then(|w| *w).map_or((0, 0), |w| (w.w, w.h))
    }

    /// The window's state bits (`gliese_proto::state`) as last configured.
    pub fn state(&self, num: u8) -> u32 {
        self.windows.get(num as usize).and_then(|w| *w).map_or(0, |w| w.state)
    }

    /// The next event, if one is waiting. A CONFIGURE is acknowledged before it is returned (the buffers have the new size), and
    /// configures already replaced by a newer one are skipped.
    pub fn next_event(&mut self, t: &mut dyn Transport) -> Option<Event> {
        loop {
            let (e, overflow) = {
                let page = t.shm(self.session)?;
                if page[S::W_FLAGS] & S::FLAG_OVERFLOW != 0 {
                    // events were lost: drop what is queued and pick up each window's latest configure from the table
                    page[S::W_FLAGS] &= !S::FLAG_OVERFLOW;
                    page[S::W_TAIL] = page[S::W_HEAD];
                    (None, true)
                } else {
                    let (head, tail) = (page[S::W_HEAD], page[S::W_TAIL]);
                    if head == tail {
                        return None;
                    }
                    let at = S::W_RING + (tail as usize % S::RING_EVENTS) * S::EVENT_WORDS;
                    let e = Event::from_words(&page[at..at + S::EVENT_WORDS]);
                    page[S::W_TAIL] = tail.wrapping_add(1);
                    (Some(e), false)
                }
            };
            if overflow {
                for num in 0..S::MAX_WINDOWS {
                    if self.windows[num].is_none() {
                        continue;
                    }
                    let page = t.shm(self.session)?;
                    let serial = page[S::win(num, S::WIN_CONF_SERIAL)];
                    if serial != 0 && Some(serial) != self.windows[num].map(|w| w.acked) {
                        let e = Event { kind: ev::CONFIGURE, window: num as u32, serial, time: 0, a: page[S::win(num, S::WIN_CONF_W)], b: page[S::win(num, S::WIN_CONF_H)], c: page[S::win(num, S::WIN_CONF_STATE)], d: 0 };
                        if self.ack(t, num as u8, e.serial).is_ok() {
                            return Some(e);
                        }
                    }
                }
                continue;
            }
            let e = e?;
            if e.kind == ev::CONFIGURE {
                let num = e.window as usize;
                let latest = t.shm(self.session).map(|p| p[S::win(num.min(S::MAX_WINDOWS - 1), S::WIN_CONF_SERIAL)]);
                if num >= S::MAX_WINDOWS || self.windows[num].is_none() || latest != Some(e.serial) {
                    continue; // (stale, or a window this side no longer has)
                }
                if self.ack(t, num as u8, e.serial).is_err() {
                    continue;
                }
            }
            return Some(e);
        }
    }

    /// Acknowledge a configure: the compositor gives the window buffers of the new size (somewhere in one of this program's pools).
    pub fn ack(&mut self, t: &mut dyn Transport, num: u8, serial: u32) -> Result<(), i64> {
        let r = status(t.call([p::w0(op::ACK_CONFIGURE, num, serial), 0, 0]))?;
        let (slot, pool, offset, generation) = (r[1] as u32, (r[1] >> 32) as u32, r[2] as u32, (r[2] >> 32) as u32);
        self.note_pool(t, slot, pool);
        let (w, h, state) = {
            let page = t.shm(self.session).ok_or(p::E_NOMEM)?;
            (page[S::win(num as usize, S::WIN_W)], page[S::win(num as usize, S::WIN_H)], page[S::win(num as usize, S::WIN_CONF_STATE)])
        };
        let win = self.windows.get_mut(num as usize).and_then(|w| w.as_mut()).ok_or(p::E_BADWIN)?;
        win.acked = serial;
        win.state = state;
        if generation != win.generation || slot != win.slot || offset != win.offset {
            *win = Win { slot, offset, w, h, generation, front: None, acked: serial, state };
        }
        Ok(())
    }

    /// A pool in `slot` with id `pool`: if the slot held another pool before, that one is gone (and so is its mapping).
    fn note_pool(&mut self, t: &mut dyn Transport, slot: u32, pool: u32) {
        if let Some(e) = self.pools.iter_mut().find(|e| e.0 == slot) {
            if e.1 != pool {
                t.forget(slot);
                e.1 = pool;
            }
        } else if let Some(e) = self.pools.iter_mut().find(|e| e.0 == 0) {
            *e = (slot, pool);
        } else {
            // (more slots seen than pools can exist: the oldest entries are of destroyed pools)
            self.pools.rotate_left(1);
            let last = self.pools.len() - 1;
            self.pools[last] = (slot, pool);
            t.forget(slot);
        }
    }

    /// The buffer to draw into next (the one not on screen), `w * h` pixels `0x00RRGGBB`, rows of `w`. Its contents are what was
    /// drawn two frames ago (or zero): draw what changed since then, or everything.
    pub fn back_buffer<'t>(&mut self, t: &'t mut dyn Transport, num: u8) -> Option<&'t mut [u32]> {
        let w = self.windows.get(num as usize).and_then(|w| *w)?;
        if w.slot == 0 {
            return None;
        }
        let n = w.w as usize * w.h as usize;
        let back = match w.front {
            Some(0) => 1,
            _ => 0,
        };
        let start = w.offset as usize * 1024 + back * n;
        let obj = t.shm(w.slot)?;
        obj.get_mut(start..start + n)
    }

    /// Put the back buffer on screen. `damage`: what changed (x, y, w, h in the buffer); `None` for all of it. `frame`: send a
    /// FRAME_DONE event once it has been shown (to pace drawing).
    pub fn commit(&mut self, t: &mut dyn Transport, num: u8, damage: Option<(u32, u32, u32, u32)>, frame: bool) -> Result<(), i64> {
        let w = self.windows.get(num as usize).and_then(|w| *w).ok_or(p::E_BADWIN)?;
        if w.slot == 0 {
            return Err(p::E_AGAIN);
        }
        let back: u8 = if w.front == Some(0) { 1 } else { 0 };
        let dmg = damage.map_or(0, |(x, y, dw, dh)| p::rect_word(x, y, dw, dh));
        status(t.call([p::w0(op::COMMIT, num, back as u32 | if frame { p::COMMIT_FRAME } else { 0 }), dmg, w.generation as u64]))?;
        if let Some(win) = self.windows[num as usize].as_mut() {
            win.front = Some(back);
        }
        Ok(())
    }

    /// Sleep until the compositor has sent something (or `timeout_ms`; 0: no limit).
    pub fn wait(&mut self, t: &mut dyn Transport, timeout_ms: u64) {
        let Some((seq, pending)) = t.shm(self.session).map(|p| (p[S::W_SEQ], p[S::W_HEAD] != p[S::W_TAIL])) else { return };
        if pending || seq != self.seq_seen {
            self.seq_seen = seq;
            return;
        }
        t.wait(self.session, S::W_SEQ, seq, timeout_ms);
        self.seq_seen = t.shm(self.session).map_or(seq, |p| p[S::W_SEQ]);
    }

    /// Leave: every window goes.
    pub fn bye(self, t: &mut dyn Transport) {
        t.call([p::w0(op::BYE, 0, 0), 0, 0]);
        // (the compositor destroyed the pools, and their mappings went with them)
        for (slot, _) in self.pools.iter().filter(|e| e.0 != 0) {
            t.forget(*slot);
        }
        t.unmap(self.session);
    }

    /// The session page's slot (for the desktop role's extra requests).
    pub fn session_slot(&self) -> u32 {
        self.session
    }
}
