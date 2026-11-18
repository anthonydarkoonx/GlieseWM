//! GlieseWM on Metal OS (`PLAN.md` §2.1): [`OsHost`] (the compositor's `Host` on system calls), [`run`] (the screen task's loop),
//! and [`OsTransport`] (the client side, for programs).
//!
//! This is the one crate with `unsafe`: memory the kernel mapped at an address is turned into slices. That memory is shared with
//! other programs, which may write to it at any time; the compositor treats everything in it as untrusted input and only reads and
//! writes whole words, and a program reads what the compositor writes the same way.
//!
//! System calls are metal-os's (`docs/ABI.md`); the ones `metal-driver-rt` has no constant for are named here.
#![no_std]
extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use gliese_server::{Host, Input, Limits, Policy, Program, Rect, Server, ShmId, Task};
use metal_driver_rt::proto::gpu::{GPU_FLUSH2, GPU_INFO};
use metal_driver_rt::proto::input::{ev, INPUT_POLL};
use metal_driver_rt::{call, mem_map, svc, va, CAP_GRANT, CAP_REVOKE, CLOCK, IPC_RECV, IPC_REPLY, MEM_MAP, SLEEP, SLEEP_US, SYSINFO};

const WAIT_WORD: u64 = 32;
const WAKE_WORD: u64 = 33;
const SHM_CREATE: u64 = 37;
const SHM_INFO: u64 = 38;
const POWER: u64 = 42;
const E_AGAIN: i64 = -10;
/// What a program gets on its session page and pools: map and write, not the right to pass them on.
const R_MAP_WRITE: u64 = 8 | 16;

struct Mapping {
    slot: u64,
    addr: usize,
    words: usize,
}

/// A memory object the kernel mapped at `addr`, `words` 32-bit words long, as a slice.
///
/// # Safety
/// `addr` must be the address `MEM_MAP` returned for a writable mapping of at least `words` words that stays mapped for `'a`, and
/// no other Rust reference to that memory may be live during `'a`.
unsafe fn words<'a>(addr: usize, words: usize) -> &'a mut [u32] {
    // SAFETY: the caller's contract: a live, writable, word-aligned (page-aligned) mapping of that length, not otherwise borrowed.
    unsafe { core::slice::from_raw_parts_mut(addr as *mut u32, words) }
}

/// The compositor's `Host` on system calls. A `ShmId` is an index into this table (a capability slot can be reused; this is not
/// until the object is destroyed).
pub struct OsHost {
    objects: Vec<Option<Mapping>>,
    /// The right to switch the machine off or restart it (`R_POWER`), if the screen task was given one.
    pub power: Option<u64>,
}

impl OsHost {
    pub fn new(power: Option<u64>) -> OsHost {
        OsHost { objects: Vec::new(), power }
    }
}

impl Host for OsHost {
    fn shm_create(&mut self, pages: usize) -> Result<ShmId, i64> {
        let slot = svc(SHM_CREATE, pages as u64, 0, 0, 0, 0)[0] as i64;
        if slot < 0 {
            return Err(slot);
        }
        // address 0: the kernel chooses (a random place, with an unmapped page after it)
        let addr = svc(MEM_MAP, slot as u64, 0, 1, 0, 0)[0] as i64;
        if addr <= 0 {
            svc(CAP_REVOKE, slot as u64, 1, 0, 0, 0);
            return Err(if addr < 0 { addr } else { gliese_proto::E_NOMEM });
        }
        let m = Mapping { slot: slot as u64, addr: addr as usize, words: pages * 1024 };
        match self.objects.iter().position(|o| o.is_none()) {
            Some(i) => {
                self.objects[i] = Some(m);
                Ok(i as ShmId)
            }
            None => {
                self.objects.push(Some(m));
                Ok(self.objects.len() as ShmId - 1)
            }
        }
    }

    fn shm_grant(&mut self, shm: ShmId, task: Task) -> Result<u32, i64> {
        let m = self.objects.get(shm as usize).and_then(|o| o.as_ref()).ok_or(gliese_proto::E_BADWIN)?;
        let r = svc(CAP_GRANT, m.slot, task as u64, R_MAP_WRITE, 0, 0)[0] as i64;
        if r < 0 {
            Err(r)
        } else {
            Ok(r as u32)
        }
    }

    fn shm_destroy(&mut self, shm: ShmId) {
        if let Some(m) = self.objects.get_mut(shm as usize).and_then(|o| o.take()) {
            // flag 1: the object itself goes, with every copy and every mapping of it (a program still touching it faults, alone)
            svc(CAP_REVOKE, m.slot, 1, 0, 0, 0);
        }
    }

    fn shm(&mut self, shm: ShmId) -> Option<&mut [u32]> {
        let m = self.objects.get(shm as usize)?.as_ref()?;
        // SAFETY: `addr` is the writable mapping MEM_MAP made for this object, `words` long; it stays mapped until `shm_destroy`
        // takes the entry out of the table, which needs `&mut self`, so the slice (borrowing `self`) cannot outlive it, and no other
        // slice of it can be live at the same time. Programs write to the same memory concurrently; the compositor only does word
        // reads and writes on it and checks what it reads.
        Some(unsafe { words(m.addr, m.words) })
    }

    fn wake(&mut self, shm: ShmId, word: usize) {
        if let Some(m) = self.objects.get(shm as usize).and_then(|o| o.as_ref()) {
            svc(WAKE_WORD, (m.addr + word * 4) as u64, u32::MAX as u64, 0, 0, 0);
        }
    }

    fn program_of(&self, task: Task) -> Program {
        // SYSINFO page 5: (session page, generation, the program the task belongs to, its parent)
        let r = svc(SYSINFO, 5, task as u64, 0, 0, 0);
        if r[0] as i64 == 0 {
            r[3] as Program
        } else {
            task
        }
    }

    fn task_alive(&self, task: Task) -> bool {
        // SYSINFO page 3: state 0 = free, 7 = dead
        let r = svc(SYSINFO, 3, task as u64, 0, 0, 0);
        r[0] as i64 == 0 && !matches!(r[3], 0 | 7)
    }

    fn now_ms(&self) -> u64 {
        svc(CLOCK, 0, 0, 0, 0, 0)[0] / 1_000_000
    }

    fn attention(&mut self) {
        metal_driver_rt::println!("[gliese] attention key");
    }

    fn power(&mut self, what: u32) -> i64 {
        // POWER(admin, op): does not return when it works
        match self.power {
            Some(slot) => svc(POWER, slot, what as u64, 0, 0, 0)[0] as i64,
            None => -1,
        }
    }
}

/// Where the screen task's capabilities are (the desktop contract's arguments).
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub display: u64,
    pub gpu: u64,
    pub input: u64,
    pub framebuffer: u64,
    pub fb_handle: usize,
    pub manager: Option<u32>,
    pub power: Option<u64>,
}

/// The compositor on the machine, one step at a time.
pub struct Os {
    pub server: Server,
    pub host: OsHost,
    cfg: Config,
    fb: &'static mut [u32],
    w: i32,
    idle_since: u64,
    last_tick: u64,
}

/// Limits on Metal OS: every session page and pool is a capability slot of the screen task (32 in all, about half free) and one
/// of its 64 mappings, so sessions are few; windows are not limited by that (they live in pools).
pub fn os_limits() -> Limits {
    Limits { sessions: 8, windows_total: 256, buffer_pages: 32 * 256 }
}

impl Os {
    pub fn new(cfg: Config, policy: Box<dyn Policy>) -> Os {
        let (st, info) = call(cfg.gpu, [GPU_INFO, 0, 0]);
        let (w, h) = if st == 0 && info[0] != 0 { ((info[0] & 0xFFFF_FFFF) as i32, (info[0] >> 32) as i32) } else { (1024, 768) };
        if mem_map(cfg.framebuffer, cfg.fb_handle, true) != 0 {
            metal_driver_rt::println!("[gliese] cannot map the framebuffer");
            metal_driver_rt::exit(1);
        }
        // SAFETY: the framebuffer capability maps `w * h` words (the display driver made it W * H * 4 bytes) at `va(fb_handle)`,
        // writable, for the life of this task; this is the only slice of it ever made (one `Os` per screen task, never dropped:
        // `run` does not return).
        let fb: &'static mut [u32] = unsafe { words(va(cfg.fb_handle), (w * h) as usize) };
        let mut server = Server::new(w, h, policy);
        server.set_limits(os_limits());
        if let Some(m) = cfg.manager {
            server.set_manager(m);
        }
        metal_driver_rt::println!("[gliese] up: {w} x {h}");
        Os { server, host: OsHost::new(cfg.power), cfg, fb, w, idle_since: 0, last_tick: 0 }
    }

    /// Serve what is waiting (requests, input, timers) and present what changed. Returns whether anything happened.
    pub fn step(&mut self) -> bool {
        let mut busy = false;
        // requests: everything queued, each answered at once
        loop {
            let r = svc(IPC_RECV, self.cfg.display, 1, 0, 0, 0);
            let st = r[0] as i64;
            if st == E_AGAIN || st < 0 {
                break;
            }
            busy = true;
            let reply = self.server.request(&mut self.host, r[1] as Task, [r[2], r[3], r[4]]);
            if r[5] != 0 {
                svc(IPC_REPLY, reply[0], reply[1], reply[2], 0, 0);
            }
        }
        // input: up to two events a call, until the driver has none
        loop {
            let (st, rep) = call(self.cfg.input, [INPUT_POLL, 0, 0]);
            let n = if st == 0 { (rep[0] & 0xFF) as usize } else { 0 };
            if n == 0 {
                break;
            }
            busy = true;
            let (w, h) = self.server.screen();
            for e in rep[1..1 + n.min(2)].iter() {
                if let Some(i) = decode_input(*e, w, h) {
                    self.server.input(&mut self.host, i);
                }
            }
        }
        // dead programs and the window manager's timers, every 20 ms
        let now = self.host.now_ms();
        if now.saturating_sub(self.last_tick) >= 20 {
            self.last_tick = now;
            self.server.tick(&mut self.host);
        }
        if self.server.needs_render() {
            let rects = self.server.render(&mut self.host, self.fb, self.w);
            present(self.cfg.gpu, &rects);
            busy = true;
        }
        busy
    }

    /// Wait before the next step: briefly while things happen, longer once nothing has for a while.
    pub fn pause(&mut self, busy: bool) {
        let now = self.host.now_ms();
        if busy {
            self.idle_since = now;
            svc(SLEEP_US, 500, 0, 0, 0, 0);
        } else if now.saturating_sub(self.idle_since) < 250 {
            svc(SLEEP_US, 2000, 0, 0, 0, 0);
        } else {
            svc(SLEEP, 1, 0, 0, 0, 0);
        }
    }
}

/// The screen task as GlieseWM: never returns.
pub fn run(cfg: Config, policy: Box<dyn Policy>) -> ! {
    let mut os = Os::new(cfg, policy);
    loop {
        let busy = os.step();
        os.pause(busy);
    }
}

/// One word of `INPUT_POLL`'s reply as an [`Input`] (the pointer from the device's 0..=32767 range to pixels).
pub fn decode_input(e: u64, w: i32, h: i32) -> Option<Input> {
    let kind = e & 0xFF;
    let code = ((e >> 8) & 0xFFFF) as u16;
    let value = (e >> 32) as u32 as i32;
    match kind {
        ev::KEY => Some(Input::Key { code, down: value != 0, repeat: value == 2 }),
        ev::MOVE => {
            let (ax, ay) = ((value as u32 & 0xFFFF) as i32, ((value as u32) >> 16) as i32);
            Some(Input::Motion { x: ax * (w - 1) / 32767, y: ay * (h - 1) / 32767 })
        }
        ev::BUTTON => Some(Input::Button { button: code as u8, down: value != 0 }),
        ev::WHEEL => Some(Input::Wheel { clicks: value }),
        _ => None,
    }
}

/// Present changed rectangles, two per `GPU_FLUSH2`.
fn present(gpu: u64, rects: &[Rect]) {
    let word = |r: &Rect| (r.x as u64 & 0xFFFF) | (r.y as u64 & 0xFFFF) << 16 | (r.w as u64 & 0xFFFF) << 32 | (r.h as u64 & 0xFFFF) << 48;
    for pair in rects.chunks(2) {
        call(gpu, [GPU_FLUSH2, word(&pair[0]), pair.get(1).map_or(0, word)]);
    }
}

// ------------------------------------------------------------------------------------------------------------------------------
/// The display endpoint's slot in a program with `permission = display` (metal-os `docs/PROGRAMS.md`).
pub const DISPLAY_SLOT: u64 = 2;

/// A program's side: `IPC_CALL` on the display endpoint, and the session page and pools mapped where the kernel chooses.
pub struct OsTransport {
    pub display: u64,
    maps: Vec<Mapping>,
}

impl OsTransport {
    pub fn new(display_slot: u64) -> OsTransport {
        OsTransport { display: display_slot, maps: Vec::new() }
    }
}

impl Default for OsTransport {
    fn default() -> Self {
        OsTransport::new(DISPLAY_SLOT)
    }
}

impl gliese_client::Transport for OsTransport {
    fn call(&mut self, w: [u64; 3]) -> [u64; 3] {
        let (st, r) = call(self.display, w);
        if st < 0 {
            [st as u64, 0, 0]
        } else {
            r
        }
    }

    fn shm(&mut self, slot: u32) -> Option<&mut [u32]> {
        if !self.maps.iter().any(|m| m.slot == slot as u64) {
            let pages = svc(SHM_INFO, slot as u64, 0, 0, 0, 0)[0] as i64;
            if pages <= 0 {
                return None;
            }
            let addr = svc(MEM_MAP, slot as u64, 0, 1, 0, 0)[0] as i64;
            if addr <= 0 {
                return None;
            }
            self.maps.push(Mapping { slot: slot as u64, addr: addr as usize, words: (pages as u64 & 0xFFFF_FFFF) as usize * 1024 });
        }
        let m = self.maps.iter().find(|m| m.slot == slot as u64)?;
        // SAFETY: the kernel mapped `words` words at `addr` for the capability in this slot, writable; the entry is dropped (`unmap`,
        // `forget`, both `&mut self`) before the slot is given back or found to hold another object, so the slice, which borrows
        // `self`, cannot outlive the mapping. The compositor writes to the same memory concurrently, word by word.
        Some(unsafe { words(m.addr, m.words) })
    }

    fn unmap(&mut self, slot: u32) {
        self.forget(slot);
        // our copy goes, and its mapping (the compositor's object is not affected)
        svc(CAP_REVOKE, slot as u64, 1, 0, 0, 0);
    }

    fn forget(&mut self, slot: u32) {
        self.maps.retain(|m| m.slot != slot as u64);
    }

    fn wait(&mut self, slot: u32, word: usize, value: u32, timeout_ms: u64) {
        let Some(addr) = self.maps.iter().find(|m| m.slot == slot as u64).map(|m| m.addr + word * 4) else { return };
        // WAIT_WORD takes ticks (100 a second); 0 = no limit
        let ticks = if timeout_ms == 0 { 0 } else { timeout_ms.div_ceil(10) };
        svc(WAIT_WORD, addr as u64, value as u64, ticks, 0, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_words() {
        let pack = |k: u64, code: u16, v: i32| k | (code as u64) << 8 | (v as u32 as u64) << 32;
        assert_eq!(decode_input(pack(ev::KEY, 30, 1), 1024, 768), Some(Input::Key { code: 30, down: true, repeat: false }));
        assert_eq!(decode_input(pack(ev::KEY, 30, 2), 1024, 768), Some(Input::Key { code: 30, down: true, repeat: true }));
        assert_eq!(decode_input(pack(ev::MOVE, 0, 32767 | 32767 << 16), 1024, 768), Some(Input::Motion { x: 1023, y: 767 }));
        assert_eq!(decode_input(pack(ev::MOVE, 0, 0), 1024, 768), Some(Input::Motion { x: 0, y: 0 }));
        assert_eq!(decode_input(pack(ev::BUTTON, 2, 0), 1024, 768), Some(Input::Button { button: 2, down: false }));
        assert_eq!(decode_input(pack(ev::WHEEL, 0, -3), 1024, 768), Some(Input::Wheel { clicks: -3 }));
        assert_eq!(decode_input(pack(99, 0, 0), 1024, 768), None);
    }
}
