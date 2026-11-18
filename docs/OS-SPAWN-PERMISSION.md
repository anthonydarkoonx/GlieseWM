# OS change: `permission = spawn`

**Applied to metal-os on 2026-11-18** (commit `6406422`, merged into `main`), at the user's request. This file is the
design record; metal-os `docs/PROGRAMS.md`, *Starting programs*, is the reference. Differences from the proposal below: the test
manager's budget went from 500 to 800 pages, and `prog info` prints the permission. `mkmap.py` needed no change.

## Why

A program the program manager starts holds **no spawn capability** (`docs/PROGRAMS.md`, "What an ability gets"). So a
packaged shell, such as GlieseWM's `terminal`, cannot run command-line programs.

Today only the OS console can run them, and only because the kernel gives role 13 a spawn capability in slot 8
(`kernel/src/task.rs:390`). A desktop that replaces the console inherits that slot by accident, and the desktop contract
does not mention it.

The fix is a manifest permission. With it, a program declares that it starts programs, the user can see and revoke that
like any other permission, and the program gets a spawn capability that can never give its children more than the
program has itself.

## What a program with `spawn` gets

| Slot | Capability | Rights |
|---|---|---|
| 5 | endpoint factory | `R_EP_CREATE` (no grant). Needed for the child's console endpoint (stdin/stdout) |
| 6 | spawn | `R_SPAWN`, plus `R_SPAWN_FS` **only with `allfiles`**, plus `R_SPAWN_NET` **only with `network`**. No `R_GRANT` |

Slots 5 and 6 are where `metal-programs` already looks (`programs/src/process.rs:12-13`: `FACTORY_SLOT = 5`,
`SPAWN_SLOT = 6`). So `Command::new("ls").spawn()` works in such a program with no library change. `Command::start`
already retries with less (no network, then no files) when the parent may not hand something out
(`process.rs:254`).

**The rules, and the reasons for them:**
- **File access for children needs `allfiles`, not just `files`.** A `files` program is confined to
  `/data/programdata/<id>`, but a confinement belongs to one session (`docs/ABI.md`, `SCOPE`). A child started with file
  access gets a new, *unconfined* session unless its parent confines it. Giving `R_SPAWN_FS` to a confined program would
  let it reach the whole disk through a child. A `files`-only program with `spawn` can still start programs, but they get
  no file access.
- **Network for children needs `network`**, for the same reason.
- **No `R_GRANT`.** A child cannot start programs of its own. The OS console's copy does carry grant, so this is a
  deliberate narrowing. It can be revisited later (for example with `permission = spawn` on the child's own package).
- **Budget.** `SPAWN` takes the pages a program may hand down to its children (request offset 8; `docs/ABI.md`). The
  manager hands down from its own budget (24000 pages, `user/src/programs.rs:345`). A new manifest key,
  `spawn_pages = N` (default 4096 when `spawn` is declared, otherwise 0, at most 16384), sets it. A child that does not fit
  is refused with `-8`, as for any spawn.
- **Lifetime.** Children die with their parent (`docs/ABI.md` lifecycle), so stopping or uninstalling the terminal ends
  everything it started. `WAIT` and `KILL` work, because the terminal is their parent.

## Changes, file by file

### 1. `user/src/programs.rs`: let the manager hand the two capabilities on
`console_session` gives the manager its factory and spawn capabilities **without** the grant right:
```rust
.give_masked(5, FACTORY_SLOT as u8, 0xFFFF & !4).give_masked(6, SPAWN_SLOT as u8, 0xFFFF & !4)   // line 345
```
Change both masks to `0xFFFF`, so the grant right stays. Comment: "grant: programs with `permission = spawn` get an
attenuated copy". The manager already holds the admin capability with grant, so this does not change who is trusted.
It only lets the manager pass on a narrowed copy.

### 2. `libs/metal-map/src/lib.rs`: the permission and the key
- In the doc comment (line 29), the list becomes `files | network | display | shell | allfiles | admin | spawn`.
- Add `pub spawn: bool` and `pub spawn_pages: u32` to the manifest struct (near line 139), with defaults false and 0.
- `Permission::Spawn`, matched in the permission accessor (near line 156).
- Parse `"spawn" => m.spawn = true` (near line 261), and the key `spawn_pages` (1..=16384). Add `spawn_pages` to the
  known keys at line 205.
- After parsing: `spawn_pages` without `spawn` is an error ("spawn_pages needs permission = spawn"). With `spawn` and no
  `spawn_pages`, the default is 4096.
- Tests: parse, the default, the error, and a round trip through the builder.

### 3. `programs/src/bin/progmgr.rs`: give it, and let it be taken away
- **Permission bit 6** = spawn. Every `0x3F` becomes `0x7F`:
  - lines 163, 321 and 386;
  - START bits 16..22 (line 499), POLICY_SET (line 529), GRANT (line 530);
  - STATE reports the bits it ran without in 25..31.
  - The comments at lines 82, 155 and 33 list the bits.
- **In `start` (line 323)**, unpack `spawn = m.spawn && off & 64 == 0`. If it is set:
  ```rust
  let spawn_rights = R_SPAWN | if allfiles { R_SPAWN_FS } else { 0 } | if network { R_SPAWN_NET } else { 0 };
  cmd.give(FACTORY_SLOT_MGR, 5, R_EP_CREATE).give(SPAWN_SLOT_MGR, 6, spawn_rights).budget_pages(m.spawn_pages);
  ```
  The manager's own slots are 5 and 6 as well: the same numbers, by coincidence of the console's layout. Name them as
  constants.
- **INFO (line 546):** `m.spawn` goes in **bit 32** of the reply. Bits 24..31 are full.
- **Taking it away while the program runs:** `REVOKE_OF` already works on any delegated capability. `GRANT` (line 462)
  gains a `6 => m.spawn` case that gives back both slots. v1 may leave this out and apply a change at the next start,
  like `files` (`docs/PROGRAMS.md`, "Taking away and giving back, live").

### 4. `programs/src/bin/progctl.rs` and `user/src/shell.rs` (`prog info`)
Show `spawn` in the permission list. Accept `spawn` wherever permission names are parsed (`prog start --without spawn`,
the policy commands).

### 5. `tools/mkmap.py`
If it checks permission names, add `spawn`.

### 6. Documentation
- `docs/PROGRAMS.md` line 18: the permission list. Line 42 ("No spawn capability") becomes "a spawn capability only with
  `permission = spawn`", with the table above.
- The bit order "(bit 0 files … 5 admin)" gains "6 spawn" (lines 71, 99 and 104).
- `docs/DESKTOP-INTERFACE.md`: one line saying a desktop's command-line shells use `permission = spawn` and must not rely
  on the screen task's own slots.
- `docs/BOARD-SYNC.md`: log the change (the rule: OS changes in the general tree, logged).

### 7. Tests (fast host tests, plus the program scenario)
- `cargo test -p metal-map`: the new parse tests.
- `progctl` scenario (`programs/src/bin/progctl.rs`, run by `make test`):
  - a test package with `permission = spawn` starts `hello` and reads its output;
  - one without it gets `PermissionDenied`;
  - one with `spawn` but only `files` starts a child that has no file access;
  - stopping the parent ends the child.

  This is a `make test` (QEMU) run, so it needs your go-ahead under the metal-os rules.

## What does not change

The kernel, the ABI, the syscall numbers, the desktop contract's function signature, and the console's own capabilities
(slot 8 stays the console's). The change is confined to the manager, the manifest format, two tools and the docs.
