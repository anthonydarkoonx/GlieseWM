//! GlieseWM end to end in the simulator (`PLAN.md` P4): tiling with exact gaps, binds from `gliese.conf`, workspaces, floating
//! and fullscreen, submaps, rules, mouse drags, the error bar.
use gliese_core::{AgentEvent, Gliese};
use gliese_proto::ev;
use gliese_server::sim::{describe, Painter, Sim};
use gliese_server::{Input, Look, Rect};

const SUPER: u16 = 125;
const SHIFT: u16 = 42;
const RETURN: u16 = 28;
const LEFT: u16 = 105;
const RIGHT: u16 = 106;
const ESC: u16 = 1;
const COLORS: [u32; 6] = [0xBF616A, 0xA3BE8C, 0x88C0D0, 0xEBCB8B, 0xB48EAD, 0xD08770];

fn key_name(k: &str) -> u16 {
    match k {
        "super" => SUPER,
        "shift" => SHIFT,
        "return" => RETURN,
        "left" => LEFT,
        "right" => RIGHT,
        "esc" => ESC,
        "1" => 2,
        "2" => 3,
        "f" => 33,
        "v" => 47,
        "q" => 16,
        "r" => 19,
        "a" => 30,
        "j" => 36,
        "equal" => 13,
        other => panic!("key {other}"),
    }
}

/// Press a chord like "super+shift+2": modifiers down, the key down and up, modifiers up.
fn chord(sim: &mut Sim, keys: &str) {
    let codes: Vec<u16> = keys.split('+').map(key_name).collect();
    for c in &codes {
        sim.input(Input::Key { code: *c, down: true, repeat: false });
    }
    for c in codes.iter().rev() {
        sim.input(Input::Key { code: *c, down: false, repeat: false });
    }
}

fn wm(cfg: &str) -> Sim {
    let cfg = gliese_config::parse("test.conf", cfg, None).unwrap_or_else(|e| panic!("{e:?}"));
    Sim::new(1024, 768, Box::new(Gliese::new(cfg)))
}

fn default_wm() -> Sim {
    Sim::new(1024, 768, Box::new(Gliese::with_default()))
}

/// Clients: one program each; every event handled (configures answered with the client's colour).
struct Clients(Vec<Painter>);

impl Clients {
    fn open(&mut self, sim: &mut Sim, title: &str) -> usize {
        let task = 100 + self.0.len() as u32;
        let mut p = Painter::connect(sim, task);
        p.window(sim, 400, 300, COLORS[self.0.len() % COLORS.len()], title);
        self.0.push(p);
        self.pump(sim);
        self.0.len() - 1
    }
    /// Let every client answer what it was sent.
    fn pump(&mut self, sim: &mut Sim) -> Vec<Vec<gliese_proto::Event>> {
        let mut all = Vec::new();
        for (i, p) in self.0.iter_mut().enumerate() {
            all.push(p.pump(sim, COLORS[i % COLORS.len()]));
        }
        all
    }
    fn task(&self, i: usize) -> u32 {
        self.0[i].task
    }
}

fn rect_of(sim: &Sim, task: u32) -> Rect {
    sim.server.windows().into_iter().find(|w| w.1 == task).map(|w| w.3).unwrap_or_else(|| panic!("no window of {task}: {}", describe(sim)))
}
fn visible(sim: &Sim, task: u32) -> bool {
    sim.server.windows().into_iter().find(|w| w.1 == task).is_some_and(|w| w.4)
}
fn focused_task(sim: &Sim) -> Option<u32> {
    let f = sim.server.focused()?;
    sim.server.windows().into_iter().find(|w| w.0 == f).map(|w| w.1)
}
/// What the window manager asked the desktop role to do (no desktop role connects in these tests, so it waits in the queue).
fn agent(sim: &mut Sim) -> Vec<AgentEvent> {
    sim.server.agent_queue().to_vec()
}

#[test]
fn three_windows_tile_with_exact_gaps() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    for t in ["a", "b", "c"] {
        c.open(&mut sim, t);
    }
    c.pump(&mut sim);
    sim.park_pointer();
    sim.render();
    // gaps_out 12, gaps_in 6, dwindle: a | (b / c)
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(12, 12, 497, 744));
    assert_eq!(rect_of(&sim, c.task(1)), Rect::new(515, 12, 497, 369));
    assert_eq!(rect_of(&sim, c.task(2)), Rect::new(515, 387, 497, 369));
    let look = Look::default();
    // the gap between a and b is background, exactly 6 pixels
    for x in 509..515 {
        assert_eq!(sim.pixel(x, 100), look.background, "gap at x {x}");
    }
    assert_eq!(sim.pixel(508, 100), look.inactive_border, "a's border");
    assert_eq!(sim.pixel(515, 100), look.inactive_border, "b's border");
    // the newest has the focus and the active border
    assert_eq!(focused_task(&sim), Some(c.task(2)));
    assert_eq!(sim.pixel(515, 500), look.active_border);
    // the clients were told their tile size (border 2 inside the tile) and that they are tiled
    assert_eq!(c.0[0].conn.size(0), (493, 740));
    assert_eq!(c.0[0].conn.state(0) & gliese_proto::state::TILED, gliese_proto::state::TILED);
    assert_eq!(sim.pixel(100, 100), COLORS[0], "a's own pixels");
    sim.save_png(concat!(env!("CARGO_TARGET_TMPDIR"), "/p4-three-tiled.png")).unwrap();
}

#[test]
fn closing_a_window_gives_its_space_back() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    for t in ["a", "b", "c"] {
        c.open(&mut sim, t);
    }
    // Super+Q asks the focused window (c) to close; the client does
    chord(&mut sim, "super+q");
    let ev = c.pump(&mut sim);
    assert!(ev[2].iter().any(|e| e.kind == ev::CLOSE_REQUEST), "killactive asks the program");
    let t2 = c.task(2);
    c.0[2].conn.destroy(&mut sim.client(t2), 0);
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(1)), Rect::new(515, 12, 497, 744), "b takes c's place");
    assert_eq!(focused_task(&sim), Some(c.task(1)), "the focus goes to the most recent window");
}

#[test]
fn super_enter_asks_for_a_terminal_and_bound_keys_stay_with_the_wm() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    chord(&mut sim, "super+return");
    assert_eq!(agent(&mut sim), vec![AgentEvent::Exec("terminal".into())]);
    let ev = c.pump(&mut sim);
    let keys: Vec<_> = ev[0].iter().filter(|e| e.kind == ev::KEY).map(|e| (e.a, e.b)).collect();
    // the window sees Super going down and up (a modifier), never Return
    assert!(!keys.iter().any(|k| k.0 == RETURN as u32), "{keys:?}");
    // an unbound key goes to the window
    chord(&mut sim, "a");
    assert!(c.pump(&mut sim)[0].iter().any(|e| e.kind == ev::KEY && e.a == 30 && e.d == 'a' as u32));
}

#[test]
fn focus_and_windows_move_by_direction() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    for t in ["a", "b"] {
        c.open(&mut sim, t);
    }
    assert_eq!(focused_task(&sim), Some(c.task(1)));
    chord(&mut sim, "super+left");
    assert_eq!(focused_task(&sim), Some(c.task(0)));
    chord(&mut sim, "super+left");
    assert_eq!(focused_task(&sim), Some(c.task(0)), "nothing further left");
    chord(&mut sim, "super+right");
    assert_eq!(focused_task(&sim), Some(c.task(1)));
    // swap b to the left
    chord(&mut sim, "super+shift+left");
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(1)).x, 12, "b is on the left now");
    assert_eq!(rect_of(&sim, c.task(0)).x, 515);
    assert_eq!(focused_task(&sim), Some(c.task(1)), "the focus moved with it");
}

#[test]
fn workspaces() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    c.open(&mut sim, "b");
    // move b to workspace 2: we follow it
    chord(&mut sim, "super+shift+2");
    c.pump(&mut sim);
    assert!(!visible(&sim, c.task(0)) && visible(&sim, c.task(1)), "{}", describe(&sim));
    assert_eq!(rect_of(&sim, c.task(1)), Rect::new(12, 12, 1000, 744), "alone on workspace 2: the whole area");
    assert_eq!(focused_task(&sim), Some(c.task(1)));
    // back to 1: a alone, re-tiled to the whole area
    chord(&mut sim, "super+1");
    c.pump(&mut sim);
    assert!(visible(&sim, c.task(0)) && !visible(&sim, c.task(1)));
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(12, 12, 1000, 744));
    assert_eq!(focused_task(&sim), Some(c.task(0)));
    // a new window opens on the workspace shown
    c.open(&mut sim, "c");
    assert!(visible(&sim, c.task(2)));
    // Super+Tab: the previous workspace
    chord(&mut sim, "super+2");
    assert!(visible(&sim, c.task(1)) && !visible(&sim, c.task(2)));
    sim.input(Input::Key { code: SUPER, down: true, repeat: false });
    sim.input(Input::Key { code: 15, down: true, repeat: false });
    sim.input(Input::Key { code: 15, down: false, repeat: false });
    sim.input(Input::Key { code: SUPER, down: false, repeat: false });
    assert!(visible(&sim, c.task(2)), "back on workspace 1");
}

#[test]
fn floating_and_fullscreen() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    c.open(&mut sim, "b");
    chord(&mut sim, "super+v"); // b floats
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(12, 12, 1000, 744), "a has the area to itself");
    let fb = rect_of(&sim, c.task(1));
    assert!(fb.w < 497 && fb.h < 744, "b floats, smaller: {fb:?}");
    assert_eq!(sim.server.windows().last().unwrap().1, c.task(1), "above the tiles");
    assert_eq!(c.0[1].conn.state(0) & gliese_proto::state::TILED, 0, "and is told it is not a tile");
    chord(&mut sim, "super+v"); // and tiles again
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(1)).w, 497);
    chord(&mut sim, "super+f");
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(1)), Rect::new(0, 0, 1024, 768));
    assert_eq!(c.0[1].conn.size(0), (1024, 768), "fullscreen: no border");
    chord(&mut sim, "super+f");
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(1)).w, 497);
}

#[test]
fn the_resize_submap() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    c.open(&mut sim, "b");
    chord(&mut sim, "super+left"); // focus a
    chord(&mut sim, "super+r");
    // Right, held: the first press and two repeats grow a by 20 each
    sim.input(Input::Key { code: RIGHT, down: true, repeat: false });
    sim.input(Input::Key { code: RIGHT, down: true, repeat: true });
    sim.input(Input::Key { code: RIGHT, down: true, repeat: true });
    sim.input(Input::Key { code: RIGHT, down: false, repeat: false });
    c.pump(&mut sim);
    let a = rect_of(&sim, c.task(0));
    assert!((555..=559).contains(&a.w), "a grew by about 60: {a:?}");
    // in the submap, Super+Left is not bound: it reaches the window instead of moving the focus
    chord(&mut sim, "esc");
    chord(&mut sim, "super+right");
    assert_eq!(focused_task(&sim), Some(c.task(1)), "out of the submap, the normal binds are back");
}

#[test]
fn rules_apply_once_the_title_is_known() {
    let mut sim = wm("windowrule = float, title:*Dialog*\nwindowrule = workspace 3 silent, title:mail\n");
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "editor");
    c.open(&mut sim, "Save Dialog");
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(12, 12, 1000, 744), "the dialog is not a tile");
    c.open(&mut sim, "mail");
    c.pump(&mut sim);
    assert!(!visible(&sim, c.task(2)), "sent to workspace 3, silently");
    assert!(visible(&sim, c.task(0)));
}

#[test]
fn mouse_drags() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    c.open(&mut sim, "b");
    // Super + left drag on a tile, dropped on the other: they swap
    sim.input(Input::Motion { x: 100, y: 100 });
    sim.input(Input::Key { code: SUPER, down: true, repeat: false });
    sim.input(Input::Button { button: 1, down: true });
    sim.input(Input::Motion { x: 700, y: 100 });
    sim.input(Input::Button { button: 1, down: false });
    sim.input(Input::Key { code: SUPER, down: false, repeat: false });
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(0)).x, 515, "a was dropped on b's place");
    // a floating window moves with the pointer
    chord(&mut sim, "super+v");
    c.pump(&mut sim);
    let before = rect_of(&sim, c.task(0));
    let (sx, sy) = (before.x + 20, before.y + 20);
    sim.input(Input::Motion { x: sx, y: sy });
    sim.input(Input::Key { code: SUPER, down: true, repeat: false });
    sim.input(Input::Button { button: 1, down: true });
    sim.input(Input::Motion { x: sx - 50, y: sy + 30 });
    sim.input(Input::Button { button: 1, down: false });
    sim.input(Input::Key { code: SUPER, down: false, repeat: false });
    let after = rect_of(&sim, c.task(0));
    assert_eq!((after.x, after.y), (before.x - 50, before.y + 30));
    // the clicks of a drag reach no window
    assert!(c.pump(&mut sim).iter().flatten().all(|e| e.kind != ev::POINTER_BUTTON));
}

#[test]
fn a_bad_config_keeps_the_old_one_and_shows_the_error_bar() {
    let mut sim = default_wm();
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    let load = |sim: &mut Sim, text: &str| sim.server.with_policy(&mut sim.host, |p, ctl| p.config_text(ctl, "gliese.conf", text));
    let r = load(&mut sim, "general {\n  gaps_in = 4\n  gap_out = 30\n}\n");
    assert_eq!(r, Err((3, 3, "unknown key `general:gap_out`".to_string())));
    assert_eq!(sim.server.error(), Some("gliese.conf:3:3: unknown key `general:gap_out`"));
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(12, 12, 1000, 744), "the old gaps stay");
    sim.park_pointer();
    sim.render();
    assert_eq!(sim.pixel(3, 3), gliese_server::BAR_BG, "the bar is drawn");
    sim.save_png(concat!(env!("CARGO_TARGET_TMPDIR"), "/p4-error-bar.png")).unwrap();
    // fixed: applied, and the bar goes
    assert_eq!(load(&mut sim, "general {\n  gaps_out = 30\n  border_size = 4\n}\nbind = SUPER, Q, killactive\n"), Ok(()));
    assert_eq!(sim.server.error(), None);
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(30, 30, 964, 708));
    assert_eq!(c.0[0].conn.size(0), (956, 700), "border 4");
    sim.render();
    assert_eq!(sim.pixel(3, 3), Look::default().background);
    // the new binds replaced the old: Super+Enter does nothing now
    let before = agent(&mut sim).len();
    chord(&mut sim, "super+return");
    assert_eq!(agent(&mut sim).len(), before);
}

#[test]
fn smart_gaps_setgaps_and_layouts() {
    let mut sim = wm("general {\n smart_gaps = true\n}\nbind = SUPER, equal, setgaps, +4 +8\nbind = SUPER, J, layout, master\n");
    let mut c = Clients(Vec::new());
    c.open(&mut sim, "a");
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(0, 0, 1024, 768), "one window, smart gaps: everything");
    assert_eq!(c.0[0].conn.size(0), (1024, 768), "and no border");
    c.open(&mut sim, "b");
    c.open(&mut sim, "c");
    assert_eq!(rect_of(&sim, c.task(0)).x, 12, "gaps again with more than one");
    chord(&mut sim, "super+equal");
    c.pump(&mut sim);
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(20, 20, 487, 728), "outer 20, inner 10");
    chord(&mut sim, "super+j");
    c.pump(&mut sim);
    // master: usable width 984 - 10 = 974, 55% = 536
    assert_eq!(rect_of(&sim, c.task(0)), Rect::new(20, 20, 536, 728));
    assert_eq!(rect_of(&sim, c.task(1)).x, 20 + 536 + 10);
}

#[test]
fn too_many_windows_float_instead_of_shrinking_to_nothing() {
    let mut sim = wm("misc {\n min_tile = 200\n}\n");
    let mut c = Clients(Vec::new());
    for i in 0..8 {
        c.open(&mut sim, &format!("w{i}"));
    }
    c.pump(&mut sim);
    let tiles: Vec<Rect> = (0..8).map(|i| rect_of(&sim, c.task(i))).collect();
    let floating = (0..8).filter(|i| c.0[*i].conn.state(0) & gliese_proto::state::TILED == 0).count();
    assert!(floating > 0, "some float");
    for (i, r) in tiles.iter().enumerate() {
        if c.0[i].conn.state(0) & gliese_proto::state::TILED != 0 {
            assert!(r.w >= 200 && r.h >= 200, "tile {i} {r:?} is at least min_tile");
        }
    }
}

#[test]
fn screenshot_master_and_floating() {
    let mut sim = wm("general {\n layout = master\n gaps_in = 10\n gaps_out = 20\n border_size = 3\n}\nbind = SUPER, V, togglefloating\n");
    let mut c = Clients(Vec::new());
    for t in ["a", "b", "c", "d"] {
        c.open(&mut sim, t);
    }
    chord(&mut sim, "super+v");
    c.pump(&mut sim);
    assert_eq!(c.0[3].conn.state(0) & gliese_proto::state::TILED, 0, "d floats");
    sim.input(Input::Motion { x: 300, y: 300 });
    sim.render();
    sim.save_png(concat!(env!("CARGO_TARGET_TMPDIR"), "/p4-master-floating.png")).unwrap();
}
