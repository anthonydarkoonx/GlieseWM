//! What `gliesed` does (`PLAN.md` §8): the desktop-role program, with no window. The compositor has no file system and cannot start
//! programs; this program can (it has `permission = files`, and the control endpoint the desktop role implies):
//!
//! * at start, it reads `gliese.conf` (its own `/data/gliese.conf`, which is `/data/programdata/gliesed/gliese.conf` on disk) and
//!   uploads it; with no file, the built-in default is uploaded (so deleting the file goes back to the default), then starts the
//!   `exec-once` programs;
//! * with `misc { auto_reload = true }` it reads the file once a second and uploads it again when it changed: saving in `nano`
//!   applies it;
//! * on RELOAD (the `reload` dispatcher, or any program's `GL_RELOAD`) it uploads it again;
//! * on EXEC it starts the program; one that already runs (the program manager runs one instance of each) is asked for another
//!   window instead.
//!
//! The system is behind [`Env`] (the file and the program manager) and [`gliese_client::Transport`] (the compositor).
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use gliese_client::{Connection, Transport};
use gliese_proto::{self as p, ev, op, session as S};

/// Where the config is, as `gliesed` sees its own files.
pub const CONFIG_PATH: &str = "/data/gliese.conf";
/// The same file as a program that sees the whole disk (the terminal) names it: `$GLIESE_CONF`.
pub const CONFIG_PATH_ON_DISK: &str = "/data/programdata/gliesed/gliese.conf";
/// How often the file is looked at with `auto_reload`.
pub const RELOAD_EVERY_MS: u64 = 1000;

/// What happened when a program was to be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// It was started; this is its task.
    Started(u32),
    /// It was running already; this is its task.
    Running(u32),
    /// Not installed, or it could not be started.
    Failed,
}

/// The system, as `gliesed` needs it.
pub trait Env {
    /// The config file's bytes; `None` if there is none.
    fn read_config(&mut self) -> Option<Vec<u8>>;
    /// Start an installed program by id.
    fn start(&mut self, id: &str) -> Start;
    fn now_ms(&mut self) -> u64;
    /// A line for the log.
    fn log(&mut self, _line: &str) {}
}

/// A config the compositor refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

pub struct Agent {
    conn: Connection,
    /// FNV-1a of the bytes last uploaded (`None`: the built-in default).
    last: Option<u64>,
    uploaded_once: bool,
    last_check: u64,
    auto_reload: bool,
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x100_0000_01B3))
}

impl Agent {
    pub fn connect(t: &mut dyn Transport) -> Result<Agent, i64> {
        Ok(Agent { conn: Connection::connect(t)?, last: None, uploaded_once: false, last_check: 0, auto_reload: true })
    }

    /// Read the file (or take the default) and upload it. On the first success, start the `exec-once` programs.
    pub fn load(&mut self, t: &mut dyn Transport, env: &mut dyn Env) -> Result<(), Refused> {
        let file = env.read_config();
        let text = file.as_deref().unwrap_or(gliese_config::DEFAULT.as_bytes());
        self.last = file.as_deref().map(fnv);
        self.last_check = env.now_ms();
        let r = self.upload(t, text);
        match &r {
            Ok(()) => {
                // what this side needs from the config: whether to watch the file, and what to start once
                let cfg = gliese_config::parse("gliese.conf", &String::from_utf8_lossy(text), None).ok();
                self.auto_reload = cfg.as_ref().is_none_or(|c| c.misc.auto_reload);
                if !self.uploaded_once {
                    self.uploaded_once = true;
                    for id in cfg.map(|c| c.exec_once).unwrap_or_default() {
                        self.exec(t, env, &id);
                    }
                }
            }
            Err(e) => {
                let line = alloc::format!("[gliesed] gliese.conf:{}:{}: {}", e.line, e.col, e.msg);
                env.log(&line);
            }
        }
        r
    }

    /// Send a config to the compositor, a scratch area at a time.
    pub fn upload(&mut self, t: &mut dyn Transport, text: &[u8]) -> Result<(), Refused> {
        let fail = |msg: &str| Refused { line: 0, col: 0, msg: msg.into() };
        if text.len() > p::MAX_CONFIG_BYTES {
            return Err(fail("the file is too big (64 KiB at most)"));
        }
        let session = self.conn.session_slot();
        if (t.call([p::w0(op::GL_CONFIG_BEGIN, 0, text.len() as u32), 0, 0])[0] as i64) < 0 {
            return Err(fail("the compositor refused the upload (is this the desktop role?)"));
        }
        for chunk in text.chunks(S::SCRATCH_BYTES) {
            let n = match t.shm(session) {
                Some(pg) => p::scratch_write(pg, chunk),
                None => return Err(fail("no session page")),
            };
            if (t.call([p::w0(op::GL_CONFIG_CHUNK, 0, n as u32), 0, 0])[0] as i64) < 0 {
                return Err(fail("the compositor refused part of the upload"));
            }
        }
        let r = t.call([p::w0(op::GL_CONFIG_END, 0, 0), 0, 0]);
        match r[0] as i64 {
            0 => Ok(()),
            p::E_CONFIG => {
                let n = (r[2] >> 32) as usize;
                let mut buf = [0u8; S::SCRATCH_BYTES];
                let msg = t.shm(session).map_or_else(String::new, |pg| String::from_utf8_lossy(p::scratch_read(pg, n, &mut buf)).into_owned());
                Err(Refused { line: r[1] as u32, col: r[2] as u32, msg })
            }
            _ => Err(fail("the compositor refused the upload")),
        }
    }

    /// Start a program, or ask the running one for a new window.
    pub fn exec(&mut self, t: &mut dyn Transport, env: &mut dyn Env, id: &str) {
        match env.start(id) {
            Start::Started(task) => {
                // tell the compositor the program's id, for `program:` rules
                if let Some(pg) = t.shm(self.conn.session_slot()) {
                    let n = p::scratch_write(pg, id.as_bytes());
                    t.call([p::w0(op::GL_PROGRAM_NAME, 0, n as u32), task as u64, 0]);
                }
            }
            Start::Running(task) => {
                t.call([p::w0(op::GL_NEW_WINDOW, 0, 0), task as u64, 0]);
            }
            Start::Failed => {
                let line = alloc::format!("[gliesed] cannot start `{id}` (is it installed?)");
                env.log(&line);
            }
        }
    }

    /// Handle what the compositor sent, and look at the file if it is time. Returns whether anything happened.
    pub fn step(&mut self, t: &mut dyn Transport, env: &mut dyn Env) -> bool {
        let mut busy = false;
        while let Some(e) = self.conn.next_event(t) {
            busy = true;
            match e.kind {
                ev::EXEC => {
                    let mut buf = [0u8; 16];
                    let id = String::from(p::unpack_id([e.a, e.b, e.c, e.d], &mut buf));
                    if !id.is_empty() {
                        self.exec(t, env, &id);
                    }
                }
                ev::RELOAD => {
                    let _ = self.load(t, env);
                }
                _ => {}
            }
        }
        let now = env.now_ms();
        if self.auto_reload && now.saturating_sub(self.last_check) >= RELOAD_EVERY_MS {
            self.last_check = now;
            let file = env.read_config();
            if file.as_deref().map(fnv) != self.last {
                busy = true;
                let _ = self.load(t, env);
            }
        }
        busy
    }

    /// Sleep until the compositor sends something, or until it is time to look at the file again.
    pub fn wait(&mut self, t: &mut dyn Transport) {
        self.conn.wait(t, RELOAD_EVERY_MS);
    }
}
