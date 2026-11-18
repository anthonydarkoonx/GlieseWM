//! GlieseWM's side of the Metal OS desktop contract (metal-os `docs/DESKTOP-INTERFACE.md`).
//!
//! When the OS image is built with `make run-window DESKTOP=<this repository>`, the screen task (role 13) calls
//! [`metal_desktop_run`] instead of starting its console, and never gets control back. The arguments are plain integers
//! (capability slots), so the OS needs none of GlieseWM's types.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use gliese_core::Gliese;
use gliese_os::{run, Config};

/// Take over the screen: serve the window protocol on `display`, draw through `gpu`, read `input`, scan out `framebuffer` (mapped
/// at `fb_handle`). `manager` is the program manager's task id (0: none), `power` the power right's slot (0: none).
///
/// GlieseWM starts with its built-in `gliese.conf`; `gliesed` uploads the user's file once it runs.
pub fn metal_desktop_run(display: u64, gpu: u64, input: u64, framebuffer: u64, fb_handle: usize, manager: u32, power: u64) -> ! {
    // This crate's copy of `metal-driver-rt` has its own statics: like every boot service, the screen task lets the kernel choose
    // where its mappings go (the kernel refuses fixed addresses), so tell this copy too.
    metal_driver_rt::auto_maps();
    let cfg = Config { display, gpu, input, framebuffer, fb_handle, manager: (manager != 0).then_some(manager), power: (power != 0).then_some(power) };
    run(cfg, Box::new(Gliese::with_default()))
}
