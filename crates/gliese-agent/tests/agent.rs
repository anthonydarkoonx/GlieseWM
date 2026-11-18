//! `gliesed` end to end in the simulator (`PLAN.md` P6): the compositor with the window manager, the program manager's role word,
//! the agent with a fake file and program manager, and a terminal that gets a second window.
use gliese_agent::{Agent, Env, Refused, Start};
use gliese_client::Connection;
use gliese_core::Gliese;
use gliese_proto::{self as p, ev, op};
use gliese_server::sim::{Painter, Sim};
use gliese_server::{Input, Rect};

const MANAGER: u32 = 3;
const GLIESED: u32 = 50;
const SUPER: u16 = 125;

#[derive(Default)]
struct FakeEnv {
    file: Option<Vec<u8>>,
    now: u64,
    installed: Vec<&'static str>,
    running: Vec<(String, u32)>,
    next_task: u32,
    log: Vec<String>,
}

impl Env for FakeEnv {
    fn read_config(&mut self) -> Option<Vec<u8>> {
        self.file.clone()
    }
    fn start(&mut self, id: &str) -> Start {
        if let Some((_, t)) = self.running.iter().find(|(r, _)| r == id) {
            return Start::Running(*t);
        }
        if !self.installed.contains(&id) {
            return Start::Failed;
        }
        let t = 60 + self.next_task;
        self.next_task += 1;
        self.running.push((id.into(), t));
        Start::Started(t)
    }
    fn now_ms(&mut self) -> u64 {
        self.now
    }
    fn log(&mut self, line: &str) {
        self.log.push(line.into());
    }
}

/// The desktop as it comes up: the compositor, the manager naming gliesed the desktop role, gliesed connected and its config loaded.
fn desktop(file: Option<&str>) -> (Sim, Agent, FakeEnv) {
    let mut sim = Sim::new(1024, 768, Box::new(Gliese::with_default()));
    sim.server.set_manager(MANAGER);
    // progmgr's word, as it sends it right after starting a `role = desktop` program
    let r = gliese_client::Transport::call(&mut sim.client(MANAGER), [97 | 1 << 24, GLIESED as u64, 0]);
    assert_eq!(r[0], 0);
    let mut env = FakeEnv { file: file.map(|f| f.as_bytes().to_vec()), installed: vec!["terminal", "files"], ..Default::default() };
    let mut agent = Agent::connect(&mut sim.client(GLIESED)).expect("gliesed connects");
    agent.load(&mut sim.client(GLIESED), &mut env).expect("the config loads");
    (sim, agent, env)
}

fn chord(sim: &mut Sim, codes: &[u16]) {
    for c in codes {
        sim.input(Input::Key { code: *c, down: true, repeat: false });
    }
    for c in codes.iter().rev() {
        sim.input(Input::Key { code: *c, down: false, repeat: false });
    }
}

fn rect_of(sim: &Sim, task: u32, num: u8) -> Rect {
    sim.server.windows().into_iter().find(|w| w.1 == task && w.2 == num).map(|w| w.3).expect("the window")
}

#[test]
fn the_users_config_is_applied_and_a_broken_one_is_refused() {
    let (mut sim, mut agent, mut env) = desktop(Some("general {\n gaps_out = 30\n}\nbind = SUPER, Return, exec, terminal\n"));
    let mut t = Painter::connect(&mut sim, 70);
    t.window(&mut sim, 300, 200, 0x336699, "w");
    assert_eq!(rect_of(&sim, 70, 0), Rect::new(30, 30, 964, 708), "the file's gaps");
    // the user saves a mistake: auto_reload picks it up within a second
    env.file = Some(b"general {\n gaps_out = 30\n gaps_in = lots\n}\n".to_vec());
    env.now += 400;
    agent.step(&mut sim.client(GLIESED), &mut env);
    assert_eq!(sim.server.error(), None, "not looked at yet");
    env.now += 600;
    agent.step(&mut sim.client(GLIESED), &mut env);
    assert_eq!(sim.server.error(), Some("gliese.conf:3:12: general:gaps_in: expected a whole number"));
    assert!(env.log.iter().any(|l| l.contains("gliese.conf:3:12")), "{:?}", env.log);
    t.pump(&mut sim, 0x336699);
    assert_eq!(rect_of(&sim, 70, 0), Rect::new(30, 30, 964, 708), "the old config is still in force");
    // fixed
    env.file = Some(b"general {\n gaps_out = 8\n}\n".to_vec());
    env.now += 1000;
    agent.step(&mut sim.client(GLIESED), &mut env);
    t.pump(&mut sim, 0x336699);
    assert_eq!(sim.server.error(), None);
    assert_eq!(rect_of(&sim, 70, 0), Rect::new(8, 8, 1008, 752));
    // an unchanged file is not uploaded again (nothing to see, but nothing breaks either)
    env.now += 5000;
    assert!(!agent.step(&mut sim.client(GLIESED), &mut env));
}

#[test]
fn no_file_means_the_default() {
    let (mut sim, mut agent, mut env) = desktop(None);
    // the default binds Super+Enter
    chord(&mut sim, &[SUPER, 28]);
    agent.step(&mut sim.client(GLIESED), &mut env);
    assert_eq!(env.running.len(), 1, "the terminal was started");
    // a file appears: it is used; deleted again: back to the default
    env.file = Some(b"general:gaps_out = 40\n".to_vec());
    env.now += 1000;
    agent.step(&mut sim.client(GLIESED), &mut env);
    let mut w = Painter::connect(&mut sim, 71);
    w.window(&mut sim, 100, 100, 0x111111, "w");
    assert_eq!(rect_of(&sim, 71, 0).x, 40);
    env.file = None;
    env.now += 1000;
    agent.step(&mut sim.client(GLIESED), &mut env);
    w.pump(&mut sim, 0x111111);
    assert_eq!(rect_of(&sim, 71, 0).x, 12, "the default's gaps");
}

#[test]
fn super_enter_starts_a_terminal_then_asks_it_for_more_windows() {
    let (mut sim, mut agent, mut env) = desktop(None);
    chord(&mut sim, &[SUPER, 28]);
    agent.step(&mut sim.client(GLIESED), &mut env);
    assert_eq!(env.running, vec![("terminal".to_string(), 60)]);
    // the terminal comes up (task 60) with its first window
    let mut term = Painter::connect(&mut sim, 60);
    term.window(&mut sim, 640, 400, 0x202020, "terminal");
    // Super+Enter again: it runs, so it is asked for another window
    chord(&mut sim, &[SUPER, 28]);
    agent.step(&mut sim.client(GLIESED), &mut env);
    assert_eq!(env.running.len(), 1, "not started twice");
    let seen = term.pump(&mut sim, 0x202020);
    assert!(seen.iter().any(|e| e.kind == ev::NEW_WINDOW), "the terminal is asked for a new window");
    // it opens one: two tiles
    term.window(&mut sim, 640, 400, 0x202020, "terminal 2");
    term.pump(&mut sim, 0x202020);
    assert_eq!(rect_of(&sim, 60, 0), Rect::new(12, 12, 497, 744));
    assert_eq!(rect_of(&sim, 60, 1), Rect::new(515, 12, 497, 744));
    // a program that is not installed: logged, nothing else
    chord(&mut sim, &[SUPER, 18]); // Super+E is not bound by default: nothing
    agent.exec(&mut sim.client(GLIESED), &mut env, "nothere");
    assert!(env.log.iter().any(|l| l.contains("cannot start `nothere`")));
}

#[test]
fn requests_wait_for_gliesed_and_exec_once_runs_once() {
    let mut sim = Sim::new(1024, 768, Box::new(Gliese::with_default()));
    sim.server.set_manager(MANAGER);
    // Super+Enter before gliesed is there: kept
    chord(&mut sim, &[SUPER, 28]);
    gliese_client::Transport::call(&mut sim.client(MANAGER), [97 | 1 << 24, GLIESED as u64, 0]);
    let mut env = FakeEnv { file: Some(b"exec-once = files\nbind = SUPER, Return, exec, terminal\n".to_vec()), installed: vec!["terminal", "files"], ..Default::default() };
    let mut agent = Agent::connect(&mut sim.client(GLIESED)).unwrap();
    agent.load(&mut sim.client(GLIESED), &mut env).unwrap();
    agent.step(&mut sim.client(GLIESED), &mut env);
    let ids: Vec<&str> = env.running.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["files", "terminal"], "exec-once at the first load, then the Super+Enter that waited");
    // a reload does not start exec-once programs again
    env.file = Some(b"exec-once = files\nexec-once = terminal\n".to_vec());
    env.now += 1000;
    agent.step(&mut sim.client(GLIESED), &mut env);
    assert_eq!(env.running.len(), 2);
}

#[test]
fn the_reload_bind_and_any_programs_reload_request() {
    let (mut sim, mut agent, mut env) = desktop(Some("misc:auto_reload = false\nbind = SUPER SHIFT, C, reload\n"));
    env.file = Some(b"misc:auto_reload = false\nbind = SUPER SHIFT, C, reload\ngeneral:gaps_out = 50\n".to_vec());
    env.now += 5000;
    agent.step(&mut sim.client(GLIESED), &mut env);
    let mut w = Painter::connect(&mut sim, 71);
    w.window(&mut sim, 100, 100, 0x111111, "w");
    assert_eq!(rect_of(&sim, 71, 0).x, 12, "auto_reload is off: the change is not seen");
    chord(&mut sim, &[SUPER, 42, 46]);
    agent.step(&mut sim.client(GLIESED), &mut env);
    w.pump(&mut sim, 0x111111);
    assert_eq!(rect_of(&sim, 71, 0).x, 50, "Super+Shift+C reloaded it");
    // any program may ask for a reload (the terminal's `reload`)
    env.file = Some(b"misc:auto_reload = false\ngeneral:gaps_out = 20\n".to_vec());
    assert_eq!(gliese_client::Transport::call(&mut sim.client(71), [p::w0(op::GL_RELOAD, 0, 0), 0, 0])[0], 0);
    agent.step(&mut sim.client(GLIESED), &mut env);
    w.pump(&mut sim, 0x111111);
    assert_eq!(rect_of(&sim, 71, 0).x, 20);
}

#[test]
fn only_the_desktop_role_may_upload() {
    let (mut sim, _agent, _env) = desktop(None);
    let mut other = Connection::connect(&mut sim.client(80)).unwrap();
    let _ = other.create(&mut sim.client(80), 10, 10, None);
    for o in [op::GL_CONFIG_BEGIN, op::GL_CONFIG_CHUNK, op::GL_CONFIG_END, op::GL_NEW_WINDOW, op::GL_PROGRAM_NAME] {
        assert_eq!(gliese_client::Transport::call(&mut sim.client(80), [p::w0(o, 0, 0), 60, 0])[0] as i64, p::E_PERM, "op {o}");
    }
}

#[test]
fn program_rules_use_the_names_gliesed_reports() {
    let (mut sim, mut agent, mut env) = desktop(Some("windowrule = float, program:terminal\nbind = SUPER, Return, exec, terminal\n"));
    let mut other = Painter::connect(&mut sim, 71);
    other.window(&mut sim, 300, 200, 0x111111, "other");
    chord(&mut sim, &[SUPER, 28]);
    agent.step(&mut sim.client(GLIESED), &mut env);
    let mut term = Painter::connect(&mut sim, 60);
    term.window(&mut sim, 300, 200, 0x202020, "terminal");
    other.pump(&mut sim, 0x111111);
    assert_eq!(rect_of(&sim, 71, 0), Rect::new(12, 12, 1000, 744), "the terminal floats: the other window keeps the whole area");
    assert_eq!(term.conn.state(0) & p::state::TILED, 0);
}

#[test]
fn exit_switches_off_through_the_compositor() {
    let (mut sim, _agent, _env) = desktop(Some("bind = SUPER SHIFT, E, exit\n"));
    chord(&mut sim, &[SUPER, 42, 18]);
    assert_eq!(sim.host.power_calls, vec![0]);
}

#[test]
fn a_huge_file_is_refused_before_upload() {
    let (mut sim, mut agent, _env) = desktop(None);
    let big = vec![b'#'; p::MAX_CONFIG_BYTES + 1];
    let r = agent.upload(&mut sim.client(GLIESED), &big);
    assert!(matches!(r, Err(Refused { ref msg, .. }) if msg.contains("too big")));
    // a big but allowed file (many chunks) goes through
    let mut ok = String::new();
    while ok.len() < 20_000 {
        ok.push_str("# a comment line to make the file long\n");
    }
    ok.push_str("general:gaps_out = 3\n");
    assert_eq!(agent.upload(&mut sim.client(GLIESED), ok.as_bytes()), Ok(()));
}
