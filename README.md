# GlieseWM

A modular tiling window manager and compositor for [Metal OS](https://github.com/anthonydarkoonx/metal-os): dwindle and master
layouts, exact gaps, workspaces, and keybindings in `gliese.conf`. Ideas from Hyprland; no code from it. Everything is `no_std` +
`alloc` and builds for `aarch64-unknown-none-softfloat`.

The design and the phases are in [`PLAN.md`](PLAN.md).

| Crate | What it is | Status |
|---|---|---|
| `crates/gliese-layout` | `Layout` trait, the gap arithmetic (`split`, `stack`, outer and smart gaps), dwindle and master | done, property-tested |
| `crates/gliese-config` | `gliese.conf` parser: sections, variables, binds (`bind`/`binde`/`bindr`/`bindm`), submaps, window rules, workspace overrides, `source`; errors with line and column. The built-in default is [`crates/gliese-config/gliese.conf`](crates/gliese-config/gliese.conf) | done, tested |
| `crates/gliese-proto` | The window protocol: ops, events, word packing, the session page and its event ring, buffer pools | done |
| `crates/gliese-client` | The program side: connect, make windows, double-buffered drawing, events (configures acknowledged for you) | done |
| `crates/gliese-server` | The compositor: sessions, windows, double buffers, the configure handshake, input routing, damage-tracked rendering, a software pointer, the error bar; the `Policy` seam the WM plugs into; feature `sim` = an in-memory host with real clients and PNG screenshots | done, 13 end-to-end tests |
| `crates/gliese-core` | The window manager (a `Policy`): 10 workspaces, tiling with gaps, floating, fullscreen, pinning, binds and submaps from `gliese.conf`, every dispatcher, window rules, mouse drags (move, resize, drop a tile on another to swap), the error bar for a bad config | done, 13 end-to-end tests |
| `crates/gliese-os` | GlieseWM on Metal OS system calls: the compositor's `Host`, the screen task's loop (display requests, `INPUT_POLL`, `GPU_FLUSH2`), and `OsTransport`, the client side programs use. The only crate with `unsafe` | done; links into the OS image |
| `crates/metal-desktop-boot` | The desktop contract's entry point, `metal_desktop_run` | done |
| `crates/gliese-agent` | What `gliesed` does: upload `gliese.conf` (or the built-in default), reload it when it changes or on `reload`, start programs for `exec` (or ask a running one for a new window), `exec-once` | done, 9 end-to-end tests |
| `crates/gliese-term` | The terminal's screen: lines with scrollback, a small ANSI subset (colours, clear), drawn with metal-gfx's font | done, tested |
| `crates/gliese-shell` | The shell: line editing, history, 24 built-ins, pipes and `>`/`>>` between them, `nano` (metal-edit), running programs | done, tested |
| `os/` | The Metal OS programs: `gliesed` (the desktop role, no window; started at boot) and `terminal` (Super+Enter; one program, a window per Super+Enter, runs programs through `permission = spawn`) | done; both run on QEMU |

## Building and testing

```sh
cargo test --workspace --all-features                                   # host tests (the compositor's need its `sim` feature)
cargo clippy --workspace --all-targets --all-features                   # lints (metal-os's deny-level set)
cargo run -p gliese-server --features sim --example screenshot          # pictures: crates/gliese-server/out/*.png
cargo build --release --target aarch64-unknown-none-softfloat --workspace
```

## Building it into Metal OS

The metal-os Makefile builds `metal-desktop-boot` and links it into the screen task:

```sh
cd ../metal-os && make run-window DESKTOP=../GlieseWM
```

That builds into `target-metal-os/` and `target-metal-os-programs/` here, signs `gliesed` and `terminal` into the system image and
installs them by default (`gliesed` is started at boot). Super+Enter opens a terminal. Running programs from it needs metal-os's
`permission = spawn` (its `spawn-permission` branch until that is merged).

## Your config

On the machine your file is `/data/programdata/gliesed/gliese.conf`. Open a terminal (Super+Enter) and run `nano $GLIESE_CONF`;
saving applies it. A mistake shows in a bar at the top of the screen with its line and column, and the previous config stays in force.

License: Apache-2.0 OR MIT, like Metal OS.
