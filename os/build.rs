// Position-independent Metal OS programs link with metal-os's own linker script; METAL_OS_DIR names that checkout (the OS Makefile
// sets it). Each program carries its name (-soname), as the OS's own programs do.
fn main() {
    let os = std::env::var("METAL_OS_DIR").expect("METAL_OS_DIR is not set: build with `make run-window DESKTOP=<GlieseWM>` in metal-os");
    println!("cargo:rustc-link-arg=-T{os}/programs/link-pie.ld");
    for e in std::fs::read_dir("src/bin").expect("src/bin").flatten() {
        if let Some(n) = e.file_name().to_str().and_then(|n| n.strip_suffix(".rs")) {
            println!("cargo:rustc-link-arg-bin={n}=-soname={n}");
        }
    }
    println!("cargo:rerun-if-changed=src/bin");
    println!("cargo:rerun-if-env-changed=METAL_OS_DIR");
}
