//! The compositor end to end, with real protocol clients in the simulator (`PLAN.md` P3): a client connects, opens a window,
//! draws, is resized, closes; a dead client is cleaned up; input goes where it should; the limits hold.
use gliese_client::Connection;
use gliese_proto::{self as p, ev, keymap::mods, op, state, want};
use gliese_server::sim::{describe, Painter, Sim};
use gliese_server::{Input, Limits, Look, Rect};

const RED: u32 = 0xCC2222;
const GREEN: u32 = 0x22AA44;
const BLUE: u32 = 0x2255CC;

fn look() -> Look {
    Look::default()
}

/// The outer rectangle of the window `num` of `program`.
fn outer(sim: &Sim, program: u32, num: u8) -> Rect {
    sim.server.windows().into_iter().find(|w| w.1 == program && w.2 == num).map(|w| w.3).unwrap_or_else(|| panic!("no window {program}/{num}: {}", describe(sim)))
}

#[test]
fn a_client_opens_draws_and_closes_a_window() {
    let mut sim = Sim::floating(800, 600);
    sim.render();
    assert_eq!(sim.pixel(10, 10), look().background);
    let mut a = Painter::connect(&mut sim, 7);
    let n = a.window(&mut sim, 300, 200, RED, "red");
    sim.render();
    let o = outer(&sim, 7, n);
    // centred, border of 2 around 300 x 200
    assert_eq!((o.w, o.h), (304, 204));
    assert_eq!((o.x, o.y), ((800 - 304) / 2, (600 - 204) / 2));
    let (cx, cy) = (o.x + 150, o.y + 100);
    assert_eq!(sim.pixel(cx, cy), RED, "the client's pixels are on screen");
    assert_eq!(sim.pixel(o.x, o.y + 50), look().active_border, "the focused window has the active border");
    assert_eq!(sim.pixel(o.x - 1, o.y + 50), look().background);
    // draw again, only a part
    {
        let t = &mut sim.client(7);
        let px = a.conn.back_buffer(t, n).unwrap();
        px.fill(RED);
        px[..300].fill(GREEN); // the first row
        a.conn.commit(t, n, Some((0, 0, 300, 1)), false).unwrap();
    }
    let presented = sim.render().to_vec();
    assert_eq!(sim.pixel(o.x + 2 + 10, o.y + 2), GREEN);
    assert!(presented.iter().all(|r| r.h <= 19 + 1 || r.y == o.y + 2), "only the damaged row (and maybe the pointer) was redrawn: {presented:?}");
    // close it
    a.conn.destroy(&mut sim.client(7), n);
    sim.render();
    assert_eq!(sim.pixel(cx, cy), look().background, "gone from the screen");
    assert_eq!(sim.server.pages_used(), 0, "its buffers are freed, and the pool with them");
    assert_eq!(sim.host.live_objects(), 1, "only the session page is left");
    a.conn.bye(&mut sim.client(7));
    assert_eq!(sim.host.live_objects(), 0);
    assert_eq!(sim.server.session_count(), 0);
}

#[test]
fn a_resize_keeps_the_old_frame_until_the_new_one_is_committed() {
    let mut sim = Sim::floating(800, 600);
    let mut a = Painter::connect(&mut sim, 7);
    let n = a.window(&mut sim, 200, 100, RED, "w");
    sim.park_pointer();
    sim.render();
    let id = sim.server.windows()[0].0;
    // the window manager makes it fullscreen: a CONFIGURE for 800 x 600
    sim.server.with_policy(&mut sim.host, |pol, c| pol.wants(c, id, want::FULLSCREEN));
    // the client has not answered yet: the old 200 x 100 frame is drawn at the new place, the rest of the client area background
    sim.render();
    assert_eq!(sim.pixel(100, 50), RED, "the old frame, top left");
    assert_eq!(sim.pixel(400, 300), look().background, "not covered by the old frame yet");
    let seen = a.pump(&mut sim, BLUE);
    let conf = seen.iter().find(|e| e.kind == ev::CONFIGURE).expect("a configure");
    assert_eq!((conf.a, conf.b, conf.c), (800, 600, state::FULLSCREEN));
    assert_eq!(a.conn.size(n), (800, 600));
    sim.render();
    assert_eq!(sim.pixel(400, 300), BLUE);
    assert_eq!(sim.pixel(0, 0), BLUE, "fullscreen: no border");
    // one pool holds both the old and the new buffers (the old ones were freed at the commit)
    assert_eq!(sim.server.pages_used(), 1024, "one pool of the smallest size");
    assert_eq!(sim.host.slots_of(7), 2, "the client holds its session page and the one pool");
    // and back
    sim.server.with_policy(&mut sim.host, |pol, c| pol.wants(c, id, want::NOT_FULLSCREEN));
    a.pump(&mut sim, GREEN);
    sim.render();
    assert_eq!(a.conn.size(n), (200, 100));
    assert_eq!(sim.pixel(400, 300), GREEN);
    assert_eq!(sim.pixel(0, 0), look().background);
}

#[test]
fn a_program_that_dies_is_cleaned_up() {
    let mut sim = Sim::floating(800, 600);
    let mut a = Painter::connect(&mut sim, 7);
    let mut b = Painter::connect(&mut sim, 8);
    a.window(&mut sim, 200, 100, RED, "a");
    let nb = b.window(&mut sim, 200, 100, GREEN, "b");
    sim.render();
    let ob = outer(&sim, 8, nb);
    assert_eq!(sim.server.session_count(), 2);
    sim.kill(8);
    sim.tick();
    sim.render();
    assert_eq!(sim.server.session_count(), 1);
    assert_eq!(sim.server.windows().len(), 1, "{}", describe(&sim));
    assert_eq!(sim.pixel(ob.x + ob.w - 5, ob.y + ob.h - 5), look().background, "its window is gone from the screen");
    assert_eq!(sim.server.pages_used(), 1024, "its pool is freed (the other program's is left)");
    // the focus went back to the other window
    assert_eq!(sim.server.focused(), Some(sim.server.windows()[0].0));
}

#[test]
fn threads_of_a_program_share_a_session() {
    let mut sim = Sim::floating(800, 600);
    sim.set_program(21, 20);
    sim.set_program(22, 20);
    let mut a = Painter::connect(&mut sim, 21);
    let b = Connection::connect(&mut sim.client(22)).unwrap();
    assert_eq!(sim.server.session_count(), 1);
    a.window(&mut sim, 100, 100, RED, "t");
    // the program lives while one of its threads does
    sim.kill(21);
    sim.tick();
    assert_eq!(sim.server.session_count(), 1);
    sim.kill(22);
    sim.tick();
    assert_eq!(sim.server.session_count(), 0);
    let _ = b;
}

#[test]
fn keys_go_to_the_focused_window_and_clicks_move_the_focus() {
    let mut sim = Sim::floating(800, 600);
    let mut a = Painter::connect(&mut sim, 7);
    let mut b = Painter::connect(&mut sim, 8);
    let na = a.window(&mut sim, 200, 100, RED, "a");
    a.pump(&mut sim, RED);
    let nb = b.window(&mut sim, 200, 100, GREEN, "b"); // cascades 24 px down and right, and has the focus
    b.pump(&mut sim, GREEN);
    a.pump(&mut sim, RED);
    sim.input(Input::Key { code: 30, down: true, repeat: false }); // a
    sim.input(Input::Key { code: 30, down: false, repeat: false });
    let kb: Vec<_> = b.pump(&mut sim, GREEN).into_iter().filter(|e| e.kind == ev::KEY).collect();
    assert_eq!(kb.len(), 2);
    assert_eq!((kb[0].a, kb[0].b, kb[0].d), (30, 1, 'a' as u32));
    assert!(a.pump(&mut sim, RED).iter().all(|e| e.kind != ev::KEY), "the other window hears nothing");
    // shift makes capitals; ctrl makes no character
    sim.input(Input::Key { code: 42, down: true, repeat: false });
    sim.input(Input::Key { code: 30, down: true, repeat: false });
    sim.input(Input::Key { code: 42, down: false, repeat: false });
    sim.input(Input::Key { code: 29, down: true, repeat: false });
    sim.input(Input::Key { code: 46, down: true, repeat: false });
    sim.input(Input::Key { code: 29, down: false, repeat: false });
    let keys: Vec<_> = b.pump(&mut sim, GREEN).into_iter().filter(|e| e.kind == ev::KEY && e.b == 1).map(|e| (e.a, e.c & 3, e.d)).collect();
    assert!(keys.contains(&(30, mods::SHIFT, 'A' as u32)), "{keys:?}");
    assert!(keys.contains(&(46, mods::CTRL, 0)), "{keys:?}");
    // click on the part of a that b does not cover: a gets the focus, then the click (client coordinates)
    let oa = outer(&sim, 7, na);
    sim.input(Input::Motion { x: oa.x + 12, y: oa.y + 12 });
    sim.input(Input::Button { button: 1, down: true });
    sim.input(Input::Button { button: 1, down: false });
    let ea = a.pump(&mut sim, RED);
    assert!(ea.iter().any(|e| e.kind == ev::FOCUS && e.a == 1));
    assert!(ea.iter().any(|e| e.kind == ev::POINTER_ENTER));
    let click = ea.iter().find(|e| e.kind == ev::POINTER_BUTTON && e.b == 1).expect("the click");
    assert_eq!((click.c, click.d), (10, 10), "client coordinates: outer + border");
    assert!(b.pump(&mut sim, GREEN).iter().any(|e| e.kind == ev::FOCUS && e.a == 0));
    // a is on top now
    sim.render();
    let ob = outer(&sim, 8, nb);
    assert_eq!(sim.pixel(ob.x + 10, ob.y + 10), RED, "a covers b where they overlap");
    // the attention keys reach no program
    for code in [29, 56, 111] {
        sim.input(Input::Key { code, down: true, repeat: false });
    }
    assert_eq!(sim.host.attentions, 1);
    assert!(a.pump(&mut sim, RED).iter().all(|e| !(e.kind == ev::KEY && e.a == 111)));
}

#[test]
fn a_button_held_keeps_the_pointer_on_its_window() {
    let mut sim = Sim::floating(800, 600);
    let mut a = Painter::connect(&mut sim, 7);
    let na = a.window(&mut sim, 200, 100, RED, "a");
    let o = outer(&sim, 7, na);
    sim.input(Input::Motion { x: o.x + 50, y: o.y + 50 });
    sim.input(Input::Button { button: 1, down: true });
    sim.input(Input::Motion { x: 5, y: 5 }); // far outside
    let ev = a.pump(&mut sim, RED);
    let m = ev.iter().rev().find(|e| e.kind == ev::POINTER_MOTION).unwrap();
    assert_eq!((m.a as i32, m.b as i32), (5 - o.x - 2, 5 - o.y - 2), "still sent to it, outside its area");
    assert!(ev.iter().all(|e| e.kind != ev::POINTER_LEAVE), "no leave while grabbed");
    sim.input(Input::Button { button: 1, down: false });
    assert!(a.pump(&mut sim, RED).iter().any(|e| e.kind == ev::POINTER_LEAVE), "leave when the grab ends");
}

#[test]
fn frame_callbacks_pace_drawing() {
    let mut sim = Sim::floating(400, 300);
    let mut a = Painter::connect(&mut sim, 7);
    let n = a.window(&mut sim, 100, 100, RED, "a");
    {
        let t = &mut sim.client(7);
        a.conn.back_buffer(t, n).unwrap().fill(GREEN);
        a.conn.commit(t, n, None, true).unwrap();
    }
    assert!(a.pump(&mut sim, RED).iter().all(|e| e.kind != ev::FRAME_DONE), "not before it is shown");
    sim.render();
    assert!(a.pump(&mut sim, RED).iter().any(|e| e.kind == ev::FRAME_DONE));
}

#[test]
fn bad_requests_are_refused() {
    let mut sim = Sim::floating(400, 300);
    // no session yet
    assert_eq!(gliese_client::Transport::call(&mut sim.client(9), [p::w0(op::CREATE, 0, 0), p::size_word(10, 10), 0])[0] as i64, p::E_PERM);
    let mut a = Painter::connect(&mut sim, 7);
    let n = a.window(&mut sim, 100, 100, RED, "a");
    let t = &mut sim.client(7);
    let call = |t: &mut gliese_server::sim::SimClient, w: [u64; 3]| gliese_client::Transport::call(t, w)[0] as i64;
    assert_eq!(call(t, [p::w0(op::SET_TITLE, 9, 0), 0, 0]), p::E_BADWIN, "a window it does not have");
    assert_eq!(call(t, [p::w0(op::CREATE, n, 0), p::size_word(10, 10), 0]), p::E_INVAL, "a number in use");
    assert_eq!(call(t, [p::w0(op::CREATE, 5, 0), p::size_word(0, 10), 0]), p::E_INVAL, "zero size");
    assert_eq!(call(t, [p::w0(op::CREATE, 5, 0), p::size_word(10, 10), 9]), p::E_BADWIN, "a parent it does not have");
    assert_eq!(call(t, [p::w0(op::COMMIT, n, 0), 0, 77]), p::E_AGAIN, "a stale generation");
    assert_eq!(call(t, [p::w0(op::COMMIT, n, 2), 0, 1]), p::E_INVAL, "buffer 2 does not exist");
    assert_eq!(call(t, [p::w0(op::ACK_CONFIGURE, n, 12345), 0, 0]), p::E_AGAIN, "a serial never sent");
    assert_eq!(call(t, [p::w0(op::SET_LIMITS, n, 0), p::size_word(50, 50), p::size_word(40, 40)]), p::E_INVAL, "max below min");
    // the manager's request is the manager's alone
    assert_eq!(call(t, [97 | 1 << 24, 7, 0]), p::E_PERM);
}

#[test]
fn the_managers_role_word_is_accepted_from_the_manager_only() {
    let mut sim = Sim::floating(400, 300);
    sim.server.set_manager(3);
    let r = gliese_client::Transport::call(&mut sim.client(3), [97 | 1 << 24, 40, 0]);
    assert_eq!(r[0], 0, "progmgr's MGR_SET_ROLE, word for word");
    assert_eq!(sim.server.desktop_program(), Some(40));
    assert_eq!(gliese_client::Transport::call(&mut sim.client(4), [97 | 1 << 24, 41, 0])[0] as i64, p::E_PERM);
}

#[test]
fn limits_hold() {
    let mut sim = Sim::floating(400, 300);
    // room for two 100 x 100 windows' buffers, not three
    sim.server.set_limits(Limits { sessions: 2, windows_total: 3, buffer_pages: 2 * p::buffer_pages(100, 100) });
    let mut a = Painter::connect(&mut sim, 7);
    a.window(&mut sim, 100, 100, RED, "1");
    a.window(&mut sim, 100, 100, RED, "2");
    let n3 = a.window(&mut sim, 100, 100, RED, "3");
    assert_eq!(a.conn.size(n3), (0, 0), "no buffer for the third: over the budget");
    assert_eq!(gliese_client::Transport::call(&mut sim.client(7), [p::w0(op::CREATE, 9, 0), p::size_word(10, 10), 0])[0] as i64, p::E_FULL, "the window limit");
    let _b = Painter::connect(&mut sim, 8);
    assert_eq!(gliese_client::Transport::call(&mut sim.client(9), [p::w0(op::HELLO, 0, p::VERSION), 0, 0])[0] as i64, p::E_FULL, "the session limit");
}

#[test]
fn a_client_that_stops_reading_catches_up() {
    let mut sim = Sim::floating(400, 300);
    let mut a = Painter::connect(&mut sim, 7);
    let n = a.window(&mut sim, 100, 100, RED, "a");
    sim.park_pointer();
    let id = sim.server.windows()[0].0;
    // far more events than the ring holds, then a configure
    for _ in 0..200 {
        sim.input(Input::Key { code: 30, down: true, repeat: true });
        sim.input(Input::Key { code: 30, down: false, repeat: false });
    }
    sim.server.with_policy(&mut sim.host, |pol, c| pol.wants(c, id, want::FULLSCREEN));
    let seen = a.pump(&mut sim, BLUE);
    assert!(seen.iter().any(|e| e.kind == ev::CONFIGURE), "the configure is found in the window table after the overflow");
    assert_eq!(a.conn.size(n), (400, 300));
    sim.render();
    assert_eq!(sim.pixel(200, 150), BLUE);
}

#[test]
fn dialogs_open_over_their_owner() {
    let mut sim = Sim::floating(800, 600);
    let mut a = Painter::connect(&mut sim, 7);
    let n = a.window(&mut sim, 400, 300, RED, "main");
    let d = {
        let t = &mut sim.client(7);
        a.conn.create(t, 100, 50, Some(n)).unwrap()
    };
    a.pump(&mut sim, GREEN);
    let (o, od) = (outer(&sim, 7, n), outer(&sim, 7, d));
    assert_eq!(o.center(), od.center());
    // raising the owner keeps the dialog above it
    let id = sim.server.windows().iter().find(|w| w.2 == n).unwrap().0;
    sim.server.with_policy(&mut sim.host, |_, c| c.raise(id));
    assert_eq!(sim.server.windows().last().unwrap().2, d);
}

#[test]
fn screenshot() {
    let mut sim = Sim::floating(640, 400);
    let mut a = Painter::connect(&mut sim, 7);
    let mut b = Painter::connect(&mut sim, 8);
    a.window(&mut sim, 300, 200, RED, "a");
    b.window(&mut sim, 300, 200, BLUE, "b");
    sim.input(Input::Motion { x: 320, y: 200 });
    sim.render();
    let path = concat!(env!("CARGO_TARGET_TMPDIR"), "/p3-floating.png");
    sim.save_png(path).unwrap();
    let png = std::fs::read(path).unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn many_windows_share_a_few_slots() {
    // the reason for pools: a program with 40 windows holds its session page and a handful of pool slots, not 40
    let mut sim = Sim::floating(1024, 768);
    let mut a = Painter::connect(&mut sim, 7);
    for i in 0..40 {
        a.window(&mut sim, 160, 120, RED, &format!("w{i}"));
    }
    sim.render();
    assert_eq!(sim.server.windows().len(), 40);
    let slots = sim.host.slots_of(7);
    assert!(slots <= 1 + p::MAX_POOLS, "{slots} slots");
    assert!(sim.host.live_objects() <= 1 + p::MAX_POOLS);
    // closing them all gives every pool back
    for n in 0..40u8 {
        a.conn.destroy(&mut sim.client(7), n);
    }
    assert_eq!(sim.server.pages_used(), 0);
    assert_eq!(sim.host.live_objects(), 1);
}
