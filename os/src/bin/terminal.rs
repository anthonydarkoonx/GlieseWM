//! terminal: GlieseWM's shell windows (`PLAN.md` §8.1). One program, any number of windows: it opens one when it starts and
//! another each time GlieseWM sends NEW_WINDOW (Super+Enter while it runs). Each window has its own shell (`gliese-shell`) and
//! screen (`gliese-term`).
//!
//! Programs typed at the prompt are started here: the manifest's `permission = spawn` gives this program a spawn capability and
//! an endpoint factory (metal-os `docs/PROGRAMS.md`, "Starting programs"). Each program gets a console endpoint of its own; a
//! thread per program serves the console protocol on it (metal-os `docs/ABI.md`, "Console"), because a task can owe only one
//! reply at a time and a program waiting to READ must not hold up the other windows.
#![no_std]
#![no_main]
extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use gliese_client::Connection;
use gliese_os::OsTransport;
use gliese_proto::{self as p, ev};
use gliese_shell::{key_byte, Effect, Entry, FsError, Shell, Sys, GLIESE_CONF, HOME};
use gliese_term::Term;
use metal_programs::io::ErrorKind;
use metal_programs::process::{Child, Command};
use metal_programs::sync::Mutex;
use metal_programs::thread::{self, JoinHandle};
use metal_programs::*;

/// The endpoint factory (`permission = spawn`).
const FACTORY: u64 = 5;
/// The display endpoint (`permission = display`) and the program manager's control endpoint (`permission = shell`).
const DISPLAY: u64 = 2;
const CTL: u64 = 3;
const IPC_RECV: u64 = 2;
const IPC_REPLY: u64 = 8;
const CLOCK: u64 = 34;

// ---- the system, for the shell -------------------------------------------------------------------------------------------------

struct OsSys {
    /// The screen's size (for `fetch`).
    screen: (u32, u32),
}

/// The CPU's name from its MIDR (from metal-os `user/src/info.rs`, the console's `fetch`).
fn cpu_name(midr: u64) -> String {
    let (implementer, part) = (midr >> 24 & 0xff, midr >> 4 & 0xfff);
    let name = match (implementer, part) {
        (0x41, 0xd03) => "Cortex-A53",
        (0x41, 0xd04) => "Cortex-A35",
        (0x41, 0xd05) => "Cortex-A55",
        (0x41, 0xd07) => "Cortex-A57",
        (0x41, 0xd08) => "Cortex-A72",
        (0x41, 0xd09) => "Cortex-A73",
        (0x41, 0xd0a) => "Cortex-A75",
        (0x41, 0xd0b) => "Cortex-A76",
        (0x41, 0xd0c) => "Neoverse-N1",
        (0x41, 0xd0d) => "Cortex-A77",
        (0x00, _) | (0x51, _) => "QEMU virtual CPU",
        (0x41, _) => return format!("ARM part {part:#x}"),
        _ => return format!("implementer {implementer:#x} part {part:#x}"),
    };
    String::from(name)
}


fn fs_err(e: metal_programs::io::Error) -> FsError {
    match e.kind {
        ErrorKind::NotFound => FsError::NotFound,
        k => FsError::Other(format!("{k:?}")),
    }
}

/// A program id as the two words the control protocol carries.
fn pack(id: &str) -> (u64, u64) {
    let mut b = [0u8; 16];
    let n = id.len().min(16);
    b[..n].copy_from_slice(&id.as_bytes()[..n]);
    let word = |s: &[u8]| s.iter().rev().fold(0u64, |v, x| v << 8 | *x as u64);
    (word(&b[..8]), word(&b[8..]))
}

impl Sys for OsSys {
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        fs::read(path).map_err(fs_err)
    }
    fn write(&mut self, path: &str, data: &[u8], append: bool) -> Result<(), FsError> {
        if append {
            let mut all = fs::read(path).unwrap_or_default();
            all.extend_from_slice(data);
            fs::write(path, &all).map_err(fs_err)
        } else {
            fs::write(path, data).map_err(fs_err)
        }
    }
    fn list(&mut self, path: &str) -> Result<Vec<Entry>, FsError> {
        let list = fs::read_dir(path).map_err(fs_err)?;
        Ok(list.into_iter().map(|e| Entry { name: e.name.trim_end_matches('/').to_string(), dir: e.is_dir }).collect())
    }
    fn stat(&mut self, path: &str) -> Result<(u64, bool), FsError> {
        let m = fs::metadata(path).map_err(fs_err)?;
        Ok((m.len, m.is_dir))
    }
    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        fs::create_dir(path).map_err(fs_err)
    }
    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        match fs::remove_file(path) {
            Err(e) if e.kind == ErrorKind::IsADirectory => fs::remove_dir(path).map_err(fs_err),
            r => r.map_err(fs_err),
        }
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        fs::rename(from, to).map_err(fs_err)
    }
    fn prog(&mut self, args: &[&str]) -> Result<String, String> {
        match args {
            ["list"] | [] => {
                let mut s = String::new();
                for (id, name) in ctl::installed() {
                    s.push_str(&format!("{id:16} {name}\n"));
                }
                Ok(s)
            }
            ["start", id] => {
                if ctl::start(id) {
                    Ok(format!("{id}: started\n"))
                } else {
                    Err(format!("{id}: not started (not installed, or running already)"))
                }
            }
            ["stop", id] | ["state", id] => {
                let (a, b) = pack(id);
                let op = if args[0] == "stop" { 4 } else { 5 };
                let (st, r) = call(CTL, [op, a, b]);
                if st != 0 || r[0] != 0 {
                    return Err(format!("{id}: refused (status {})", if st != 0 { st } else { r[0] as i64 }));
                }
                Ok(if op == 4 {
                    format!("{id}: stopping\n")
                } else {
                    let names = ["stopped", "created", "foreground", "background", "destroying", "failed"];
                    let state = names.get((r[1] & 0xFF) as usize).copied().unwrap_or("?");
                    format!("{id}: {state}, {} restarts, {}\n", (r[1] >> 8) & 0xFFFF, if (r[1] >> 24) & 1 == 1 { "running" } else { "not running" })
                })
            }
            _ => Err("usage: prog list | start ID | stop ID | state ID".into()),
        }
    }
    fn reload(&mut self) -> Result<(), String> {
        let (st, r) = call(DISPLAY, [p::w0(p::op::GL_RELOAD, 0, 0), 0, 0]);
        if st == 0 && r[0] == 0 {
            Ok(())
        } else {
            Err("GlieseWM did not take the request".into())
        }
    }
    fn uptime_ms(&mut self) -> u64 {
        svc(CLOCK, 0, 0, 0, 0, 0)[0] / 1_000_000
    }
    fn memory(&mut self) -> Option<(u64, u64)> {
        let r = svc(21, 0, 0, 0, 0, 0);
        (r[0] == 0).then_some((r[2] * 4096, r[3] * 4096))
    }
    fn facts(&mut self) -> Vec<(String, String)> {
        // SYSINFO page 0: (MIDR, frames, free frames, online cores, ticks a second); page 1: (SMMU on, devices, tasks, restarts)
        let (s, t) = (svc(21, 0, 0, 0, 0, 0), svc(21, 1, 0, 0, 0, 0));
        let secs = ticks() / s[5].max(1);
        let (total, free) = (s[2] * 4096 >> 20, s[3] * 4096 >> 20);
        let rows = [
            ("KERNEL", String::from("Metal OS 0.1 aarch64 microkernel")),
            ("UPTIME", format!("{}h {}m {:02}s", secs / 3600, secs / 60 % 60, secs % 60)),
            ("CPU", format!("{}x {}", s[4], cpu_name(s[1]))),
            ("GPU", format!("virtio-gpu {}x{} (2D)", self.screen.0, self.screen.1)),
            ("RAM", format!("{} MiB / {} MiB", total - free, total)),
            ("STORAGE", String::from("virtio-blk (EROFS + LFS)")),
            ("IOMMU", if t[1] != 0 { format!("Arm SMMUv3, {} devices confined", t[2]) } else { String::from("none") }),
            ("TASKS", format!("{} running, {} driver restarts", t[3], t[4])),
        ];
        rows.into_iter().map(|(k, v)| (String::from(k), v)).collect()
    }
}

// ---- a running program -----------------------------------------------------------------------------------------------------------

/// What a program's console thread and the main loop share.
struct JobIo {
    out: Mutex<Vec<u8>>,
    input: Mutex<VecDeque<u8>>,
    /// Bumped when input arrives or ends (the thread waits on it).
    in_seq: AtomicU32,
    eof: AtomicBool,
}

impl JobIo {
    fn new() -> JobIo {
        JobIo { out: Mutex::new(Vec::new()), input: Mutex::new(VecDeque::new()), in_seq: AtomicU32::new(0), eof: AtomicBool::new(false) }
    }
}

struct Job {
    child: Child,
    ep: u64,
    io: Arc<JobIo>,
    thread: Option<JoinHandle<()>>,
}

/// The console protocol for one program, on its own endpoint, until the endpoint ends.
fn serve(ep: u64, io: Arc<JobIo>) {
    loop {
        let r = svc(IPC_RECV, ep, 0, 0, 0, 0);
        if (r[0] as i64) < 0 {
            return; // the endpoint ended: the program is gone
        }
        let (w0, w1, w2, is_call) = (r[2], r[3], r[4], r[5] != 0);
        let reply = match w0 & 0xFF {
            // WRITE (1) and WRITE to standard error (3): n bytes, 6 in w0, 8 in w1, 8 in w2
            1 | 3 => {
                let n = (((w0 >> 8) & 0xFF) as usize).min(22);
                let mut b = [0u8; 22];
                b[..6].copy_from_slice(&(w0 >> 16).to_le_bytes()[..6]);
                b[6..14].copy_from_slice(&w1.to_le_bytes());
                b[14..22].copy_from_slice(&w2.to_le_bytes());
                io.out.lock().extend_from_slice(&b[..n]);
                [0, 0, 0]
            }
            // READ: what has been typed (up to max bytes), or wait for it; 0 bytes is the end of input
            2 => {
                let max = (((w0 >> 8) & 0xFF) as usize).clamp(1, 23);
                loop {
                    let seen = io.in_seq.load(Ordering::Acquire);
                    {
                        let mut q = io.input.lock();
                        if !q.is_empty() {
                            let mut b = [0u8; 23];
                            let n = q.len().min(max);
                            for x in b.iter_mut().take(n) {
                                *x = q.pop_front().unwrap_or(0);
                            }
                            let mut first = [0u8; 8];
                            first[0] = n as u8;
                            first[1..8].copy_from_slice(&b[..7]);
                            break [u64::from_le_bytes(first), u64::from_le_bytes(b[7..15].try_into().unwrap_or([0; 8])), u64::from_le_bytes(b[15..23].try_into().unwrap_or([0; 8]))];
                        }
                    }
                    if io.eof.load(Ordering::Acquire) {
                        break [0, 0, 0];
                    }
                    thread::wait_word(&io.in_seq, seen, 50);
                }
            }
            _ => [(-5i64) as u64, 0, 0],
        };
        if is_call {
            svc(IPC_REPLY, reply[0], reply[1], reply[2], 0, 0);
        }
    }
}

fn start_job(argv: &[String], cwd: &str) -> Result<Job, String> {
    let ep = ep_create(FACTORY);
    if ep < 0 {
        return Err(if ep == -1 || ep == -2 {
            "running programs needs `permission = spawn` (an OS update)".into()
        } else {
            format!("cannot make its console ({ep}): too many programs running?")
        });
    }
    let ep = ep as u64;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(argv[1..].iter().map(String::as_str)).env("PWD", cwd).env("HOME", HOME).env("TERM", "gliese").env("GLIESE_CONF", GLIESE_CONF).files(true).network(true);
    let child = match cmd.spawn_with_console(ep) {
        Ok(c) => c,
        Err(e) => {
            svc(6 /* CAP_REVOKE */, ep, 1, 0, 0, 0);
            return Err(match e.kind {
                ErrorKind::NotFound => "command not found (not a built-in, and not in /system/bin or /data/bin; try help)".into(),
                ErrorKind::PermissionDenied => "running programs needs `permission = spawn` (an OS update)".into(),
                k => format!("cannot start it ({k:?})"),
            });
        }
    };
    let io = Arc::new(JobIo::new());
    let io2 = io.clone();
    let thread = thread::Builder::new().capabilities(1 << ep).spawn(move || serve(ep, io2)).ok();
    if thread.is_none() {
        let _ = child.kill();
        svc(6, ep, 1, 0, 0, 0);
        return Err("cannot serve its console (no thread left)".into());
    }
    Ok(Job { child, ep, io, thread })
}

impl Job {
    fn input(&self, bytes: &[u8]) {
        self.io.input.lock().extend(bytes.iter().copied());
        self.io.in_seq.fetch_add(1, Ordering::Release);
        thread::wake_word(&self.io.in_seq, 1);
    }
    fn eof(&self) {
        self.io.eof.store(true, Ordering::Release);
        self.io.in_seq.fetch_add(1, Ordering::Release);
        thread::wake_word(&self.io.in_seq, 1);
    }
    fn take_output(&self) -> Vec<u8> {
        core::mem::take(&mut *self.io.out.lock())
    }
    /// After the program ended: end its console (the thread returns) and collect the thread.
    fn finish(mut self) {
        self.eof();
        svc(6 /* CAP_REVOKE */, self.ep, 1, 0, 0, 0);
        if let Some(t) = self.thread.take() {
            t.join();
        }
    }
}

// ---- windows -------------------------------------------------------------------------------------------------------------------------

struct Win {
    num: u8,
    term: Term,
    shell: Shell,
    job: Option<Job>,
    dirty: bool,
    focused: bool,
    closing: bool,
}

fn open_window(conn: &mut Connection, t: &mut OsTransport, wins: &mut Vec<Win>) {
    let Ok(num) = conn.create(t, 800, 500, None) else {
        println!("[terminal] cannot open another window");
        return;
    };
    let _ = conn.set_title(t, num, "terminal");
    let shell = Shell::new(80, 24);
    let mut term = Term::new();
    shell.greet(&mut term);
    wins.push(Win { num, term, shell, job: None, dirty: true, focused: true, closing: false });
}

fn draw(conn: &mut Connection, t: &mut OsTransport, w: &mut Win) {
    let (pw, ph) = conn.size(w.num);
    if pw == 0 {
        return;
    }
    if let Some(px) = conn.back_buffer(t, w.num) {
        match w.shell.editor_screen() {
            Some((cells, cur)) => Term::draw_grid(px, pw, ph, &cells, Some(cur)),
            None => {
                let (live, cur) = w.shell.live();
                w.term.draw(px, pw, ph, &live, cur, w.focused);
            }
        }
        if conn.commit(t, w.num, None, false).is_ok() {
            w.dirty = false;
        }
    }
}

fn effect(w: &mut Win, e: Effect) {
    match e {
        Effect::None => {}
        Effect::Run { argv, cwd } => match start_job(&argv, &cwd) {
            Ok(j) => w.job = Some(j),
            Err(why) => w.shell.job_failed(&mut w.term, &argv[0], &why),
        },
        Effect::Input(b) => {
            if let Some(j) = &w.job {
                j.input(&b);
            }
        }
        Effect::Eof => {
            if let Some(j) = &w.job {
                j.eof();
            }
        }
        Effect::Interrupt => {
            if let Some(j) = &w.job {
                let _ = j.child.kill();
            }
        }
        Effect::Scroll(d) => {
            let (cols, _) = Term::grid_for(800, 500);
            w.term.scroll_by(d, cols);
        }
        Effect::Close => w.closing = true,
    }
}

fn main(_c: &Ctx) -> u64 {
    let mut t = OsTransport::default();
    let mut conn = match Connection::connect(&mut t) {
        Ok(c) => c,
        Err(e) => {
            println!("[terminal] cannot reach the compositor ({e})");
            return 1;
        }
    };
    let mut sys = OsSys { screen: conn.screen(&mut t) };
    let mut wins: Vec<Win> = Vec::new();
    open_window(&mut conn, &mut t, &mut wins);
    loop {
        while let Some(e) = conn.next_event(&mut t) {
            if e.kind == ev::NEW_WINDOW {
                open_window(&mut conn, &mut t, &mut wins);
                continue;
            }
            let Some(w) = wins.iter_mut().find(|w| w.num as u32 == e.window) else { continue };
            match e.kind {
                ev::CONFIGURE => {
                    let (cols, rows) = Term::grid_for(e.a, e.b);
                    w.shell.resize(cols, rows);
                    w.dirty = true;
                }
                ev::KEY if e.b != 0 => {
                    if let Some(k) = key_byte(e.a as u16, e.c, e.d) {
                        let fx = w.shell.key(&mut sys, &mut w.term, k);
                        effect(w, fx);
                        w.dirty = true;
                    }
                }
                ev::POINTER_WHEEL => {
                    let (cols, _) = Term::grid_for(conn.size(w.num).0, conn.size(w.num).1);
                    w.term.scroll_by(e.a as i32 * 3, cols);
                    w.dirty = true;
                }
                ev::FOCUS => {
                    w.focused = e.a == 1;
                    w.dirty = true;
                }
                ev::CLOSE_REQUEST => w.closing = true,
                _ => {}
            }
        }
        // the programs: their output, and whether they ended
        for w in wins.iter_mut() {
            let Some(job) = &w.job else { continue };
            let out = job.take_output();
            if !out.is_empty() {
                w.shell.job_output(&mut w.term, &out);
                w.dirty = true;
            }
            if let Ok(Some(status)) = job.child.try_wait() {
                let job = w.job.take().expect("there");
                let rest = job.take_output();
                w.shell.job_output(&mut w.term, &rest);
                job.finish();
                w.shell.job_done(&mut w.term, Some(status.0));
                w.dirty = true;
            }
        }
        // windows that close: their program is stopped first
        for w in wins.iter_mut().filter(|w| w.closing) {
            if let Some(job) = w.job.take() {
                let _ = job.child.kill();
                let _ = job.child.try_wait();
                job.finish();
            }
            conn.destroy(&mut t, w.num);
        }
        wins.retain(|w| !w.closing);
        if wins.is_empty() {
            return 0;
        }
        for w in wins.iter_mut().filter(|w| w.dirty) {
            draw(&mut conn, &mut t, w);
        }
        // with a program running, look at its output often; otherwise sleep until GlieseWM sends something
        let busy = wins.iter().any(|w| w.job.is_some());
        conn.wait(&mut t, if busy { 10 } else { 0 });
    }
}

program!("terminal", main);
