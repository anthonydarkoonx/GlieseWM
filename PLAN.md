# GlieseWM: plan

A modular tiling window manager and compositor for Metallium-OS. It has gaps, and you set its keybindings in `gliese.conf`. The
ideas come from Hyprland: dwindle and master layouts, `bind = MOD, KEY, dispatcher, arg`, workspaces, submaps,
`gaps_in`/`gaps_out`, no title bars, and an error bar for a broken config. No Hyprland code is used. Hyprland is
BSD-3-Clause; only its concepts and config shape are taken, and `PROVENANCE.md` will say so.

Revised 2026-11-18. **Status:** P0 (scaffold), P1 (layouts and gaps), P2 (config), P3 (protocol, client, compositor with the simulator), P4 (the window manager) and P5 (the OS adapter and `metallium-desktop-boot`; the metallium-os user image links with GlieseWM in it) P6 (`gliesed`; `make disk DESKTOP=…` builds, signs and installs it) and P8 (first boot in QEMU, 2026-11-18: GlieseWM takes the screen at 1024 x 768, the program manager installs and starts `gliesed` and gives it the desktop role, the config loads, Super+Enter reaches the window manager and `gliesed`) are done; see `README.md`. P7 (the terminal) is done too: on QEMU Super+Enter opens tiled terminal windows; built-ins, `fetch` and `argdump` from `/system/bin` run in them through `permission = spawn`. Each running program has its own console endpoint and a thread serving it: a task owes at most one reply at a time (`kernel/src/ipc.rs`), so one loop could not keep a program waiting for input without stalling the others. Config change detection compares the file's contents once a second (the VFS `stat` gives size and kind, no modification time). Not yet done from P4's sections: `drop_hidden_buffers` (§3.2: hidden windows still keep their buffers) and `focusurgent` (nothing marks a window urgent yet).

---

## 0. Ground rules

- **Nothing is written in `~/Desktop/metallium-os`.** GlieseWM plugs in as a desktop repository through the OS's own seam,
  `make run-window DESKTOP=../GlieseWM` (metallium-os `docs/DESKTOP-INTERFACE.md`).
  - Running that target puts build output (`build/`, `disk.img`) in the metallium-os folder, so every QEMU run waits for
    explicit permission (P8).
- **Nothing is taken from metallium-os-desktop.** No crates, no code, no protocol, no taskbar. It was read only to learn how a
  desktop plugs into the OS. Every interface GlieseWM relies on is defined in metallium-os itself (§1), and is cited from
  there.
- **Dependencies:** only metallium-os's own libraries, as git dependencies on `github.com/anthonydarkoonx/metallium-os` at a pinned
  revision. These are `metallium-gfx` (canvas, rects, regions, fonts), `metallium-driver-rt` (system calls, the GPU and input
  driver protocols), `metallium-input` (input event translation) and `metallium-programs` (the program runtime, for `os/`). The
  OS Makefile already repoints those at its local copies when it builds a desktop.
- The metallium-os rules carry over:
  - `no_std` + `alloc`;
  - `#![forbid(unsafe_code)]` in every crate except the single OS adapter;
  - deny `unsafe_op_in_unsafe_fn` and `clippy::undocumented_unsafe_blocks`;
  - no GPL-derived code;
  - say "program", not "app";
  - no programs beyond what this plan lists (§10 asks about the rest);
  - fast host tests only, unless asked.

## 1. How a desktop plugs into Metallium-OS (the OS-side contract)

| Contract | Source in metallium-os |
|---|---|
| A crate named `metallium-desktop-boot` exporting `pub fn metallium_desktop_run(display: u64, gpu: u64, input: u64, framebuffer: u64, fb_handle: usize, manager: u32, power: u64) -> !`. The screen task (role 13) calls it and never gets control back | `docs/DESKTOP-INTERFACE.md`; `Makefile` lines 57-90 |
| It must call `metallium_driver_rt::auto_maps()` first: the screen task is a boot task, and the kernel refuses fixed mapping addresses | same |
| Built with `-C relocation-model=pic -Zembed-metadata=yes`, fat LTO | `Makefile` |
| An `os/` Cargo workspace of `no_std` programs (`os/src/bin/*.rs`, using `metallium-programs`), `os/packages/<id>.manifest`, and `os/defaults` (`id autostart`). These are signed with the dev key and installed by default | `DESKTOP-INTERFACE.md` |
| `role = desktop` in a manifest implies `permission = shell`. Once that program starts, the program manager calls the display endpoint with `w0 = 97 \| 1 << 24` (MGR_SET_ROLE, role 1) and `w1 = task id`, and expects `w0 = 0` back. It is the only display call the OS makes | `programs/src/bin/progmgr.rs:351` |
| Programs with `permission = display` get a send-only copy of the display endpoint in **slot 2** | `docs/PROGRAMS.md` |
| A program with `permission = files` sees its own `/data/programdata/<id>` as `/data` | `docs/PROGRAMS.md` |
| GPU: `GPU_INFO` (32) returns `w \| h << 32`; `GPU_FLUSH2` (34) presents two rectangles, each `x \| y<<16 \| w<<32 \| h<<48` | `libs/metallium-driver-rt/src/proto.rs` |
| Input: `INPUT_POLL` (71) returns up to two event words: `kind` (KEY=1, MOVE=2 absolute 0..32767, BUTTON=3 with 1 left / 2 right / 3 middle, WHEEL), a 16-bit code, and a 32-bit value. Key numbers are PC/AT set 1 (A=30, Super=125/126) | `proto.rs` |
| Shared memory and wake-ups: `SHM_CREATE` 37, `SHM_INFO` 38, `MEM_MAP` 4, `CAP_GRANT` 5, `CAP_REVOKE` 6, `WAIT_WORD` 32, `WAKE_WORD` 33. Since metallium-os `335ff8f` a *program* may `CAP_GRANT` only into its own program, programs it started and its parent (anything else: `-1`); boot-time tasks, which includes the compositor in the screen task, are not restricted, so the compositor still grants session pages and buffers to any client. Client programs must not grant to each other or to the compositor | `docs/ABI.md` |
| Time, tasks and power: `CLOCK` 34, `SLEEP_US` 44, `SYSINFO` pages 3 (task alive) and 5 (a task's program), `POWER` 42. Since metallium-os `1aa9b5d` `SYSINFO` pages 10, 11, 13 and 14 need an `Admin` capability; 3 and 5 stay open | `docs/ABI.md` |

So GlieseWM's display protocol is its own. It must only answer op byte 97 the way the manager expects, and it reserves
op bytes 96..=99 for the manager.

## 2. Architecture

```
  gliese.conf ─ read by `gliesed` (role = desktop) ─ uploaded over the display endpoint
                                                       │
┌──────────────── screen task (role 13) ───────────────▼──────────────────┐
│ metallium-desktop-boot ─► gliese-os::run   (OsHost: the only `unsafe`)       │
│                          │ requests · input words · timers               │
│   gliese-server: sessions, windows, buffers, configure/commit,           │
│                  input routing, damage, borders, cursor, error bar       │
│          │ events (mapped, key, button, …)       ▲ Ctl (place, hide, …)  │
│          ▼                                       │                       │
│   gliese-core: workspaces, focus, binds/submaps, dispatchers, rules ─────┘│
│          └─ gliese-layout: dwindle | master | (monocle, columns)         │
└──────────────────────────────────────────────────────────────────────────┘
        ▲ display endpoint (gliese-proto)
  programs with `permission = display` (via gliese-client)      terminal, gliesed
```

### 2.1 Crates (Cargo workspace in `~/Desktop/GlieseWM`)

| Crate | Purpose | `unsafe` |
|---|---|---|
| `crates/gliese-proto` | The window protocol: op and event numbers, word packing, the session page layout, the event ring | forbidden |
| `crates/gliese-config` | `gliese.conf` parser: variables, sections, binds, rules, the key-name table, errors with line:col | forbidden |
| `crates/gliese-layout` | The `Layout` trait, gap and split arithmetic, one module per layout behind a Cargo feature | forbidden |
| `crates/gliese-core` | The WM's brain: workspaces, focus history, binds, submaps, dispatchers, window rules. Knows only ids and rectangles | forbidden |
| `crates/gliese-server` | The compositor: sessions, windows, buffers, the configure handshake, input routing, rendering. Talks to the system only through a `Host` trait; feature `sim` gives an in-memory host for tests and screenshots | forbidden |
| `crates/gliese-os` | `OsHost` (the `Host` trait on system calls) and the `run` loop | **the only one** |
| `crates/gliese-client` | Client library for programs: connect, create a window, draw into the buffer, handle events | forbidden |
| `metallium-desktop-boot` | The contract's entry point: `auto_maps()`, built-in config, `gliese_os::run` | forbidden |
| `os/` | Programs: `gliesed` (no window, `role = desktop`) and `terminal` (§8.1) | forbidden |
| `crates/gliese-config/gliese.conf` | The default config, compiled in with `include_str!` | — |

**Modular** means:
1. **Layouts:** each is one file implementing `Layout`, behind a feature, and registered by name, so `layout = master`
   picks it at run time.
2. **Dispatchers:** a static table of `(name, arg parser, fn)`. Adding one means adding a single table entry.
3. **Config sections:** each module declares its keys and types (`ConfigSchema`), and the parser checks them. A module
   brings its own settings without touching the parser.
4. **Core and server are separate:** `gliese-core` drives `gliese-server` only through the narrow `Ctl` interface, so the
   tiling logic can be tested with no compositor at all.

There is no run-time plugin loading: Metallium-OS has no `.so` mechanism. Modules are chosen with Cargo features.

## 3. The compositor (`gliese-server`)

**Model:**
- One **session** per program (threads share it).
- A session has a 4-page shared **session page**, created by the compositor and granted map+write:
  - a header (magic, version, screen size);
  - a window table (buffer slot, size, generation, configure serial for each window);
  - an event ring.
- Events go into the ring, then `WAKE_WORD`. There is never a per-client endpoint, so a stuck client cannot block the
  compositor.

**Window kinds** (only two in v1):
- **toplevel:** tiled or floating;
- **child:** a dialog with a parent, which floats centred on the parent.

Menus and popups come later.

**Buffers:**
- The compositor creates them (`SHM_CREATE`) and pays for them.
- **Pools (changed in P5):** buffers are cut out of a few large shared objects per program (at most 6, up to 4096 pages each),
  not one object per window. On Metallium-OS a task has only 32 capability slots (`kernel/src/cap.rs`, about half of the screen
  task's are free) and 64 mappings, and every object costs one of each in the compositor *and* in the program. A program
  holds its session page plus one slot per pool, so 40 windows cost about 3 slots instead of 40. Up to 64 windows per program.
- They are **double-buffered from the start**: tiling resizes windows often, and a single buffer would show half-drawn
  frames.
- Each resize goes through a **configure handshake**:
  1. CONFIGURE (serial, w, h, state);
  2. the client sends ACK_CONFIGURE and gets the new buffer pair;
  3. the client draws, then COMMIT (generation, damage);
  4. the compositor swaps the buffers and redraws the damage.

  Until the commit, the old buffer stays on screen at its old size, drawn inside the new tile.

**Rendering:**
- Software compositing on the framebuffer: opaque, damage-tracked, with `GPU_FLUSH2` for each pair of changed rectangles.
- No title bars. Every window gets a border in `col.active_border` or `col.inactive_border`.
- The cursor is drawn in software, and the background is a solid colour.

**Input routing:** keys go to the core's bind table first. A key with no bind goes to the focused window.
- Focus follows clicks, plus `follow_mouse` as an option.
- The pointer goes to the window under it, with an implicit grab while a button is held.
- The secure attention keys (Ctrl+Alt+Del, Ctrl+Shift+Esc) are handled *before* the bind table, so no config can capture
  them.

**Error bar:** when a config upload fails, the compositor draws a strip at the top of the screen with the first error
(`gliese.conf:12:7: unknown key 'gap_in'`) until the next good config. There are no notifications to show it otherwise.

**The manager:** op 97 (MGR_SET_ROLE) is honoured only from the `manager` task id passed to `metallium_desktop_run`. It marks
that task's program as the desktop role, the only session allowed the privileged ops in §7.

**Dead programs:** each tick checks whether every session's tasks are still alive (`SYSINFO` page 3). A dead program's
windows and buffers go, and its tiles close up.

### 3.1 `Ctl`: what the core may do to the server

| Method | Notes |
|---|---|
| `windows()`, `win(id)`, `screen()`, `usable_area()`, `focused()` | read-only views |
| `place(id, outer: Rect)` | the border is inside `outer`; the client gets `outer - 2·border` through a configure |
| `set_hidden(id, bool)` | used for workspaces |
| `focus(id)`, `raise(id)`, `close(id)` | `close` sends CLOSE_REQUEST |
| `begin_grab(kind)`, `end_grab()` | mouse binds |
| `to_agent(event)` | an event to `gliesed`: exec, reload |
| `set_error(Option<String>)` | the error bar |
| `set_border(colours, size)`, `damage(rect)` | |

The server calls the core on: `mapped(&WinInfo) -> Placement { Tile, Float(Rect) }`, `unmapped`, `key`, `button`,
`motion` (during a grab), `client_wants(fullscreen | float)`, `title_changed`, `area_changed`, `tick`.

### 3.2 How many windows

There is no small fixed cap. The real limit is memory: `make run-window` boots with 128 MiB, and one 1024×768 screen of
pixels is 3 MiB.

- **Visible workspace:** its tiles together cover about one screen, so they cost about 2 screens of buffers with double
  buffering (≈ 6 MiB). This holds however many windows are open.
- **Hidden workspaces:** with `misc { drop_hidden_buffers = true }` (the default), a window on a hidden workspace gives
  its buffers back. When its workspace is shown again it gets a configure and redraws; until its first commit, its tile
  shows the background colour (one frame). A terminal redraws from its character grid, so this costs it nothing. Memory
  therefore follows what is on screen, not how many windows exist.
- **Bookkeeping caps:**
  - `misc { max_windows = 256 }` in total, and 64 per program, so one runaway program cannot flood the table.
  - `misc { buffer_budget_mib = 32 }` is the ceiling for all buffers. A request beyond it is refused, and the client keeps
    its old buffer.
- **Tiles too small to use:** dwindle keeps halving, so a workspace with 30 windows has tiny tiles. Below
  `misc { min_tile = 64 }` pixels, a new window opens on that workspace in the master/stack fallback, or you move it to
  another workspace. Nothing is refused.

## 4. Layouts and gaps (`gliese-layout`)

### 4.1 The trait

```rust
pub trait Layout {
    fn name(&self) -> &'static str;
    /// Tile rectangles for the workspace's tiled windows, inside `area` (already shrunk by gaps_out).
    fn arrange(&self, area: Rect, g: &Gaps, out: &mut Vec<(WinId, Rect)>);
    fn inserted(&mut self, id: WinId, focused: Option<WinId>, area: Rect);
    fn removed(&mut self, id: WinId);
    fn message(&mut self, msg: &LayoutMsg) -> bool;   // togglesplit, swapwithmaster, mfact +0.05, orientation …
    fn resize(&mut self, id: WinId, dx: i32, dy: i32, area: Rect);
}
```

Each workspace owns its layout's state.

- **dwindle:** a binary tree. A new window splits the focused leaf along its longer side, or the side named by
  `force_split`. `preserve_split` keeps the direction; `togglesplit` flips it. Each node has its own ratio.
- **master:** settings `mfact`, `nmaster`, `orientation = left|right|top|bottom|center`, `new_on_top`. Messages
  `swapwithmaster`, `addmaster`, `removemaster`.
- Later: `monocle` and `columns` (scrolling).

### 4.2 Gap arithmetic (exact, never off by a pixel)

Settings: `gaps_out` (1 or 4 values: top right bottom left), `gaps_in` (the full gap between two tiles), `border_size`,
`smart_gaps`, and `reserved` (edges kept free, for a bar someone may add later).

1. `area = screen - reserved - gaps_out`.
2. Every split goes through one function:
   ```
   split(r, ratio, gap, horizontal) -> (a, b):
       usable = r.len - gap
       a.len  = round(usable * ratio)  clamped to 1..usable-1
       b.pos  = a.pos + a.len + gap
       b.len  = r.len - a.len - gap
   ```
   The space between any two adjacent tiles is exactly `gaps_in`. Stacks of n windows share `len - (n-1)·gaps_in`, and
   the remainder pixels go to the first tiles.
3. A tile is the window's outer rectangle. The border is drawn inside it, and the client gets `tile - 2·border_size`.
4. `smart_gaps = true` means one tiled window gets no outer gap and no border. `fullscreen` ignores gaps, borders and
   `reserved`.
5. **Minimum sizes:** when a client's minimum is larger than its tile, the client keeps its size and is drawn clipped to
   the tile. A rule can float it instead.
6. Gaps can change at run time (`setgaps`, or a reload). Each workspace can override them
   (`workspace = 3, gaps_in:0, gaps_out:0`).

Property tests cover each layout, n = 1..16 windows, random ratios, gaps 0..40, and odd sizes. They check that:
- no two tiles overlap;
- every tile lies inside the area;
- adjacent tiles are exactly `gaps_in` apart;
- the outer edges sit exactly `gaps_out` from the edge;
- tiles plus gaps cover the area;
- the result is deterministic.

There are also golden rectangles for 1, 2, 3 and 5 windows at 1024×768.

## 5. `gliese.conf`

Hyprland-shaped and line-oriented. The built-in default is `crates/gliese-config/gliese.conf`. Grammar:

```
line      := comment | assign | section-open | '}' | keyword
comment   := '#' …
assign    := name '=' value                 # inside a section: general { gaps_in = 6 }
variable  := '$' name '=' value             # $mod = SUPER
keyword   := bind* | windowrule | workspace | submap | exec-once | source
bind      := ('bind'|'binde'|'bindr'|'bindm') '=' MODS ',' KEY ',' dispatcher [',' arg]
```

- **Binds:**
  - Flags: `bind` (press), `binde` (repeat while held), `bindr` (release), `bindm` (mouse drag: `movewindow`,
    `resizewindow`).
  - Modifiers: `SUPER SHIFT CTRL ALT`. Caps Lock is ignored.
  - Keys: `A`–`Z`, `0`–`9`, `Return`, `Space`, `Tab`, `Escape`, `F1`–`F12`, arrows, punctuation names,
    `mouse:left|right|middle`, or `code:N` for any raw key number. The name table is written from scratch from the input
    protocol's PC/AT set-1 numbering.
- **Submaps:** `submap = resize` … `submap = reset`.
- **Rules:** `windowrule = float|tile|workspace N|size W H|center, title:<glob>|program:<id>`. Matching uses globs; there
  is no regex engine.
- **Reserved:** Ctrl+Alt+Del and Ctrl+Shift+Esc cannot be bound; the parser says so.
- **Errors:** every error has line:col. A config with errors is **not applied**: the previous one stays and the error bar
  shows the first error. Unknown keys are errors.
- **Where the file lives (agreed):**
  - The built-in default applies at start.
  - Then `gliesed` (with `permission = files`) reads `/data/gliese.conf` in its own view, which is
    `/data/programdata/gliesed/gliese.conf` on disk, and uploads it.
  - No file means the default stays.
  - `reload` re-reads it.

### 5.1 Dispatchers (v1)

| Group | Dispatchers |
|---|---|
| Windows | `killactive`, `togglefloating`, `fullscreen`, `pin`, `centerwindow` |
| Focus and move | `movefocus l\|r\|u\|d`, `movewindow l\|r\|u\|d` (swap), `cyclenext [prev]`, `focusurgent` |
| Resize | `resizeactive dx dy`, `splitratio ±f` |
| Workspaces | `workspace N\|+1\|-1\|previous\|empty`, `movetoworkspace N`, `movetoworkspacesilent N` |
| Layout | `layoutmsg <msg>`, `togglesplit`, `layout <name>`, `setgaps in out` |
| Programs | `exec <program-id>`: see §5.2 |
| Meta | `submap <name>`, `reload`, `exit` (power off through `POWER`, only with the power right) |

`movefocus` works on geometry, not on the layout: the nearest visible tile in that direction, by edge overlap and then by
distance. Every layout gets it for free.

With no taskbar, binds are the only way to start programs, as in Hyprland. The default config has
`bind = $mod, Return, exec, terminal`: **Super+Enter opens a shell window** (§8.1).

### 5.2 How `exec` works

The program manager runs **one instance per program**: `START` on a running program returns status 3, "wrong state"
(`progmgr.rs:239`). Super+Enter must still open a new terminal every time, so `exec` goes like this:

1. The compositor sends `EXEC(id)` to `gliesed`.
2. `gliesed` calls `START` on the manager's control endpoint.
   - If it works, the program comes up and opens its first window.
   - If the status is 3 (already running), `gliesed` asks the compositor to send that program a `NEW_WINDOW` event, and
     the program opens another window. The terminal handles it; a program that ignores it just gets its newest window
     focused.

Arguments are not passed: the OS cannot start a program with one yet.

### 5.3 Reloading

- `misc { auto_reload = true }` (the default) makes `gliesed` check the file's size and modification time once a second.
  It re-uploads the file when either changes, so saving in `nano` applies the config at once, as in Hyprland.
- The `reload` dispatcher (Super+Shift+C) and the terminal's `reload` builtin do the same on demand.

## 6. Workspaces

- Ten numbered workspaces on the one screen.
- Inactive workspaces' windows are hidden.
- Floating windows belong to a workspace. `pin` shows a window on all of them.
- Rules can send a program to a workspace.
- Later: a scratchpad (Hyprland's "special workspace").

## 7. Protocol (`gliese-proto`)

Requests are `IPC_CALL` on the display endpoint with 3 words. `w0 = op | window << 8 | arg << 24`, the same packing the
manager's op 97 uses, so one decoder serves both.

| Range | Ops |
|---|---|
| 1..=31, client | `HELLO` (returns the session page slot), `BYE`, `CREATE` (toplevel or child, size), `DESTROY`, `SET_TITLE` (through the session page), `SET_LIMITS`, `ACK_CONFIGURE`, `COMMIT` (generation, damage rect), `SET_CURSOR`, `WANT` (fullscreen / float hint) |
| 32..=39, any program | `GL_QUERY` (workspaces, windows, active window, layout, version: for a future `workspaces` built-in), `GL_RELOAD` (asks `gliesed` to re-read the file; harmless) |
| 48..=63, desktop role only | `GL_CONFIG_BEGIN/CHUNK/END` (config text in 4088-byte chunks through the session page; the reply is ok or the first error), `GL_AGENT_DONE` |
| 96..=99 | reserved for the program manager; 97 = MGR_SET_ROLE (§1) |

Events in the ring: `CONFIGURE`, `CLOSE_REQUEST`, `FOCUS_IN/OUT`, `KEY` (code, state, modifiers, character from the US
layout), `POINTER_ENTER/LEAVE/MOTION/BUTTON/WHEEL`, `FRAME_DONE`, `NEW_WINDOW` (§5.2). Agent events: `EXEC(program id)`,
`RELOAD`. The desktop-role op `GL_NEW_WINDOW(program)` asks the compositor to send that program `NEW_WINDOW`.

Dispatching from a program is **left out**: the server cannot tell which programs hold the shell permission, so it would let any
program move or close other programs' windows.

## 8. Programs (`os/`)

| Program | Manifest | What it does |
|---|---|---|
| `gliesed` | `role = desktop`, `permission = files`, `autostart` in `os/defaults`, no window | Reads `/data/gliese.conf` (its own view) and uploads it at start; watches it (§5.3). On RELOAD it re-reads and uploads; on EXEC it starts the program, or asks for a new window (§5.2). It holds the control endpoint because the role implies `shell` |
| `terminal` | `permission = display, allfiles, shell, network, spawn`, `spawn_pages = 8192`, `class = interactive`, installed by default (not autostarted) | The shell window. One program, any number of windows (§8.1) |

`glctl` is dropped: the terminal's built-ins cover querying and reloading.

### 8.1 The terminal

Super+Enter opens a window with a prompt, where you can run commands and edit `gliese.conf` with `nano`.

**Running programs: through a new OS permission, `spawn` (decided 2026-11-18).**
- Today a packaged program has no spawn capability. The only thing that can start programs is the screen task, and it
  does so by accident of the console's layout (`kernel/src/task.rs:378-398`).
- The proper fix is a manifest permission. The program manager gives a program that declares `permission = spawn` an
  endpoint factory in slot 5 and a spawn capability in slot 6. That spawn capability can never give a child more than
  the program has: file access only with `allfiles`, network only with `network`, and no grant right.
- **Applied in metallium-os on 2026-11-18** (uncommitted). The design record is `docs/OS-SPAWN-PERMISSION.md`; the
  reference is metallium-os `docs/PROGRAMS.md`, *Starting programs*. GlieseWM's own code still never writes in metallium-os.

**What the terminal does with it:**
- It starts command-line programs with `metallium-programs`' own `Command`, which already looks in slots 5 and 6, finds the
  image on `PATH=/system/bin:/data/bin`, and passes the environment.
- Each child gets a console endpoint the terminal created. The terminal serves the console protocol itself
  (`docs/ABI.md`, "Console"): output words arrive with `IPC_RECV`, and reads are answered from what you type.
- Ctrl-C is `KILL` and the exit code is `WAIT`. Both work because the terminal is the child's parent, and children end
  when their terminal ends.
- There is **no run broker, no `GL_RUN`/`GL_KILL`/`GL_TRUST`**, and GlieseWM does not touch the screen task's
  console-only slots. The compositor stays a compositor.

**Your CLI programs:**
- Put them in `/system/bin` or `/data/bin`, type the name, and they run in that terminal window, with their output there.
- They need no permission of their own. They get file access because the terminal has `allfiles`, and network because it
  has `network`.

**`nano`:** it is **not** a separate program, in the OS console or here. It is metallium-os's `metallium-edit` library (its
`term` module lays the editor out as a character grid; ctrl-O to write, ctrl-X to exit), linked into the terminal, so
`nano FILE` takes over that terminal window until ctrl-X.

**Built-ins:** some commands stay built into the shell, because they change the shell itself (`cd`, `exit`) or because
they must work even without the OS change (`ls`, `cat`, `nano`, `prog`, `reload`, …).

**Before the OS change lands:** the terminal checks slot 6 with `CAP_INFO` at start. If there is no spawn capability,
typing a program name says "running programs needs `permission = spawn` (an OS update)". The built-ins work either way.

**Crates:**

| Crate | Purpose |
|---|---|
| `crates/gliese-term` | The character grid: cells with colour and reverse video, cursor, scrollback (1000 lines), a small subset of ANSI (SGR colours, clear, cursor moves), and selection later. It draws into the window buffer with `metallium-gfx`'s bitmap font and reflows on resize. Pure and host-tested |
| `crates/gliese-shell` | The shell as pure state behind three traits: `Fs` (the VFS), `Programs` (the control endpoint) and `Runner` (start through `metallium-programs` `Command`, serve the child's console, kill, wait). A command that is not a built-in runs through `Runner`. Ctrl-C kills the child. It has the line editor (left/right, home/end, backspace, Ctrl-A/E/U/K/W/L/C), history (↑/↓, 100 lines), `~`, `$GLIESE_CONF`, and quoting with `" '`. Host-tested against a fake file system |
| `os/src/bin/terminal.rs` | Glue: `gliese-client` windows, one `gliese-term` and one `gliese-shell` per window. Opens a window at start and on each `NEW_WINDOW`; exits when its last window closes |

**Built-ins (v1):** `help`, `clear`, `echo`, `pwd`, `cd`, `ls`, `cat`, `head`, `tail`, `wc`, `grep`, `sort`, `stat`,
`mkdir`, `rm`, `cp`, `mv`, `uptime`, `free`, `nano`/`edit`, `prog list|start|stop|state` (the control endpoint, through
`shell`), `reload`, `exit`. Pipes (`|`) and `>`/`>>` between built-ins work as text passed along inside the shell.

**Editing the config:** `nano $GLIESE_CONF`. The variable is `/data/programdata/gliesed/gliese.conf`; the terminal has
`allfiles`, so it sees the whole disk. Saving with ctrl-O triggers auto-reload. A mistake shows in the error bar, and the
previous config stays in force. The file does not exist until it is first saved, so `nano` opens the built-in default as
its starting text (`gliese-config` exposes it).

**Not in v1:** `fetch`, `download` and `play` as built-ins (they can come back as CLI programs), tabs, mouse selection
and copy/paste, and full-screen CLI programs other than `nano`. Those would need a cursor-addressing protocol on the
console, which the OS console does not have either.

## 9. Testing and phases

**Tests:**
- **gliese-config:** every construct, every error message with line:col, the default file round-trips, and a random-input
  fuzz loop (reduced under Miri).
- **gliese-layout:** the property and golden tests in §4.2.
- **gliese-core:** scenarios against a fake `Ctl`.
- **gliese-server:** the `sim` host with real `gliese-client` clients, plus PNG screenshots (`examples/screenshot`
  → `out/*.png`).
- **Lints:** clippy with the metallium-os deny-level lints.

| Phase | Work | Done when |
|---|---|---|
| **P0 Scaffold** | Workspace, `README.md`, `PROVENANCE.md` (Hyprland credited for ideas; metallium-os-desktop named as a read-only reference, nothing taken), pinned metallium-os git deps, lint config | `cargo build` on host and for `aarch64-unknown-none-softfloat` |
| **P1 Layout + gaps** | `gliese-layout`: split function, gaps, dwindle, master | property and golden tests pass |
| **P2 Config** | `gliese-config`, key names, `crates/gliese-config/gliese.conf` | every rule tested; the default parses |
| **P3 Protocol + server** | `gliese-proto`, `gliese-client`, `gliese-server` with the `sim` host: sessions, double buffers, configure/commit, input routing, borders, cursor; everything floats | sim tests: a client opens, draws, resizes and closes; a dead client is cleaned up |
| **P4 Core** | `gliese-core`: workspaces, tiling, floating, binds, submaps, v1 dispatchers, rules, the error bar | sim tests and screenshots: three windows with exact gaps, binds move focus and workspaces, a bad config is rejected and the old one kept, the attention key cannot be bound |
| **P5 OS adapter** | `gliese-os` (`OsHost`, `run`, op 97), `metallium-desktop-boot` | builds for aarch64 with the flags the OS Makefile uses |
| **P6 Agent** | `gliesed` (+ manifest and defaults): upload, reload, exec | sim: upload → applied; broken upload → error bar |
| **P7 Terminal** | `gliese-term`, `gliese-shell`, `terminal` program; `NEW_WINDOW`; auto-reload; running programs with `Command` | host tests for the grid and every built-in; sim: Super+Enter 20 times gives 20 terminals; `nano` edits a config, and saving it changes the gaps on screen; with no spawn capability the terminal says so and the built-ins still work. On the machine (P8, after the OS change): `hello` and `ls` from `/system/bin` run in a terminal window, and Ctrl-C stops `burn` |
| **P7b Polish** | Mouse binds (move, resize, drag-to-swap), per-workspace gaps | sim tests |
| **P8 Machine** | `make run-window DESKTOP=../GlieseWM` (**writes build output in metallium-os; ask first**) | boots; screenshot over QMP |
| Later | `monocle` and `columns`, scratchpad, popups and menus, animations, a bar program, multiple outputs | |

## 10. Decisions and open questions

Decided (2026-11-18):
- Nothing is taken from metallium-os-desktop.
- There is no taskbar.
- The config lives at `/data/programdata/gliesed/gliese.conf`.
- Super+Enter opens a terminal, where `nano` edits the config.
- You can open many windows; memory decides how many, not a fixed cap (§3.2).
- The terminal runs CLI programs itself through a new OS permission, `permission = spawn` (`docs/OS-SPAWN-PERMISSION.md`). There is no broker in the compositor. `nano` is built in (§8.1).

Open:
1. **Where your CLI programs live.** Will they go in `/system/bin` like the OS's own (built into the OS image, which
   means metallium-os), or be shipped by GlieseWM as packages in its `os/` folder? `/system/bin` and `/data/bin` work as
   soon as the OS change lands. The package route would also need the terminal to read a `.map` package and run the
   program inside it, which is extra work. My recommendation is `/data/bin` (or `/system/bin`) for CLI programs.
2. **The OS change's QEMU check.** `make test` (or the `qemu-test` part of it) runs the new `progctl` check. It has
   not been run yet.

## OS hardening after the pinned revision

The metallium-os security fixes of 2026-11-18 (`SECURITY.md` in metallium-os; commits `8ab56dc` to `1aa9b5d`) changed the kernel's behaviour, not its call numbers or layouts, and touched none of the libraries GlieseWM depends on, so the pin at `6406422` stays correct. The ones a desktop can notice: `CAP_GRANT` is limited for programs (above), `SYSINFO` pages 10 to 14 need `Admin`, and `WAIT_WORD` / `WAKE_WORD` on a device address answer `-5`. By reading the code, GlieseWM uses none of the refused forms: the compositor grants from the screen task, and the clients never grant. Checked by booting on 2026-11-18, with the kernel at metallium-os `575ee3c` (headless QEMU, 4 cores, 128 MiB, serial log and QMP screenshots): GlieseWM takes the screen at 1024 x 768, the program manager installs and starts `gliesed` and gives it the desktop role, the config loads, and two Super+Enter presses open two tiled terminal windows, so the compositor's grants to client programs work. No program was terminated and nothing panicked.
