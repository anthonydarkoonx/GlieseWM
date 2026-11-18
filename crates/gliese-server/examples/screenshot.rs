//! Pictures of the compositor in the simulator: `cargo run -p gliese-server --features sim --example screenshot` writes
//! `crates/gliese-server/out/*.png`.
use gliese_proto::want;
use gliese_server::sim::{Painter, Sim};
use gliese_server::Input;

fn main() {
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/out");
    let mut sim = Sim::floating(1024, 768);
    let mut a = Painter::connect(&mut sim, 7);
    let mut b = Painter::connect(&mut sim, 8);
    let mut c = Painter::connect(&mut sim, 9);
    a.window(&mut sim, 480, 320, 0xBF616A, "a");
    b.window(&mut sim, 480, 320, 0xA3BE8C, "b");
    c.window(&mut sim, 480, 320, 0x88C0D0, "c");
    sim.input(Input::Motion { x: 300, y: 260 });
    sim.render();
    sim.save_png(&format!("{out}/p3-three-floating.png")).unwrap();
    // click the bottom window: it comes to the top and takes the focus
    sim.input(Input::Button { button: 1, down: true });
    sim.input(Input::Button { button: 1, down: false });
    sim.render();
    sim.save_png(&format!("{out}/p3-clicked.png")).unwrap();
    // the top one goes fullscreen
    let top = sim.server.focused().unwrap();
    sim.server.with_policy(&mut sim.host, |p, ctl| p.wants(ctl, top, want::FULLSCREEN));
    for p in [&mut a, &mut b, &mut c] {
        p.pump(&mut sim, 0xEBCB8B);
    }
    sim.render();
    sim.save_png(&format!("{out}/p3-fullscreen.png")).unwrap();
    println!("wrote {out}/p3-*.png");
}
