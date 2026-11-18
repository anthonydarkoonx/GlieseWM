//! The compositor on the development machine: an in-memory [`Host`], real protocol clients ([`gliese_client::Connection`] over
//! [`SimClient`]), a framebuffer, and PNG screenshots. For tests and pictures.
use crate::{Host, Input, Policy, Program, Rect, Server, ShmId, Task};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use std::collections::BTreeMap;
use std::string::String;

#[derive(Default)]
pub struct SimHost {
    objects: Vec<Option<Vec<u32>>>,
    /// (task, slot) → object.
    grants: BTreeMap<(Task, u32), ShmId>,
    next_slot: BTreeMap<Task, u32>,
    /// Task → program (a task not listed is its own program).
    programs: BTreeMap<Task, Program>,
    dead: Vec<Task>,
    pub now: u64,
    pub attentions: u32,
    pub wakes: u32,
    /// `POWER` calls (0 off, 1 restart).
    pub power_calls: Vec<u32>,
}

impl SimHost {
    /// Objects that exist (session pages and buffers).
    pub fn live_objects(&self) -> usize {
        self.objects.iter().flatten().count()
    }
    /// The slots a task holds.
    pub fn slots_of(&self, task: Task) -> usize {
        self.grants.keys().filter(|(t, _)| *t == task).count()
    }
}

impl Host for SimHost {
    fn shm_create(&mut self, pages: usize) -> Result<ShmId, i64> {
        let o = Some(vec![0u32; pages * 1024]);
        match self.objects.iter().position(|o| o.is_none()) {
            Some(i) => {
                self.objects[i] = o;
                Ok(i as ShmId)
            }
            None => {
                self.objects.push(o);
                Ok(self.objects.len() as ShmId - 1)
            }
        }
    }
    fn shm_grant(&mut self, shm: ShmId, task: Task) -> Result<u32, i64> {
        if self.objects.get(shm as usize).and_then(|o| o.as_ref()).is_none() {
            return Err(-2);
        }
        let n = self.next_slot.entry(task).or_insert(16);
        let slot = *n;
        *n += 1;
        self.grants.insert((task, slot), shm);
        Ok(slot)
    }
    fn shm_destroy(&mut self, shm: ShmId) {
        if let Some(o) = self.objects.get_mut(shm as usize) {
            *o = None;
        }
        self.grants.retain(|_, s| *s != shm);
    }
    fn shm(&mut self, shm: ShmId) -> Option<&mut [u32]> {
        self.objects.get_mut(shm as usize)?.as_deref_mut()
    }
    fn wake(&mut self, _shm: ShmId, _word: usize) {
        self.wakes += 1;
    }
    fn program_of(&self, task: Task) -> Program {
        self.programs.get(&task).copied().unwrap_or(task)
    }
    fn task_alive(&self, task: Task) -> bool {
        !self.dead.contains(&task)
    }
    fn now_ms(&self) -> u64 {
        self.now
    }
    fn attention(&mut self) {
        self.attentions += 1;
    }
    fn power(&mut self, what: u32) -> i64 {
        self.power_calls.push(what);
        0
    }
}

/// The compositor, its host, and the screen.
pub struct Sim {
    pub server: Server,
    pub host: SimHost,
    pub fb: Vec<u32>,
    pub w: i32,
    pub h: i32,
    /// What the last render presented.
    pub presented: Vec<Rect>,
}

impl Sim {
    pub fn new(w: i32, h: i32, policy: Box<dyn Policy>) -> Sim {
        Sim { server: Server::new(w, h, policy), host: SimHost::default(), fb: vec![0; (w * h) as usize], w, h, presented: Vec::new() }
    }
    /// The floating policy.
    pub fn floating(w: i32, h: i32) -> Sim {
        Sim::new(w, h, Box::new(crate::FloatingPolicy::default()))
    }
    /// A transport for task `task` (a client in it).
    pub fn client(&mut self, task: Task) -> SimClient<'_> {
        SimClient { sim: self, task }
    }
    /// Make `task` a thread of `program` (they share a session).
    pub fn set_program(&mut self, task: Task, program: Program) {
        self.host.programs.insert(task, program);
    }
    /// The task ends (as if it crashed); the compositor notices at its next tick.
    pub fn kill(&mut self, task: Task) {
        self.host.dead.push(task);
        self.host.grants.retain(|(t, _), _| *t != task);
    }
    /// Move the pointer to the bottom right corner, out of the way of pixel checks.
    pub fn park_pointer(&mut self) {
        let (w, h) = (self.w, self.h);
        self.input(Input::Motion { x: w - 1, y: h - 1 });
    }
    pub fn input(&mut self, e: Input) {
        self.server.input(&mut self.host, e);
    }
    pub fn tick(&mut self) {
        self.server.tick(&mut self.host);
    }
    pub fn advance(&mut self, ms: u64) {
        self.host.now += ms;
        self.tick();
    }
    /// Draw what changed.
    pub fn render(&mut self) -> &[Rect] {
        let w = self.w;
        self.presented = self.server.render(&mut self.host, &mut self.fb, w);
        &self.presented
    }
    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        self.fb[(y * self.w + x) as usize]
    }
    /// The screen as a PNG file.
    pub fn save_png(&self, path: &str) -> std::io::Result<()> {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, png(self.w as u32, self.h as u32, &self.fb))
    }
}

/// A client's transport in the simulator: calls go straight to the compositor, slots are the task's grants.
pub struct SimClient<'a> {
    pub sim: &'a mut Sim,
    pub task: Task,
}

impl gliese_client::Transport for SimClient<'_> {
    fn call(&mut self, w: [u64; 3]) -> [u64; 3] {
        let s = &mut *self.sim;
        if s.host.dead.contains(&self.task) {
            return [(-4i64) as u64, 0, 0];
        }
        s.server.request(&mut s.host, self.task, w)
    }
    fn shm(&mut self, slot: u32) -> Option<&mut [u32]> {
        let id = *self.sim.host.grants.get(&(self.task, slot))?;
        self.sim.host.shm(id)
    }
    fn unmap(&mut self, slot: u32) {
        self.sim.host.grants.remove(&(self.task, slot));
    }
    fn forget(&mut self, _slot: u32) {}
    fn wait(&mut self, _slot: u32, _word: usize, _value: u32, _timeout_ms: u64) {}
}

/// A PNG (RGB, 8 bits) of `0x00RRGGBB` pixels: zlib with stored blocks, so no compressor is needed.
pub fn png(w: u32, h: u32, px: &[u32]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0); // filter: none
        for &p in &px[y * w as usize..(y + 1) * w as usize] {
            raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    for (kind, data) in [(b"IHDR", ihdr), (b"IDAT", z), (b"IEND", Vec::new())] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_data = kind.to_vec();
        crc_data.extend_from_slice(&data);
        out.extend_from_slice(&crc_data);
        out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
    }
    out
}

fn crc32(d: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in d {
        c ^= b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
        }
    }
    !c
}

/// A test client: one connection, windows filled with a colour.
pub struct Painter {
    pub conn: gliese_client::Connection,
    pub task: Task,
}

impl Painter {
    pub fn connect(sim: &mut Sim, task: Task) -> Painter {
        let conn = gliese_client::Connection::connect(&mut sim.client(task)).expect("connects");
        Painter { conn, task }
    }
    /// A window filled with `color`, drawn once it is configured (events handled until then).
    pub fn window(&mut self, sim: &mut Sim, w: u32, h: u32, color: u32, title: &str) -> u8 {
        let t = &mut sim.client(self.task);
        let num = self.conn.create(t, w, h, None).expect("window");
        self.conn.set_title(t, num, title).expect("title");
        self.pump(sim, color);
        num
    }
    /// Handle every waiting event; each CONFIGURE is answered by filling the window with `color` and committing. Returns the
    /// events seen.
    pub fn pump(&mut self, sim: &mut Sim, color: u32) -> Vec<gliese_proto::Event> {
        let t = &mut sim.client(self.task);
        let mut seen = Vec::new();
        while let Some(e) = self.conn.next_event(t) {
            if e.kind == gliese_proto::ev::CONFIGURE {
                let num = e.window as u8;
                if let Some(px) = self.conn.back_buffer(t, num) {
                    px.fill(color);
                }
                self.conn.commit(t, num, None, false).expect("commit");
            }
            seen.push(e);
        }
        seen
    }
}

/// A title as `String` (for messages).
pub fn describe(sim: &Sim) -> String {
    let mut s = String::new();
    for (id, program, num, r, visible) in sim.server.windows() {
        s.push_str(&std::format!("[{id} p{program} #{num} {r:?} {}] ", if visible { "shown" } else { "hidden" }));
    }
    s
}
