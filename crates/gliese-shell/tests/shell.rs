//! The shell with an in-memory file system: typing, built-ins, pipes, history, running programs, nano on gliese.conf.
use gliese_shell::{Effect, Entry, FsError, Shell, Sys, GLIESE_CONF};
use gliese_term::Term;
use std::collections::BTreeMap;

#[derive(Default)]
struct Fake {
    files: BTreeMap<String, Vec<u8>>,
    dirs: Vec<String>,
    reloads: u32,
}

impl Fake {
    fn new() -> Fake {
        let mut f = Fake { dirs: vec!["/".into(), "/data".into(), "/system".into(), "/data/programdata".into(), "/data/programdata/gliesed".into()], ..Default::default() };
        f.files.insert("/data/notes.txt".into(), b"banana\napple\ncherry\n".to_vec());
        f
    }
}

impl Sys for Fake {
    fn read(&mut self, p: &str) -> Result<Vec<u8>, FsError> {
        self.files.get(p).cloned().ok_or(FsError::NotFound)
    }
    fn write(&mut self, p: &str, d: &[u8], append: bool) -> Result<(), FsError> {
        let e = self.files.entry(p.into()).or_default();
        if !append {
            e.clear();
        }
        e.extend_from_slice(d);
        Ok(())
    }
    fn list(&mut self, p: &str) -> Result<Vec<Entry>, FsError> {
        if !self.dirs.iter().any(|d| d == p) {
            return Err(FsError::NotFound);
        }
        let pre = if p == "/" { "/".to_string() } else { format!("{p}/") };
        let mut v: Vec<Entry> = self.files.keys().filter_map(|f| f.strip_prefix(&pre)).filter(|r| !r.contains('/')).map(|n| Entry { name: n.into(), dir: false }).collect();
        v.extend(self.dirs.iter().filter_map(|d| d.strip_prefix(&pre)).filter(|r| !r.is_empty() && !r.contains('/')).map(|n| Entry { name: n.into(), dir: true }));
        Ok(v)
    }
    fn stat(&mut self, p: &str) -> Result<(u64, bool), FsError> {
        if self.dirs.iter().any(|d| d == p) {
            return Ok((0, true));
        }
        self.files.get(p).map(|f| (f.len() as u64, false)).ok_or(FsError::NotFound)
    }
    fn mkdir(&mut self, p: &str) -> Result<(), FsError> {
        self.dirs.push(p.into());
        Ok(())
    }
    fn remove(&mut self, p: &str) -> Result<(), FsError> {
        self.files.remove(p).map(|_| ()).ok_or(FsError::NotFound)
    }
    fn rename(&mut self, a: &str, b: &str) -> Result<(), FsError> {
        let d = self.files.remove(a).ok_or(FsError::NotFound)?;
        self.files.insert(b.into(), d);
        Ok(())
    }
    fn prog(&mut self, args: &[&str]) -> Result<String, String> {
        Ok(format!("prog {}\n", args.join(" ")))
    }
    fn reload(&mut self) -> Result<(), String> {
        self.reloads += 1;
        Ok(())
    }
    fn uptime_ms(&mut self) -> u64 {
        3_723_000
    }
    fn memory(&mut self) -> Option<(u64, u64)> {
        Some((128 << 20, 40 << 20))
    }
    fn facts(&mut self) -> Vec<(String, String)> {
        [("KERNEL", "Metal OS 0.1 aarch64 microkernel"), ("UPTIME", "0h 1m 02s"), ("CPU", "4x Cortex-A72"), ("GPU", "virtio-gpu 1024x768 (2D)"), ("RAM", "40 MiB / 128 MiB"), ("STORAGE", "virtio-blk (EROFS + LFS)"), ("IOMMU", "Arm SMMUv3, 6 devices confined"), ("TASKS", "31 running, 0 driver restarts")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

struct T {
    sh: Shell,
    term: Term,
    sys: Fake,
}

impl T {
    fn new() -> T {
        T { sh: Shell::new(80, 30), term: Term::new(), sys: Fake::new() }
    }
    fn typed(&mut self, s: &str) -> Effect {
        let mut last = Effect::None;
        for b in s.bytes() {
            last = self.sh.key(&mut self.sys, &mut self.term, b);
        }
        last
    }
    fn line(&mut self, s: &str) -> Effect {
        self.typed(s);
        self.sh.key(&mut self.sys, &mut self.term, b'\n')
    }
    /// The output since the last call (lines that are not the prompt's echo).
    fn out(&mut self) -> Vec<String> {
        let t = self.term.text();
        self.term.clear();
        t.into_iter().filter(|l| !l.contains(" $ ") && !l.is_empty()).collect()
    }
}

#[test]
fn builtins_and_pipes() {
    let mut t = T::new();
    t.line("cd /data");
    assert_eq!(t.sh.cwd, "/data");
    t.out();
    t.line("ls");
    assert_eq!(t.out(), ["notes.txt", "programdata/"]);
    t.line("cat notes.txt | sort | head -n 2");
    assert_eq!(t.out(), ["apple", "banana"]);
    t.line("grep -v an notes.txt");
    assert_eq!(t.out(), ["apple", "cherry"]);
    t.line("sort -r notes.txt > sorted.txt");
    assert!(t.out().is_empty());
    assert_eq!(t.sys.files["/data/sorted.txt"], b"cherry\nbanana\napple\n");
    t.line("echo more >> sorted.txt");
    t.line("tail -1 sorted.txt");
    assert_eq!(t.out(), ["more"]);
    t.line("wc notes.txt");
    assert_eq!(t.out(), ["3 3 20"]);
    t.line("mv sorted.txt programdata");
    assert!(t.sys.files.contains_key("/data/programdata/sorted.txt"), "into a folder keeps the name");
    t.line("cp notes.txt n2");
    t.line("rm n2");
    assert!(!t.sys.files.contains_key("/data/n2"));
    t.line("uptime");
    assert_eq!(t.out(), ["up 1:02:03"]);
    t.line("free");
    assert_eq!(t.out(), ["memory: 128 MiB, 40 MiB free"]);
    t.line("prog list");
    assert_eq!(t.out(), ["prog list"]);
    t.line("reload");
    assert_eq!(t.sys.reloads, 1);
}

#[test]
fn errors_are_said_plainly() {
    let mut t = T::new();
    t.line("cd /nowhere");
    assert_eq!(t.out(), ["cd: /nowhere: no such file or folder"]);
    t.line("cat /data/nope");
    assert_eq!(t.out(), ["cat: /data/nope: no such file or folder"]);
    t.line("echo \"unclosed");
    assert_eq!(t.out(), ["a \" is not closed"]);
    t.line("ls | hello");
    assert_eq!(t.out(), ["hello: only built-in commands can be piped or redirected (yet)"]);
    t.line("cp onlyone");
    assert_eq!(t.out(), ["cp: cp takes two paths: FROM TO"]);
}

#[test]
fn other_names_run_programs_and_their_io_goes_through() {
    let mut t = T::new();
    t.line("cd /data");
    let e = t.line("hello world");
    assert_eq!(e, Effect::Run { argv: vec!["hello".into(), "world".into()], cwd: "/data".into() });
    assert!(t.sh.running());
    t.sh.job_output(&mut t.term, b"Hello from a program\nyour name? ");
    // typing goes to the program, a line at a time
    assert_eq!(t.typed("Ann"), Effect::None);
    assert_eq!(t.sh.live().0.len(), 3);
    assert_eq!(t.sh.key(&mut t.sys, &mut t.term, b'\n'), Effect::Input(b"Ann\n".to_vec()));
    assert_eq!(t.sh.key(&mut t.sys, &mut t.term, 0x04), Effect::Eof);
    assert_eq!(t.sh.key(&mut t.sys, &mut t.term, 0x03), Effect::Interrupt);
    t.sh.job_done(&mut t.term, Some(3));
    assert!(!t.sh.running());
    let out = t.out();
    assert!(out.contains(&"Hello from a program".to_string()), "{out:?}");
    assert!(out.iter().any(|l| l.starts_with("your name? Ann")), "the program's prompt and the answer: {out:?}");
    assert_eq!(out.last().unwrap(), "[exit code 3]");
    // a program that cannot be started
    t.line("nothing");
    t.sh.job_failed(&mut t.term, "nothing", "command not found");
    assert_eq!(t.out(), ["nothing: command not found"]);
    assert!(!t.sh.running());
}

#[test]
fn line_editing_and_history() {
    let mut t = T::new();
    t.line("echo one");
    t.line("echo two");
    t.out();
    // Up twice: "echo one"; then edit it: Home, Ctrl-K … simpler: Left and insert
    t.sh.key(&mut t.sys, &mut t.term, metal_edit::K_UP);
    t.sh.key(&mut t.sys, &mut t.term, metal_edit::K_UP);
    t.sh.key(&mut t.sys, &mut t.term, metal_edit::K_LEFT);
    t.sh.key(&mut t.sys, &mut t.term, metal_edit::K_LEFT);
    t.sh.key(&mut t.sys, &mut t.term, metal_edit::K_LEFT);
    t.typed("x");
    t.sh.key(&mut t.sys, &mut t.term, b'\n');
    assert_eq!(t.out(), ["xone"]);
    // Ctrl-U clears the line, Ctrl-W a word
    t.typed("echo aa bb");
    t.sh.key(&mut t.sys, &mut t.term, 0x17);
    t.sh.key(&mut t.sys, &mut t.term, b'\n');
    assert_eq!(t.out(), ["aa"]);
    t.typed("junk");
    t.sh.key(&mut t.sys, &mut t.term, 0x15);
    t.typed("pwd\n");
    assert_eq!(t.out(), ["/"]);
    // exit closes the window
    assert_eq!(t.line("exit"), Effect::Close);
}

#[test]
fn nano_on_the_config_starts_from_the_default_and_writes_it() {
    let mut t = T::new();
    t.line("nano $GLIESE_CONF");
    assert!(t.sh.editing());
    let (cells, _) = t.sh.editor_screen().unwrap();
    let screen: String = cells.iter().map(|r| r.iter().map(|c| c.0).collect::<String>() + "\n").collect();
    assert!(screen.contains("gliese.conf"), "the file name in the header, the default's text in the buffer:\n{screen}");
    // ^O writes out (Enter accepts the name), ^X exits
    t.sh.key(&mut t.sys, &mut t.term, 0x0F);
    t.sh.key(&mut t.sys, &mut t.term, b'\n');
    t.sh.key(&mut t.sys, &mut t.term, 0x18);
    assert!(!t.sh.editing(), "back at the prompt");
    let written = String::from_utf8(t.sys.files[GLIESE_CONF].clone()).unwrap();
    assert_eq!(written.trim_end(), gliese_config::DEFAULT.trim_end());
    assert!(gliese_config::parse("x", &written, None).is_ok());
}

#[test]
fn help_lists_every_builtin() {
    let mut t = T::new();
    t.line("help");
    let out = t.out().join("\n");
    for (c, _) in gliese_shell::BUILTINS {
        assert!(out.contains(c), "{c}");
    }
}

#[test]
fn keys_become_bytes() {
    use gliese_shell::key_byte;
    assert_eq!(key_byte(30, 0, 'a' as u32), Some(b'a'));
    assert_eq!(key_byte(28, 0, 0), Some(b'\n'));
    assert_eq!(key_byte(46, 2, 0), Some(0x03), "Ctrl-C");
    assert_eq!(key_byte(24, 2, 0), Some(0x0F), "Ctrl-O (nano's write out)");
    assert_eq!(key_byte(45, 2, 0), Some(0x18), "Ctrl-X");
    assert_eq!(key_byte(103, 0, 0), Some(metal_edit::K_UP));
    assert_eq!(key_byte(14, 0, 0), Some(metal_edit::K_BACKSPACE));
    assert_eq!(key_byte(125, 8, 0), None, "Super alone");
}

#[test]
fn fetch_shows_the_logo_and_the_machine() {
    let mut t = T::new();
    t.line("fetch");
    let text = t.term.text().join("\n");
    assert!(text.contains("KERNEL   Metal OS 0.1"), "{text}");
    assert!(text.contains("TASKS    31 running"));
    // drawn: the logo's pixels are at the left, the text beside it
    let (w, h) = (1000u32, 400u32);
    let mut px = vec![0u32; (w * h) as usize];
    let (live, cur) = t.sh.live();
    t.term.draw(&mut px, w, h, &live, cur, true);
    let logo_px = (0..160).flat_map(|y| (0..160).map(move |x| (x, y))).filter(|(x, y)| px[(*y * w + *x) as usize] == gliese_term::PALETTE[6]).count();
    assert!(logo_px > 100, "the logo is drawn ({logo_px} pixels)");
    std::fs::write(concat!(env!("CARGO_TARGET_TMPDIR"), "/p7-fetch.png"), gliese_server::sim::png(w, h, &px)).unwrap();
}

#[test]
fn fetch_wraps_beside_the_logo_at_any_width() {
    let mut t = T::new();
    t.line("fetch");
    t.line("echo after");
    for (w, name) in [(1000u32, "wide"), (497, "half"), (330, "narrow")] {
        let h = 400u32;
        let mut px = vec![0u32; (w * h) as usize];
        let (live, cur) = t.sh.live();
        t.term.draw(&mut px, w, h, &live, cur, true);
        let path = format!("{}/p7-fetch-{name}.png", env!("CARGO_TARGET_TMPDIR"));
        std::fs::write(path, gliese_server::sim::png(w, h, &px)).unwrap();
        let (cols, _) = Term::grid_for(w, h);
        let logo = cols >= gliese_term::LOGO_MIN_COLS;
        // the logo's square: nothing but the logo is drawn there (text would be the plain foreground colour in the cells
        // left of the label column)
        // the die's inner square (the logo's own, 24 px wide): its top edge, in the logo's colour, at the block's first row
        let (x0, y0) = (gliese_term::PAD as u32 + 76 - 12, gliese_term::PAD as u32 + gliese_term::CELL_H as u32 + 76 - 12);
        let edge = (x0..x0 + 24).filter(|x| px[(y0 * w + x) as usize] == gliese_term::PALETTE[6]).count();
        assert_eq!(edge >= 16, logo, "{name}: logo shown only when there is room ({cols} columns)");
    }
}
