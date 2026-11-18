//! GlieseWM's window protocol (`PLAN.md` §7).
//!
//! **Requests** are `IPC_CALL`s on the display endpoint, three words each: `w0 = op | window << 8 | arg << 24`, `w1`, `w2`. The
//! reply's first word is a status (0 or a negative error), the other two are results. The program manager's `MGR_SET_ROLE` (op 97,
//! sent by metal-os's `progmgr`) uses the same packing.
//!
//! **Events** go the other way through the client's *session page*, shared memory the compositor makes for each program: a ring
//! of fixed-size events the compositor writes and the client reads, then a `WAKE_WORD` on [`session::W_SEQ`]. There is no
//! endpoint per client, so a client that stops reading cannot block the compositor.
//!
//! **Buffers**: each window has two buffers of `w * h` pixels (`0x00RRGGBB`), side by side, the compositor's to make and pay for.
//! The client draws into the one that is not on screen and commits it. Buffers are cut out of a few large shared objects per
//! program, *pools*, not one object per window: on Metal OS a task has only 32 capability slots (and 64 mappings), so an object
//! per window would limit the whole desktop to a dozen windows. A program holds the session page plus one slot per pool.
#![no_std]
#![forbid(unsafe_code)]

pub mod keymap;

pub const VERSION: u32 = 1;

/// Request ops.
pub mod op {
    /// `arg` = protocol version → `[0, session page slot, 0]`.
    pub const HELLO: u8 = 1;
    /// Leave: every window goes.
    pub const BYE: u8 = 2;
    /// `window` = the client's number for it (0..MAX_WINDOWS), `w1` = size word (the size it would like), `w2` = parent window
    /// number + 1 (0: none) → `[0, 0, 0]`; a CONFIGURE event follows.
    pub const CREATE: u8 = 3;
    pub const DESTROY: u8 = 4;
    /// The title is in the session page's scratch area, `arg` bytes of UTF-8.
    pub const SET_TITLE: u8 = 5;
    /// `w1` = minimum size word, `w2` = maximum size word (0: none).
    pub const SET_LIMITS: u8 = 6;
    /// `arg` = the configure serial → `[0, pool slot | pool id << 32, offset in pages | generation << 32]`: the window's two buffers
    /// start `offset` pages into the pool in that slot. The pool id is never reused while the compositor runs: a slot number
    /// that comes back with a different id holds a new pool (the old one was destroyed), so a cached mapping of it is stale.
    pub const ACK_CONFIGURE: u8 = 7;
    /// `arg` = buffer index (0 or 1) | [`COMMIT_FRAME`]; `w1` = damage rect word (0: all of it); `w2` = the generation drawn for.
    pub const COMMIT: u8 = 8;
    /// `arg` = a [`cursor`] shape for when the pointer is over this window.
    pub const SET_CURSOR: u8 = 9;
    /// `arg` = a [`want`]: what the window would like (the window manager decides).
    pub const WANT: u8 = 10;

    // 32..=47: any program. 48..=63: the desktop role only. (PLAN.md §7)
    /// Read-only state for tools: `arg` = what (see `gliese-core`).
    pub const GL_QUERY: u8 = 32;
    /// Ask the desktop role (`gliesed`) to read the config again.
    pub const GL_RELOAD: u8 = 33;
    /// Desktop role: start a config upload (`arg` = total bytes).
    pub const GL_CONFIG_BEGIN: u8 = 48;
    /// Desktop role: the next `arg` bytes of the config are in the scratch area.
    pub const GL_CONFIG_CHUNK: u8 = 49;
    /// Desktop role: the upload is complete → `[0, 0, 0]`, or `[E_CONFIG, line, col]` (the message is in the scratch area).
    pub const GL_CONFIG_END: u8 = 50;
    /// Desktop role: send program `w1` (a task of it) a NEW_WINDOW event.
    pub const GL_NEW_WINDOW: u8 = 51;
    /// Desktop role: program `w1` (a task of it) has this id (`arg` bytes in the scratch area), for `program:` window rules.
    pub const GL_PROGRAM_NAME: u8 = 52;

    /// 96..=99 belong to the program manager (metal-os `progmgr`).
    pub const MGR_FIRST: u8 = 96;
    pub const MGR_LAST: u8 = 99;
    /// From the manager only: `arg` = role, `w1` = the task that has it.
    pub const MGR_SET_ROLE: u8 = 97;
}

/// `COMMIT` flag: send FRAME_DONE when this frame has been shown.
pub const COMMIT_FRAME: u32 = 1 << 8;

/// Roles the manager can give (`MGR_SET_ROLE`).
pub mod role {
    pub const NONE: u32 = 0;
    pub const DESKTOP: u32 = 1;
}

/// What a window would like (`WANT`).
pub mod want {
    pub const FULLSCREEN: u32 = 1;
    pub const NOT_FULLSCREEN: u32 = 2;
    pub const FLOAT: u32 = 3;
    pub const TILE: u32 = 4;
}

/// Pointer shapes.
pub mod cursor {
    pub const ARROW: u32 = 0;
    pub const TEXT: u32 = 1;
    pub const HAND: u32 = 2;
    pub const MOVE: u32 = 3;
    pub const NONE: u32 = 4;
}

/// Window state bits (CONFIGURE `c`, and the window table).
pub mod state {
    /// It has the keyboard.
    pub const ACTIVATED: u32 = 1;
    /// It is a tile (its size is the window manager's, not its own).
    pub const TILED: u32 = 2;
    pub const FULLSCREEN: u32 = 4;
    /// Being resized interactively: frames are wanted fast.
    pub const RESIZING: u32 = 8;
}

/// Errors (the reply's status).
pub const E_PERM: i64 = -1;
pub const E_BADWIN: i64 = -2;
pub const E_FULL: i64 = -3;
pub const E_INVAL: i64 = -5;
pub const E_NOMEM: i64 = -8;
/// Stale: a commit for a buffer generation that has been replaced.
pub const E_AGAIN: i64 = -10;
/// GL_CONFIG_END: the config has an error.
pub const E_CONFIG: i64 = -20;

/// The largest window side.
pub const MAX_SIDE: u32 = 8192;

pub const fn w0(op: u8, window: u8, arg: u32) -> u64 {
    op as u64 | (window as u64) << 8 | (arg as u64) << 24
}
pub const fn split_w0(w0: u64) -> (u8, u8, u32) {
    (w0 as u8, (w0 >> 8) as u8, (w0 >> 24) as u32)
}
pub const fn size_word(w: u32, h: u32) -> u64 {
    (w as u64 & 0xFFFF) | (h as u64 & 0xFFFF) << 16
}
pub const fn split_size(v: u64) -> (u32, u32) {
    ((v & 0xFFFF) as u32, ((v >> 16) & 0xFFFF) as u32)
}
/// `x | y << 16 | w << 32 | h << 48`, 16 bits each (x and y unsigned: rectangles inside a buffer).
pub const fn rect_word(x: u32, y: u32, w: u32, h: u32) -> u64 {
    (x as u64 & 0xFFFF) | (y as u64 & 0xFFFF) << 16 | (w as u64 & 0xFFFF) << 32 | (h as u64 & 0xFFFF) << 48
}
pub const fn split_rect(v: u64) -> (u32, u32, u32, u32) {
    ((v & 0xFFFF) as u32, ((v >> 16) & 0xFFFF) as u32, ((v >> 32) & 0xFFFF) as u32, ((v >> 48) & 0xFFFF) as u32)
}

/// Pages a `w` x `h` window's two buffers take in a pool.
pub const fn buffer_pages(w: u32, h: u32) -> usize {
    (2 * w as usize * h as usize).div_ceil(1024)
}

/// The largest pool (`SHM_CREATE` takes at most 4096 pages: 16 MiB, two 1920 x 1080 buffers).
pub const MAX_POOL_PAGES: usize = 4096;
/// The most pools a program has at once.
pub const MAX_POOLS: usize = 6;

/// The session page: words of 32 bits.
pub mod session {
    pub const PAGES: usize = 4;
    pub const WORDS: usize = PAGES * 1024;
    pub const MAGIC: u32 = 0x4C47_4C47; // "GLGL"
    pub const W_MAGIC: usize = 0;
    pub const W_VERSION: usize = 1;
    /// Bumped (and woken) whenever events were added: a client waits on it.
    pub const W_SEQ: usize = 2;
    /// Events written so far (the compositor's), and read so far (the client's). Free-running; the slot is `n % RING_EVENTS`.
    pub const W_HEAD: usize = 3;
    pub const W_TAIL: usize = 4;
    /// [`FLAG_OVERFLOW`]: events were dropped because the ring was full (the window table still has every window's configure).
    pub const W_FLAGS: usize = 5;
    pub const FLAG_OVERFLOW: u32 = 1;
    pub const W_SCREEN_W: usize = 6;
    pub const W_SCREEN_H: usize = 7;

    /// The window table: [`WIN_WORDS`] words for each window number.
    pub const W_WINDOWS: usize = 16;
    pub const MAX_WINDOWS: usize = 64;
    pub const WIN_WORDS: usize = 12;
    /// The pool id the window's buffers are in (0: none yet), and where in it (pages).
    pub const WIN_POOL: usize = 0;
    pub const WIN_OFFSET: usize = 1;
    /// Size of each of its two buffers.
    pub const WIN_W: usize = 2;
    pub const WIN_H: usize = 3;
    pub const WIN_GENERATION: usize = 4;
    /// The latest configure: serial, size, state (a client that missed the event finds it here).
    pub const WIN_CONF_SERIAL: usize = 5;
    pub const WIN_CONF_W: usize = 6;
    pub const WIN_CONF_H: usize = 7;
    pub const WIN_CONF_STATE: usize = 8;

    /// Scratch: titles going in, config text going in, error messages coming out.
    pub const W_SCRATCH: usize = 1024;
    pub const SCRATCH_BYTES: usize = 2048;

    /// The event ring.
    pub const W_RING: usize = 1536;
    pub const RING_EVENTS: usize = 256;
    pub const EVENT_WORDS: usize = 8;

    pub const fn win(num: usize, field: usize) -> usize {
        W_WINDOWS + num * WIN_WORDS + field
    }
}

// The session page's parts fit, in order, without overlapping.
const _: () = assert!(session::win(session::MAX_WINDOWS, 0) <= session::W_SCRATCH);
const _: () = assert!(session::W_SCRATCH + session::SCRATCH_BYTES / 4 <= session::W_RING);
const _: () = assert!(session::W_RING + session::RING_EVENTS * session::EVENT_WORDS <= session::WORDS);

/// Event kinds (the first word of an event).
pub mod ev {
    /// `a` = width, `b` = height, `c` = state bits: draw at this size and ACK_CONFIGURE `serial`.
    pub const CONFIGURE: u32 = 1;
    /// The user wants the window closed (`killactive`, Alt+F4).
    pub const CLOSE_REQUEST: u32 = 2;
    /// `a` = 1 focus in, 0 focus out.
    pub const FOCUS: u32 = 3;
    /// `a` = key number, `b` = 0 up / 1 down / 2 repeat, `c` = modifiers (`keymap::mods`), `d` = the character (0: none).
    pub const KEY: u32 = 4;
    /// `a`, `b` = position in the client area.
    pub const POINTER_ENTER: u32 = 5;
    pub const POINTER_LEAVE: u32 = 6;
    pub const POINTER_MOTION: u32 = 7;
    /// `a` = button (1 left, 2 right, 3 middle), `b` = 1 down / 0 up, `c`, `d` = position.
    pub const POINTER_BUTTON: u32 = 8;
    /// `a` = clicks (signed).
    pub const POINTER_WHEEL: u32 = 9;
    /// The frame committed with `COMMIT_FRAME` is on screen: draw the next one.
    pub const FRAME_DONE: u32 = 10;
    /// Open another window (`exec` of a program that already runs).
    pub const NEW_WINDOW: u32 = 11;
    /// For the desktop role: start this program; its id (up to 16 bytes, NUL padded) is in `a`..`d` ([`unpack_id`]).
    pub const EXEC: u32 = 12;
    /// For the desktop role: read the config again.
    pub const RELOAD: u32 = 13;
}

/// One event in the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Event {
    pub kind: u32,
    /// The window's number (the client's).
    pub window: u32,
    pub serial: u32,
    /// Milliseconds (the low 32 bits of the compositor's clock).
    pub time: u32,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}

impl Event {
    pub fn to_words(&self) -> [u32; session::EVENT_WORDS] {
        [self.kind, self.window, self.serial, self.time, self.a, self.b, self.c, self.d]
    }
    pub fn from_words(w: &[u32]) -> Event {
        Event { kind: w[0], window: w[1], serial: w[2], time: w[3], a: w[4], b: w[5], c: w[6], d: w[7] }
    }
}

/// The largest config upload.
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// A program id (up to 16 bytes) as four words, for an event's `a`..`d`.
pub fn pack_id(id: &str) -> [u32; 4] {
    let mut b = [0u8; 16];
    let n = id.len().min(16);
    b[..n].copy_from_slice(&id.as_bytes()[..n]);
    core::array::from_fn(|i| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]))
}

/// The id back from [`pack_id`]'s words (into `out`; the bytes up to the first NUL).
pub fn unpack_id(w: [u32; 4], out: &mut [u8; 16]) -> &str {
    for (i, x) in w.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
    let n = out.iter().position(|b| *b == 0).unwrap_or(16);
    core::str::from_utf8(&out[..n]).unwrap_or("")
}

/// Write `bytes` into the scratch area of a session page (as many as fit; returns how many).
pub fn scratch_write(page: &mut [u32], bytes: &[u8]) -> usize {
    let n = bytes.len().min(session::SCRATCH_BYTES);
    for (i, chunk) in bytes[..n].chunks(4).enumerate() {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        page[session::W_SCRATCH + i] = u32::from_le_bytes(w);
    }
    n
}

/// Read `n` bytes from the scratch area (at most `SCRATCH_BYTES`) into `out`; returns the slice filled.
pub fn scratch_read<'a>(page: &[u32], n: usize, out: &'a mut [u8; session::SCRATCH_BYTES]) -> &'a [u8] {
    let n = n.min(session::SCRATCH_BYTES);
    for i in 0..n.div_ceil(4) {
        let w = page[session::W_SCRATCH + i].to_le_bytes();
        let end = (i * 4 + 4).min(n);
        out[i * 4..end].copy_from_slice(&w[..end - i * 4]);
    }
    &out[..n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing() {
        assert_eq!(split_w0(w0(op::COMMIT, 7, 1 | COMMIT_FRAME)), (op::COMMIT, 7, 1 | COMMIT_FRAME));
        // the manager's word, as progmgr writes it
        assert_eq!(split_w0(97 | 1 << 24), (op::MGR_SET_ROLE, 0, role::DESKTOP));
        assert_eq!(split_size(size_word(1024, 768)), (1024, 768));
        assert_eq!(split_rect(rect_word(1, 2, 3, 4)), (1, 2, 3, 4));
        assert_eq!(buffer_pages(1024, 768), 1536);
        let e = Event { kind: ev::KEY, window: 3, serial: 9, time: 1, a: 30, b: 1, c: 0, d: 'a' as u32 };
        assert_eq!(Event::from_words(&e.to_words()), e);
    }

    #[test]
    fn scratch_round_trip() {
        let mut page = [0u32; session::WORDS];
        assert_eq!(scratch_write(&mut page, "héllo, title".as_bytes()), 13);
        let mut buf = [0u8; session::SCRATCH_BYTES];
        assert_eq!(scratch_read(&page, 13, &mut buf), "héllo, title".as_bytes());
    }
}
