# Provenance

Everything in this repository is written from scratch for GlieseWM.

| Source | What was taken | How |
|---|---|---|
| [Hyprland](https://github.com/hyprwm/Hyprland) (BSD-3-Clause) | Ideas only: the dwindle and master layouts, `gaps_in`/`gaps_out`, the shape of the config (`bind = MODS, KEY, dispatcher, arg`, `binde`/`bindr`/`bindm`, submaps, `$variables`, sections), dispatcher names, the error bar | No code was read or copied. The config syntax is a compatible-looking subset, parsed by our own code |
| [metal-os](https://github.com/anthonydarkoonx/metal-os) (Apache-2.0 OR MIT) | The desktop contract (`docs/DESKTOP-INTERFACE.md`), the system calls (`docs/ABI.md`), the input driver's key numbers (`user/src/kbd.rs`, `libs/metal-driver-rt`) | Interfaces and numbers, used as documented. Later phases depend on its libraries (`metal-gfx`, `metal-driver-rt`, `metal-input`, `metal-programs`, `metal-edit`) as git dependencies |
| metal-os `user/src/logo.rs`, `user/src/info.rs` (Apache-2.0 OR MIT, same author) | The `fetch` logo drawing (`crates/gliese-term/src/logo.rs`) and the CPU-name table (`os/src/bin/terminal.rs`), so the terminal's `fetch` looks like the OS console's | Copied, credited in the files |
| metal-os-desktop | Nothing. It was read only to see how a desktop plugs into Metal OS | — |
