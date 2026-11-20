//! GlieseWM's shell (`PLAN.md` §8.1): the prompt, line editing and history, built-in commands (and pipes and `>` between them),
//! `nano` (metallium-os's `metallium-edit`), and running other programs, which the terminal program starts (it has `permission = spawn`)
//! and connects to this shell.
//!
//! Pure state: the file system and the program manager are behind [`Sys`]; output goes into a [`gliese_term::Term`]. Keys are
//! bytes as Metallium-OS's keyboard driver numbers them (printable ASCII, control characters, and `metallium_edit::K_*` for the others).
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

mod builtins;
mod words;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use gliese_term::{Cell, Term, DEFAULT};
use metallium_edit::{Editor, IoError, Mode, Outcome, Storage, K_BACKSPACE, K_DELETE, K_DOWN, K_END, K_HOME, K_LEFT, K_PGDN, K_PGUP, K_RIGHT, K_UP};

pub use builtins::BUILTINS;
pub use words::{resolve, split};

/// The config file as the terminal (which sees the whole disk) names it: `$GLIESE_CONF`.
pub const GLIESE_CONF: &str = "/data/programdata/gliesed/gliese.conf";
/// `~` and `$HOME`.
pub const HOME: &str = "/data";
const HISTORY: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    Other(String),
}

impl FsError {
    pub fn text(&self) -> String {
        match self {
            FsError::NotFound => "no such file or folder".into(),
            FsError::Other(s) => s.clone(),
        }
    }
}

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

/// The system, as the shell needs it.
pub trait Sys {
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError>;
    fn write(&mut self, path: &str, data: &[u8], append: bool) -> Result<(), FsError>;
    fn list(&mut self, path: &str) -> Result<Vec<Entry>, FsError>;
    /// (size in bytes, is a folder)
    fn stat(&mut self, path: &str) -> Result<(u64, bool), FsError>;
    fn mkdir(&mut self, path: &str) -> Result<(), FsError>;
    fn remove(&mut self, path: &str) -> Result<(), FsError>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    /// `prog …`: the program manager (list, start, stop, state); the text to print.
    fn prog(&mut self, args: &[&str]) -> Result<String, String>;
    /// Ask GlieseWM to read `gliese.conf` again.
    fn reload(&mut self) -> Result<(), String>;
    fn uptime_ms(&mut self) -> u64;
    /// (total, free) bytes of memory, if known.
    fn memory(&mut self) -> Option<(u64, u64)>;
    /// What `fetch` shows: (label, value) rows about the machine.
    fn facts(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// A KEY event (key number, modifiers, character) as the byte the shell and the editor take: the character, a control character
/// for Ctrl+letter, Enter, Tab, Escape, Backspace, or `metallium_edit::K_*` for arrows and the like. `None`: a key that means nothing
/// here (a modifier alone, a function key).
pub fn key_byte(code: u16, mods: u32, ch: u32) -> Option<u8> {
    use gliese_proto::keymap::{chars, mods as m};
    if ch != 0 && ch < 0x80 {
        return Some(ch as u8);
    }
    if mods & m::CTRL != 0 {
        if let Some((lo, _)) = chars(code) {
            if lo.is_ascii_lowercase() {
                return Some(lo as u8 & 0x1F);
            }
        }
    }
    Some(match code {
        1 => 0x1B,
        14 => K_BACKSPACE,
        15 => b'\t',
        28 => b'\n',
        102 => K_HOME,
        103 => K_UP,
        104 => K_PGUP,
        105 => K_LEFT,
        106 => K_RIGHT,
        107 => K_END,
        108 => K_DOWN,
        109 => K_PGDN,
        111 => K_DELETE,
        _ => return None,
    })
}

/// The editor's screen: rows of (character, inverse), and the cursor (column, row).
pub type Grid = (Vec<Vec<(char, bool)>>, (usize, usize));

/// What the terminal program has to do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Start a program (by name or path) with these arguments, connected to this shell; then [`Shell::job_output`] and
    /// [`Shell::job_done`] (or [`Shell::job_failed`]).
    Run { argv: Vec<String>, cwd: String },
    /// A line typed for the running program.
    Input(Vec<u8>),
    /// Ctrl-D: the running program's input ends.
    Eof,
    /// Ctrl-C: stop the running program.
    Interrupt,
    /// Scroll the view by rows (positive: back).
    Scroll(i32),
    /// `exit`: close this window.
    Close,
}

enum State {
    Prompt,
    /// A program runs; what is being typed for it.
    Job { line: String },
    Editor(alloc::boxed::Box<Editor>),
}

pub struct Shell {
    pub cwd: String,
    line: Vec<char>,
    cur: usize,
    history: Vec<String>,
    hist_at: Option<usize>,
    state: State,
    cols: usize,
    rows: usize,
}

/// The editor's files, through the shell's system.
struct Files<'a>(&'a mut dyn Sys);

impl Storage for Files<'_> {
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        self.0.read(path).map_err(|e| match e {
            FsError::NotFound => IoError::NotFound,
            FsError::Other(s) => IoError::Other(s),
        })
    }
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), IoError> {
        self.0.write(path, data, false).map_err(|e| IoError::Other(e.text()))
    }
}

impl Shell {
    pub fn new(cols: usize, rows: usize) -> Shell {
        Shell { cwd: "/".into(), line: Vec::new(), cur: 0, history: Vec::new(), hist_at: None, state: State::Prompt, cols, rows }
    }

    /// The first lines a new window shows.
    pub fn greet(&self, term: &mut Term) {
        term.write_str("\x1b[36mGlieseWM terminal\x1b[0m. `help` lists the commands; `nano $GLIESE_CONF` edits the config.\n");
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        if let State::Editor(ed) = &mut self.state {
            let (r, c) = metallium_edit::term::text_area(self.rows, self.cols);
            ed.resize(r, c);
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.state, State::Job { .. })
    }
    pub fn editing(&self) -> bool {
        matches!(self.state, State::Editor(_))
    }

    fn prompt(&self) -> String {
        format!("{} $ ", self.cwd)
    }

    /// The line drawn below the output: the prompt and what is typed (or what is typed for a running program), and the cursor in it.
    pub fn live(&self) -> (Vec<Cell>, usize) {
        match &self.state {
            State::Prompt => {
                let p = self.prompt();
                let mut cells: Vec<Cell> = p.bytes().map(|b| Cell { ch: b, fg: if b == b'$' { DEFAULT } else { 4 }, bg: DEFAULT }).collect();
                let n = cells.len();
                cells.extend(self.line.iter().map(|c| Cell::plain(if c.is_ascii() { *c as u8 } else { b'?' })));
                (cells, n + self.cur)
            }
            State::Job { line } => (line.bytes().map(Cell::plain).collect(), line.len()),
            State::Editor(_) => (Vec::new(), 0),
        }
    }

    /// The editor's screen when `nano` runs: cells (character, inverse) and the cursor (column, row).
    pub fn editor_screen(&self) -> Option<Grid> {
        let State::Editor(ed) = &self.state else { return None };
        let s = metallium_edit::term::draw(ed, self.rows, self.cols);
        Some((s.cells.iter().map(|r| r.iter().map(|c| (c.ch, c.inverse)).collect()).collect(), s.cursor))
    }

    /// One key.
    pub fn key(&mut self, sys: &mut dyn Sys, term: &mut Term, k: u8) -> Effect {
        match &mut self.state {
            State::Editor(ed) => {
                if ed.key(&mut Files(sys), k) == Outcome::Exit {
                    self.state = State::Prompt;
                }
                Effect::None
            }
            State::Job { line } => match k {
                0x03 => Effect::Interrupt,
                0x04 => {
                    let rest = core::mem::take(line);
                    if rest.is_empty() {
                        Effect::Eof
                    } else {
                        Effect::Input(rest.into_bytes())
                    }
                }
                b'\n' | b'\r' => {
                    let mut l = core::mem::take(line);
                    term.write_str(&l);
                    term.write(b"\n");
                    l.push('\n');
                    Effect::Input(l.into_bytes())
                }
                K_BACKSPACE | 0x7F => {
                    line.pop();
                    Effect::None
                }
                K_PGUP => Effect::Scroll(self.rows as i32 / 2),
                K_PGDN => Effect::Scroll(-(self.rows as i32 / 2)),
                0x20..=0x7E | b'\t' => {
                    line.push(k as char);
                    Effect::None
                }
                _ => Effect::None,
            },
            State::Prompt => self.prompt_key(sys, term, k),
        }
    }

    fn prompt_key(&mut self, sys: &mut dyn Sys, term: &mut Term, k: u8) -> Effect {
        match k {
            b'\n' | b'\r' => {
                let line: String = self.line.iter().collect();
                self.line.clear();
                self.cur = 0;
                self.hist_at = None;
                term.write_str(&self.prompt());
                term.write_str(&line);
                term.write(b"\n");
                return self.run_line(sys, term, line.trim());
            }
            0x20..=0x7E => {
                self.line.insert(self.cur, k as char);
                self.cur += 1;
            }
            b'\t' => {
                // (no completion yet: a tab is four spaces)
                for _ in 0..4 {
                    self.line.insert(self.cur, ' ');
                    self.cur += 1;
                }
            }
            K_BACKSPACE | 0x7F if self.cur > 0 => {
                self.cur -= 1;
                self.line.remove(self.cur);
            }
            K_DELETE if self.cur < self.line.len() => {
                self.line.remove(self.cur);
            }
            K_LEFT => self.cur = self.cur.saturating_sub(1),
            K_RIGHT => self.cur = (self.cur + 1).min(self.line.len()),
            K_HOME | 0x01 => self.cur = 0,
            K_END | 0x05 => self.cur = self.line.len(),
            0x15 => {
                self.line.drain(..self.cur);
                self.cur = 0;
            }
            0x0B => self.line.truncate(self.cur),
            0x17 => {
                // Ctrl-W: the word before the cursor
                let mut start = self.cur;
                while start > 0 && self.line[start - 1] == ' ' {
                    start -= 1;
                }
                while start > 0 && self.line[start - 1] != ' ' {
                    start -= 1;
                }
                self.line.drain(start..self.cur);
                self.cur = start;
            }
            0x0C => term.clear(),
            0x03 => {
                let line: String = self.line.iter().collect();
                term.write_str(&self.prompt());
                term.write_str(&line);
                term.write(b"^C\n");
                self.line.clear();
                self.cur = 0;
            }
            K_UP | K_DOWN => self.history_step(k == K_UP),
            K_PGUP => return Effect::Scroll(self.rows as i32 / 2),
            K_PGDN => return Effect::Scroll(-(self.rows as i32 / 2)),
            _ => {}
        }
        Effect::None
    }

    fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let at = match (self.hist_at, back) {
            (None, true) => Some(self.history.len() - 1),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        self.hist_at = at;
        self.line = at.map_or_else(Vec::new, |i| self.history[i].chars().collect());
        self.cur = self.line.len();
    }

    /// Run a command line.
    pub fn run_line(&mut self, sys: &mut dyn Sys, term: &mut Term, line: &str) -> Effect {
        if line.is_empty() {
            return Effect::None;
        }
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.into());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        let words = match words::split(line, &self.cwd) {
            Ok(w) => w,
            Err(e) => {
                term.write_str(&format!("{e}\n"));
                return Effect::None;
            }
        };
        // stages separated by `|`, and a `>` or `>>` file at the end
        let mut stages: Vec<Vec<String>> = alloc::vec![Vec::new()];
        let mut redirect: Option<(String, bool)> = None;
        let mut it = words.into_iter().peekable();
        while let Some(w) = it.next() {
            match w.as_str() {
                "|" => stages.push(Vec::new()),
                ">" | ">>" => {
                    let Some(f) = it.next() else {
                        term.write_str("expected a file name after >\n");
                        return Effect::None;
                    };
                    redirect = Some((resolve(&self.cwd, &f), w == ">>"));
                    if it.peek().is_some() {
                        term.write_str("> goes at the end of the line\n");
                        return Effect::None;
                    }
                }
                _ => stages.last_mut().expect("a stage").push(w),
            }
        }
        if stages.iter().any(|s| s.is_empty()) {
            term.write_str("empty command around |\n");
            return Effect::None;
        }
        let first = stages[0][0].clone();
        // what changes the shell or the window itself
        if stages.len() == 1 && redirect.is_none() {
            let args: Vec<&str> = stages[0][1..].iter().map(String::as_str).collect();
            match first.as_str() {
                "exit" => return Effect::Close,
                "clear" => {
                    term.clear();
                    return Effect::None;
                }
                "cd" => {
                    self.cd(sys, term, args.first().copied().unwrap_or(HOME));
                    return Effect::None;
                }
                "fetch" => {
                    self.fetch(sys, term);
                    return Effect::None;
                }
                "history" => {
                    for (i, h) in self.history.iter().enumerate() {
                        term.write_str(&format!("{:4}  {h}\n", i + 1));
                    }
                    return Effect::None;
                }
                "nano" | "edit" => {
                    self.nano(sys, args.first().copied());
                    return Effect::None;
                }
                _ => {}
            }
        }
        if !builtins::is_builtin(&first) {
            if stages.len() == 1 && redirect.is_none() {
                self.state = State::Job { line: String::new() };
                return Effect::Run { argv: stages.remove(0), cwd: self.cwd.clone() };
            }
            term.write_str(&format!("{first}: only built-in commands can be piped or redirected (yet)\n"));
            return Effect::None;
        }
        let mut input: Option<String> = None;
        for stage in &stages {
            let name = stage[0].as_str();
            if !builtins::is_builtin(name) {
                term.write_str(&format!("{name}: only built-in commands can be piped or redirected (yet)\n"));
                return Effect::None;
            }
            let args: Vec<&str> = stage[1..].iter().map(String::as_str).collect();
            match builtins::run(sys, &self.cwd, name, &args, input.as_deref()) {
                Ok(out) => input = Some(out),
                Err(e) => {
                    term.write_str(&format!("{name}: {e}\n"));
                    return Effect::None;
                }
            }
        }
        let out = input.unwrap_or_default();
        match redirect {
            Some((path, append)) => {
                if let Err(e) = sys.write(&path, out.as_bytes(), append) {
                    term.write_str(&format!("{path}: {}\n", e.text()));
                }
            }
            None => term.write_str(&out),
        }
        Effect::None
    }

    /// The machine at a glance, beside the logo (as the OS console's `fetch` shows it).
    fn fetch(&mut self, sys: &mut dyn Sys, term: &mut Term) {
        let mut rows = sys.facts();
        rows.truncate(gliese_term::LOGO_ROWS);
        term.logo();
        let pad = " ".repeat(gliese_term::LOGO_COLS);
        for i in 0..gliese_term::LOGO_ROWS {
            match rows.get(i) {
                Some((k, v)) => term.write_str(&format!("{pad}\x1b[36m{k:<9}\x1b[0m{v}\n")),
                None => term.write_str("\n"),
            }
        }
    }

    fn cd(&mut self, sys: &mut dyn Sys, term: &mut Term, to: &str) {
        let p = resolve(&self.cwd, to);
        match sys.stat(&p) {
            Ok((_, true)) => self.cwd = p,
            Ok((_, false)) => term.write_str(&format!("cd: {p} is a file\n")),
            Err(e) => term.write_str(&format!("cd: {p}: {}\n", e.text())),
        }
    }

    fn nano(&mut self, sys: &mut dyn Sys, file: Option<&str>) {
        let mut ed = Editor::new(Mode::Nano);
        ed.cwd = self.cwd.clone();
        let (r, c) = metallium_edit::term::text_area(self.rows, self.cols);
        ed.resize(r, c);
        if let Some(f) = file {
            let path = resolve(&self.cwd, f);
            let missing = matches!(sys.read(&path), Err(FsError::NotFound));
            ed.open(&mut Files(sys), &path);
            // GlieseWM's config, edited for the first time: start from the built-in one
            if missing && path == GLIESE_CONF {
                ed.set_text(gliese_config::DEFAULT);
                ed.modified = true;
                ed.msg = "New file: the built-in config, to change and write out (^O)".to_string();
            }
        }
        self.state = State::Editor(alloc::boxed::Box::new(ed));
    }

    /// Output from the running program.
    pub fn job_output(&mut self, term: &mut Term, bytes: &[u8]) {
        term.write(bytes);
    }

    /// The program ended: `code` is its exit code (`None`: it could not be told).
    pub fn job_done(&mut self, term: &mut Term, code: Option<u64>) {
        match code {
            Some(0) | None => {}
            Some(u64::MAX) => term.write_str("\x1b[31m[it crashed]\x1b[0m\n"),
            Some(c) if c == u64::MAX - 1 => term.write_str("[stopped]\n"),
            Some(c) => term.write_str(&format!("\x1b[33m[exit code {c}]\x1b[0m\n")),
        }
        self.end_job(term);
    }

    /// The program could not be started.
    pub fn job_failed(&mut self, term: &mut Term, name: &str, why: &str) {
        term.write_str(&format!("{name}: {why}\n"));
        self.end_job(term);
    }

    fn end_job(&mut self, term: &mut Term) {
        // a program's last line without a newline: the prompt starts on a line of its own
        if term.text().last().is_some_and(|l| !l.is_empty()) {
            term.write(b"\n");
        }
        self.state = State::Prompt;
    }
}
