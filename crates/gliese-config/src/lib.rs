//! `gliese.conf`: GlieseWM's configuration (`PLAN.md` §5).
//!
//! Line-oriented, Hyprland-shaped:
//!
//! ```text
//! $mod = SUPER                         # a variable, used as $mod afterwards
//! general {                            # a section (or the flat form: general:gaps_in = 6)
//!     gaps_in = 6
//! }
//! bind = $mod, Return, exec, terminal  # bind[e|r|m] = MODS, KEY, dispatcher[, arg]
//! submap = resize                      # binds from here on belong to the `resize` submap …
//! submap = reset                       # … until this
//! windowrule = float, title:*Settings*
//! workspace = 3, gaps_in:0, layout:master
//! exec-once = terminal
//! source = other.conf                  # through the caller's `include`
//! ```
//!
//! [`parse`] checks everything and reports every error with its line and column; a config with errors is not used (the caller
//! keeps the one it had). The built-in default is [`DEFAULT`].
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod actions;
pub mod keys;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

pub use actions::{Action, Delta, Dir, Ws};
pub use gliese_layout::{LayoutSettings, Orientation, Ratio, Sides};
pub use keys::{mods, Key};

/// The default configuration: what applies before (or without) a user's file. It parses without errors (tested).
pub const DEFAULT: &str = include_str!("../gliese.conf");

/// The most errors one parse reports (the rest are usually follow-ups of the first ones).
pub const MAX_ERRORS: usize = 20;
/// How deep `source` may nest.
pub const MAX_SOURCE_DEPTH: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// `gliese.conf`, or the path a `source` line named.
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}: {}", self.file, self.line, self.col, self.msg)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct General {
    pub layout: String,
    pub gaps_in: i32,
    pub gaps_out: Sides,
    pub border_size: i32,
    pub smart_gaps: bool,
    /// `0xAARRGGBB`.
    pub active_border: u32,
    pub inactive_border: u32,
    /// Screen edges no tile covers (for a bar).
    pub reserved: Sides,
    /// The screen's background.
    pub background: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misc {
    pub auto_reload: bool,
    pub drop_hidden_buffers: bool,
    pub max_windows: u32,
    pub buffer_budget_mib: u32,
    pub min_tile: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub follow_mouse: bool,
}

/// `bind` flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BindFlags {
    /// `binde`: repeats while the key is held.
    pub repeat: bool,
    /// `bindr`: fires when the key is released.
    pub release: bool,
    /// `bindm`: a mouse drag (`movewindow`, `resizewindow`) while the button is held.
    pub mouse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    /// The submap it belongs to; `None` is the normal one.
    pub submap: Option<String>,
    pub mods: u8,
    pub key: Key,
    pub flags: BindFlags,
    pub action: Action,
    /// Where it was written (for messages).
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleAction {
    Float,
    Tile,
    Fullscreen,
    Center,
    Pin,
    Workspace { n: u8, silent: bool },
    Size(i32, i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Matcher {
    /// A glob on the window's title.
    Title(String),
    /// A glob on the id of the program the window belongs to.
    Program(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRule {
    pub action: RuleAction,
    /// All must match.
    pub matchers: Vec<Matcher>,
}

impl WindowRule {
    pub fn matches(&self, title: &str, program: &str) -> bool {
        self.matchers.iter().all(|m| match m {
            Matcher::Title(g) => glob(g, title),
            Matcher::Program(g) => glob(g, program),
        })
    }
}

/// `workspace = N, …`: overrides for one workspace.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WorkspaceRule {
    pub n: u8,
    pub gaps_in: Option<i32>,
    pub gaps_out: Option<Sides>,
    pub layout: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub general: General,
    pub layouts: LayoutSettings,
    pub misc: Misc,
    pub input: Input,
    pub binds: Vec<Bind>,
    pub rules: Vec<WindowRule>,
    pub workspaces: Vec<WorkspaceRule>,
    pub exec_once: Vec<String>,
}

impl Default for Config {
    /// The settings with nothing set (no binds): what each key is before a file says otherwise. Not [`DEFAULT`], which also has the
    /// default binds.
    fn default() -> Config {
        Config {
            general: General {
                layout: "dwindle".into(),
                gaps_in: 6,
                gaps_out: Sides::all(12),
                border_size: 2,
                smart_gaps: false,
                active_border: 0xFF5E_81AC,
                inactive_border: 0xFF3B_4252,
                reserved: Sides::all(0),
                background: 0xFF2E_3440,
            },
            layouts: LayoutSettings::default(),
            misc: Misc { auto_reload: true, drop_hidden_buffers: true, max_windows: 256, buffer_budget_mib: 32, min_tile: 64 },
            input: Input { follow_mouse: false },
            binds: Vec::new(),
            rules: Vec::new(),
            workspaces: Vec::new(),
            exec_once: Vec::new(),
        }
    }
}

impl Config {
    /// The overrides for workspace `n`, if any.
    pub fn workspace(&self, n: u8) -> Option<&WorkspaceRule> {
        self.workspaces.iter().find(|w| w.n == n)
    }
    /// The bind for a key in a submap.
    pub fn bind_for(&self, submap: Option<&str>, m: u8, key: Key, release: bool) -> Option<&Bind> {
        self.binds.iter().find(|b| b.submap.as_deref() == submap && b.mods == m && b.key == key && b.flags.release == release && !b.flags.mouse)
    }
}

/// Reads the file a `source = path` line names (the path as written); `None` if it cannot be read.
pub type Include<'a> = &'a mut dyn FnMut(&str) -> Option<String>;

/// Parse a config. `file` names it in errors; `include` serves `source` lines (without one, `source` is an error).
pub fn parse(file: &str, text: &str, include: Option<Include<'_>>) -> Result<Config, Vec<Error>> {
    let mut p = Parser { cfg: Config::default(), vars: Vec::new(), errors: Vec::new(), submap: None, submaps: Vec::new(), include };
    p.text(file, text, 0);
    p.finish(file, text);
    if p.errors.is_empty() {
        Ok(p.cfg)
    } else {
        Err(p.errors)
    }
}

/// The built-in default, parsed.
pub fn default_config() -> Config {
    parse("default", DEFAULT, None).expect("the built-in default parses")
}

struct Parser<'i> {
    cfg: Config,
    vars: Vec<(String, String)>,
    errors: Vec<Error>,
    submap: Option<String>,
    /// Submaps defined, with the line where.
    submaps: Vec<(String, u32)>,
    include: Option<Include<'i>>,
}

/// Where in a line a piece of it starts (1-based column), for messages.
fn col_of(line: &str, part: &str) -> u32 {
    let base = line.as_ptr() as usize;
    let p = part.as_ptr() as usize;
    if p >= base && p <= base + line.len() {
        (p - base) as u32 + 1
    } else {
        1
    }
}

impl Parser<'_> {
    fn err(&mut self, file: &str, line: u32, col: u32, msg: impl Into<String>) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(Error { file: file.into(), line, col, msg: msg.into() });
        }
    }

    fn text(&mut self, file: &str, text: &str, depth: usize) {
        let mut sections: Vec<&str> = Vec::new();
        let mut last = 0;
        for (i, raw) in text.lines().enumerate() {
            let ln = i as u32 + 1;
            last = ln;
            let code = raw.split('#').next().unwrap_or("");
            let line = code.trim();
            if line.is_empty() {
                continue;
            }
            if line == "}" {
                if sections.pop().is_none() {
                    self.err(file, ln, col_of(raw, line), "`}` without an open section");
                }
                continue;
            }
            if let Some(name) = line.strip_suffix('{') {
                let name = name.trim();
                if !ident_ok(name) {
                    self.err(file, ln, col_of(raw, line), "a section needs a name: `name {`");
                } else if sections.len() == 2 {
                    self.err(file, ln, col_of(raw, line), "sections nest at most two deep");
                }
                sections.push(name);
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                self.err(file, ln, col_of(raw, line), "expected `key = value`");
                continue;
            };
            let (key, value) = (k.trim(), v.trim());
            let (kcol, vcol) = (col_of(raw, key), if value.is_empty() { col_of(raw, v) } else { col_of(raw, value) });
            // variables: defined anywhere, used from then on
            if let Some(name) = key.strip_prefix('$') {
                if !ident_ok(name) {
                    self.err(file, ln, kcol, "a variable name is letters, digits, - and _");
                    continue;
                }
                match self.substitute(value) {
                    Ok(v) => {
                        self.vars.retain(|(n, _)| n != name);
                        self.vars.push((name.into(), v));
                    }
                    Err(m) => self.err(file, ln, vcol, m),
                }
                continue;
            }
            let value = match self.substitute(value) {
                Ok(v) => v,
                Err(m) => {
                    self.err(file, ln, vcol, m);
                    continue;
                }
            };
            if !sections.is_empty() {
                let mut path = sections.join(":");
                path.push(':');
                path.push_str(key);
                if let Err(m) = set(&mut self.cfg, &path, &value) {
                    self.err(file, ln, if m == UNKNOWN_KEY { kcol } else { vcol }, msg_for(m, &path));
                }
                continue;
            }
            let r = match key {
                k if k.starts_with("bind") => self.bind(k, &value, ln),
                "windowrule" => rule(&value).map(|r| self.cfg.rules.push(r)),
                "workspace" => workspace(&value).map(|w| {
                    self.cfg.workspaces.retain(|o| o.n != w.n);
                    self.cfg.workspaces.push(w);
                }),
                "submap" => {
                    if value == "reset" {
                        self.submap = None;
                        Ok(())
                    } else if ident_ok(&value) {
                        if !self.submaps.iter().any(|(n, _)| *n == value) {
                            self.submaps.push((value.clone(), ln));
                        }
                        self.submap = Some(value.clone());
                        Ok(())
                    } else {
                        Err(String::from("submap takes a name, or `reset`"))
                    }
                }
                "exec-once" => actions::parse("exec", &value).map(|a| {
                    if let Action::Exec(p) = a {
                        self.cfg.exec_once.push(p)
                    }
                }).map_err(String::from),
                "source" => {
                    self.source(file, &value, ln, vcol, depth);
                    Ok(())
                }
                k if k.contains(':') => set(&mut self.cfg, k, &value).map_err(|m| msg_for(m, k)),
                _ => Err(format!("unknown keyword `{key}` (settings go in a section: `general {{ {key} = … }}`)")),
            };
            if let Err(m) = r {
                let col = if m.starts_with("unknown keyword") { kcol } else { vcol };
                self.err(file, ln, col, m);
            }
        }
        if !sections.is_empty() {
            self.err(file, last.max(1), 1, format!("section `{}` is not closed with `}}`", sections.join(":")));
        }
    }

    fn source(&mut self, file: &str, path: &str, ln: u32, col: u32, depth: usize) {
        if depth + 1 >= MAX_SOURCE_DEPTH {
            self.err(file, ln, col, "`source` nests too deep (a file that includes itself?)");
            return;
        }
        let Some(inc) = self.include.as_mut() else {
            self.err(file, ln, col, "`source` is not available here");
            return;
        };
        match inc(path) {
            Some(text) => {
                let path = path.to_string();
                self.text(&path, &text, depth + 1);
            }
            None => self.err(file, ln, col, format!("cannot read `{path}`")),
        }
    }

    /// `$name` → its value. A `$` not followed by a defined name is an error.
    fn substitute(&self, v: &str) -> Result<String, String> {
        if !v.contains('$') {
            return Ok(v.into());
        }
        let mut out = String::new();
        let mut rest = v;
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let end = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(after.len());
            let name = &after[..end];
            match self.vars.iter().find(|(n, _)| n == name) {
                Some((_, val)) => out.push_str(val),
                None => return Err(format!("`${name}` is not defined (define it above: `${name} = …`)")),
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn bind(&mut self, key: &str, value: &str, ln: u32) -> Result<(), String> {
        let mut flags = BindFlags::default();
        for c in key["bind".len()..].chars() {
            let f = match c {
                'e' => &mut flags.repeat,
                'r' => &mut flags.release,
                'm' => &mut flags.mouse,
                _ => return Err(format!("unknown keyword `{key}` (bind, binde, bindr or bindm)")),
            };
            if *f {
                return Err(format!("unknown keyword `{key}`: a flag twice"));
            }
            *f = true;
        }
        if flags.mouse && (flags.repeat || flags.release) {
            return Err("bindm cannot be combined with e or r".into());
        }
        let mut parts = value.splitn(4, ',');
        let (Some(m), Some(k), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
            return Err("a bind is: MODS, KEY, dispatcher[, argument]".into());
        };
        let arg = parts.next().unwrap_or("").trim();
        let m = keys::parse_mods(m.trim()).map_err(|bad| format!("unknown modifier `{bad}` (SUPER, SHIFT, CTRL, ALT)"))?;
        let k = keys::parse_key(k).ok_or_else(|| format!("unknown key `{}`", k.trim()))?;
        if keys::reserved(m, k) {
            return Err("Ctrl+Alt+Delete and Ctrl+Shift+Escape are the system's and cannot be bound".into());
        }
        let action = actions::parse(d.trim(), arg).map_err(|e| if e == "unknown dispatcher" { format!("unknown dispatcher `{}`", d.trim()) } else { e.into() })?;
        let mouse_action = matches!(action, Action::MouseMove | Action::MouseResize);
        if flags.mouse != mouse_action {
            return Err(if flags.mouse { "bindm takes movewindow or resizewindow (with no argument)".into() } else { "movewindow without a direction, and resizewindow, are for bindm".into() });
        }
        if flags.mouse && !matches!(k, Key::Mouse(_)) {
            return Err("bindm needs a mouse button (mouse:left, mouse:right, …)".into());
        }
        let submap = self.submap.clone();
        if let Some(prev) = self.cfg.binds.iter().find(|b| b.submap == submap && b.mods == m && b.key == k && b.flags == flags) {
            return Err(format!("this key is already bound on line {}", prev.line));
        }
        self.cfg.binds.push(Bind { submap, mods: m, key: k, flags, action, line: ln });
        Ok(())
    }

    /// Checks that need the whole file: every submap a bind enters exists, and every submap has a way back out.
    fn finish(&mut self, file: &str, _text: &str) {
        let binds = self.cfg.binds.clone();
        for b in &binds {
            if let Action::Submap(name) = &b.action {
                if name != "reset" && !self.submaps.iter().any(|(n, _)| n == name) {
                    self.err(file, b.line, 1, format!("submap `{name}` is not defined (`submap = {name}` … `submap = reset`)"));
                }
            }
        }
        for (name, line) in self.submaps.clone() {
            let out = binds.iter().any(|b| b.submap.as_deref() == Some(&name) && matches!(&b.action, Action::Submap(n) if n == "reset"));
            if !out {
                self.err(file, line, 1, format!("submap `{name}` has no bind back out (`bind = , Escape, submap, reset`): you would be stuck in it"));
            }
        }
    }
}

const UNKNOWN_KEY: &str = "unknown key";

fn msg_for(m: &'static str, path: &str) -> String {
    if m == UNKNOWN_KEY {
        format!("unknown key `{path}`")
    } else {
        format!("{path}: {m}")
    }
}

/// A setting, by its full path (`general:gaps_in`). The table every section's keys are in.
fn set(c: &mut Config, path: &str, v: &str) -> Result<(), &'static str> {
    let g = &mut c.general;
    let l = &mut c.layouts;
    let m = &mut c.misc;
    match path {
        "general:layout" => g.layout = actions::layout_name(v)?.into(),
        "general:gaps_in" => g.gaps_in = int(v, 0, 200)?,
        "general:gaps_out" => g.gaps_out = sides(v, 200)?,
        "general:border_size" => g.border_size = int(v, 0, 20)?,
        "general:smart_gaps" => g.smart_gaps = boolean(v)?,
        "general:col.active_border" => g.active_border = color(v)?,
        "general:col.inactive_border" => g.inactive_border = color(v)?,
        "general:col.background" => g.background = color(v)?,
        "general:reserved" => g.reserved = sides(v, 2000)?,
        "dwindle:split_ratio" => l.dwindle_split_ratio = ratio(v)?,
        "dwindle:force_split" => l.dwindle_force_split = int(v, 0, 2)? as u8,
        "dwindle:preserve_split" => l.dwindle_preserve_split = boolean(v)?,
        "master:mfact" => l.master_mfact = ratio(v)?,
        "master:nmaster" => l.master_nmaster = int(v, 1, 8)? as u8,
        "master:orientation" => l.master_orientation = actions::orientation(v).ok_or("must be left, right, top, bottom or center")?,
        "master:new_on_top" => l.master_new_on_top = boolean(v)?,
        "misc:auto_reload" => m.auto_reload = boolean(v)?,
        "misc:drop_hidden_buffers" => m.drop_hidden_buffers = boolean(v)?,
        "misc:max_windows" => m.max_windows = int(v, 1, 1024)? as u32,
        "misc:buffer_budget_mib" => m.buffer_budget_mib = int(v, 4, 1024)? as u32,
        "misc:min_tile" => m.min_tile = int(v, 16, 1000)?,
        "input:follow_mouse" => c.input.follow_mouse = boolean(v)?,
        _ => return Err(UNKNOWN_KEY),
    }
    Ok(())
}

fn int(v: &str, lo: i32, hi: i32) -> Result<i32, &'static str> {
    let n: i32 = v.parse().map_err(|_| "expected a whole number")?;
    if n < lo || n > hi {
        return Err("number out of range");
    }
    Ok(n)
}

fn boolean(v: &str) -> Result<bool, &'static str> {
    match v {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err("expected true or false"),
    }
}

/// One value for all four sides, or four: top right bottom left.
fn sides(v: &str, max: i32) -> Result<Sides, &'static str> {
    let mut n = [0i32; 4];
    let mut count = 0;
    for p in v.split_whitespace() {
        if count == 4 {
            return Err("expected one value, or four (top right bottom left)");
        }
        n[count] = int(p, 0, max)?;
        count += 1;
    }
    match count {
        1 => Ok(Sides::all(n[0])),
        4 => Ok(Sides::new(n[0], n[1], n[2], n[3])),
        _ => Err("expected one value, or four (top right bottom left)"),
    }
}

/// `0xAARRGGBB`, `rgb(RRGGBB)` or `rgba(RRGGBBAA)`.
fn color(v: &str) -> Result<u32, &'static str> {
    const MSG: &str = "expected a colour: 0xAARRGGBB, rgb(RRGGBB) or rgba(RRGGBBAA)";
    let hex = |s: &str, len: usize| (s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u32::from_str_radix(s, 16).ok()).flatten();
    if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        return hex(h, 8).ok_or(MSG);
    }
    if let Some(h) = v.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let x = hex(h, 8).ok_or(MSG)?;
        return Ok(x.rotate_right(8)); // RRGGBBAA → AARRGGBB
    }
    if let Some(h) = v.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        return Ok(0xFF00_0000 | hex(h, 6).ok_or(MSG)?);
    }
    Err(MSG)
}

/// `0.55` → 5500 basis points, within 0.05..=0.95.
fn ratio(v: &str) -> Result<Ratio, &'static str> {
    let bp = decimal_bp(v).ok_or("expected a number like 0.55")?;
    if !(Ratio::MIN as i32..=(Ratio::ONE - Ratio::MIN) as i32).contains(&bp) {
        return Err("must be between 0.05 and 0.95");
    }
    Ok(Ratio(bp as u16))
}

/// A decimal number with up to four places, in basis points (`0.5` → 5000, `1` → 10000).
fn decimal_bp(v: &str) -> Option<i32> {
    let (int_part, frac) = v.split_once('.').unwrap_or((v, ""));
    if frac.len() > 4 || (int_part.is_empty() && frac.is_empty()) || !int_part.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let i: i32 = if int_part.is_empty() { 0 } else { int_part.parse().ok().filter(|i| *i <= 100)? };
    let mut f: i32 = if frac.is_empty() { 0 } else { frac.parse().ok()? };
    for _ in frac.len()..4 {
        f *= 10;
    }
    Some(i * 10000 + f)
}

/// `+0.05` / `-0.05` → ±500 basis points.
pub(crate) fn signed_ratio(v: &str) -> Option<i32> {
    if let Some(r) = v.strip_prefix('-') {
        decimal_bp(r).map(|x| -x)
    } else {
        decimal_bp(v.strip_prefix('+').unwrap_or(v))
    }
}

pub(crate) fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

fn rule(v: &str) -> Result<WindowRule, String> {
    let mut parts = v.split(',').map(str::trim);
    let act = parts.next().unwrap_or("");
    let mut words = act.split_whitespace();
    let action = match (words.next(), words.next(), words.next(), words.next()) {
        (Some("float"), None, ..) => RuleAction::Float,
        (Some("tile"), None, ..) => RuleAction::Tile,
        (Some("fullscreen"), None, ..) => RuleAction::Fullscreen,
        (Some("center"), None, ..) => RuleAction::Center,
        (Some("pin"), None, ..) => RuleAction::Pin,
        (Some("workspace"), Some(n), s, None) if matches!(s, None | Some("silent")) => match n.parse::<u8>() {
            Ok(n @ 1..=10) => RuleAction::Workspace { n, silent: s.is_some() },
            _ => return Err("windowrule workspace takes 1-10 (and `silent`)".into()),
        },
        (Some("size"), Some(_), Some(_), None) => match actions::two_ints(&act["size".len()..]) {
            Some((w, h)) if w > 0 && h > 0 => RuleAction::Size(w, h),
            _ => return Err("windowrule size takes a width and a height".into()),
        },
        _ => return Err("unknown windowrule action (float, tile, fullscreen, center, pin, workspace N [silent], size W H)".into()),
    };
    let mut matchers = Vec::new();
    for p in parts {
        matchers.push(if let Some(g) = p.strip_prefix("title:") {
            Matcher::Title(g.into())
        } else if let Some(g) = p.strip_prefix("program:") {
            Matcher::Program(g.into())
        } else {
            return Err(format!("unknown match `{p}` (title:GLOB or program:GLOB)"));
        });
    }
    if matchers.is_empty() {
        return Err("a windowrule needs at least one match: title:GLOB or program:GLOB".into());
    }
    Ok(WindowRule { action, matchers })
}

fn workspace(v: &str) -> Result<WorkspaceRule, String> {
    let mut parts = v.split(',').map(str::trim);
    let n = match parts.next().unwrap_or("").parse::<u8>() {
        Ok(n @ 1..=10) => n,
        _ => return Err("workspace takes a number 1-10 first".into()),
    };
    let mut w = WorkspaceRule { n, ..Default::default() };
    for p in parts {
        let Some((k, val)) = p.split_once(':') else { return Err(format!("expected key:value, found `{p}`")) };
        let val = val.trim();
        match k.trim() {
            "gaps_in" => w.gaps_in = Some(int(val, 0, 200).map_err(|e| format!("gaps_in: {e}"))?),
            "gaps_out" => w.gaps_out = Some(sides(val, 200).map_err(|e| format!("gaps_out: {e}"))?),
            "layout" => w.layout = Some(actions::layout_name(val)?.into()),
            other => return Err(format!("unknown workspace setting `{other}` (gaps_in, gaps_out, layout)")),
        }
    }
    Ok(w)
}

/// Glob matching: `*` any run of characters, `?` one character; everything else literally.
pub fn glob(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}
