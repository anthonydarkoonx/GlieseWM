//! gliesed: GlieseWM's desktop-role program, with no window (`gliese-agent` has what it does). The program manager starts it at
//! boot (`os/defaults`) and tells the compositor it is the desktop role (`role = desktop` in its manifest), which also gives it the
//! control endpoint (slot 3) to start programs with; `permission = files` gives it its own `/data`, where `gliese.conf` lives.
#![no_std]
#![no_main]
extern crate alloc;

use alloc::vec::Vec;
use gliese_agent::{Agent, Env, Start, CONFIG_PATH};
use gliese_os::OsTransport;
use metal_programs::*;

/// The program manager's control endpoint, and its ops (metal-os `docs/PROGRAMS.md`, "Control protocol").
const CTL: u64 = 3;
const OP_START: u64 = 3;
const OP_STATE: u64 = 5;
/// START's status for "it runs already".
const E_STATE: u64 = 3;

/// A program id as the two words the protocol carries (up to 16 bytes, NUL padded).
fn pack(id: &str) -> (u64, u64) {
    let mut b = [0u8; 16];
    let n = id.len().min(16);
    b[..n].copy_from_slice(&id.as_bytes()[..n]);
    let word = |s: &[u8]| s.iter().rev().fold(0u64, |v, x| v << 8 | *x as u64);
    (word(&b[..8]), word(&b[8..]))
}

struct OsEnv;

impl Env for OsEnv {
    fn read_config(&mut self) -> Option<Vec<u8>> {
        fs::read(CONFIG_PATH).ok()
    }

    fn start(&mut self, id: &str) -> Start {
        let (a, b) = pack(id);
        let (st, r) = call(CTL, [OP_START, a, b]);
        match (st, r[0]) {
            // the reply: (status, board entry, the new task)
            (0, 0) => Start::Started(r[2] as u32),
            (0, E_STATE) => {
                // already running: STATE says which task (bits 32.. of the first word)
                let (st, s) = call(CTL, [OP_STATE, a, b]);
                if st == 0 && s[0] == 0 && (s[1] >> 24) & 1 == 1 {
                    Start::Running((s[1] >> 32) as u32)
                } else {
                    Start::Failed
                }
            }
            _ => Start::Failed,
        }
    }

    fn now_ms(&mut self) -> u64 {
        ticks() * 10
    }

    fn log(&mut self, line: &str) {
        println!("{line}");
    }
}

fn main(_c: &Ctx) -> u64 {
    let mut t = OsTransport::default();
    let mut env = OsEnv;
    let mut agent = match Agent::connect(&mut t) {
        Ok(a) => a,
        Err(e) => {
            println!("[gliesed] cannot reach the compositor ({e})");
            return 1;
        }
    };
    match agent.load(&mut t, &mut env) {
        Ok(()) => println!("[gliesed] up, config loaded"),
        Err(e) => println!("[gliesed] up; gliese.conf:{}:{}: {} (the built-in config stays)", e.line, e.col, e.msg),
    }
    loop {
        if !agent.step(&mut t, &mut env) {
            agent.wait(&mut t);
        }
    }
}

program!("gliesed", main);
