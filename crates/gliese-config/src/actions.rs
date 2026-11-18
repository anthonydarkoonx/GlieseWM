//! Dispatchers: what a bind does (`bind = MODS, KEY, dispatcher, arg`). Each is one entry in [`DISPATCHERS`]: a name and a parser
//! for its argument. `gliese-core` carries them out.
use alloc::string::String;
use gliese_layout::{LayoutMsg, Orientation};

/// A direction for `movefocus` and `movewindow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Which workspace (`workspace`, `movetoworkspace`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ws {
    /// 1..=10.
    Number(u8),
    /// `+1`: the next one (10 wraps to 1).
    Next,
    /// `-1`.
    Prev,
    /// `previous`: the one shown before this.
    Previous,
    /// `empty`: the first one with no windows.
    Empty,
}

/// A new value, or a change to the current one (`setgaps +2 -4`, `setgaps 8 16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta {
    Set(i32),
    Add(i32),
}

impl Delta {
    pub fn apply(self, v: i32) -> i32 {
        match self {
            Delta::Set(n) => n,
            Delta::Add(d) => v + d,
        }
        .clamp(0, 200)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start a program (an installed program's id) or, if it runs, ask it for a new window.
    Exec(String),
    KillActive,
    ToggleFloating,
    Fullscreen,
    Pin,
    CenterWindow,
    MoveFocus(Dir),
    MoveWindow(Dir),
    CycleNext { prev: bool },
    FocusUrgent,
    ResizeActive(i32, i32),
    Workspace(Ws),
    MoveToWorkspace { ws: Ws, silent: bool },
    LayoutMsg(LayoutMsg),
    Layout(String),
    SetGaps { inner: Delta, outer: Delta },
    Submap(String),
    Reload,
    Exit,
    /// `bindm` only: drag the window under the pointer.
    MouseMove,
    /// `bindm` only: resize it.
    MouseResize,
}

type Parser = fn(&str) -> Result<Action, &'static str>;

/// Every dispatcher: name, argument parser.
pub const DISPATCHERS: &[(&str, Parser)] = &[
    ("exec", |a| program_id(a).map(|p| Action::Exec(p.into()))),
    ("killactive", |a| none(a, Action::KillActive)),
    ("togglefloating", |a| none(a, Action::ToggleFloating)),
    ("fullscreen", |a| none(a, Action::Fullscreen)),
    ("pin", |a| none(a, Action::Pin)),
    ("centerwindow", |a| none(a, Action::CenterWindow)),
    ("movefocus", |a| dir(a).map(Action::MoveFocus)),
    ("movewindow", |a| if a.is_empty() { Ok(Action::MouseMove) } else { dir(a).map(Action::MoveWindow) }),
    ("resizewindow", |a| none(a, Action::MouseResize)),
    ("cyclenext", |a| match a {
        "" => Ok(Action::CycleNext { prev: false }),
        "prev" => Ok(Action::CycleNext { prev: true }),
        _ => Err("cyclenext takes nothing or `prev`"),
    }),
    ("focusurgent", |a| none(a, Action::FocusUrgent)),
    ("resizeactive", |a| two_ints(a).map(|(x, y)| Action::ResizeActive(x, y)).ok_or("resizeactive takes two numbers: dx dy")),
    ("workspace", |a| ws(a).map(Action::Workspace)),
    ("movetoworkspace", |a| ws(a).map(|ws| Action::MoveToWorkspace { ws, silent: false })),
    ("movetoworkspacesilent", |a| ws(a).map(|ws| Action::MoveToWorkspace { ws, silent: true })),
    ("layoutmsg", |a| layout_msg(a).map(Action::LayoutMsg)),
    ("togglesplit", |a| none(a, Action::LayoutMsg(LayoutMsg::ToggleSplit))),
    ("swapsplit", |a| none(a, Action::LayoutMsg(LayoutMsg::SwapSplit))),
    ("layout", |a| layout_name(a).map(|n| Action::Layout(n.into()))),
    ("setgaps", |a| {
        let mut it = a.split_whitespace();
        match (it.next().and_then(delta), it.next().and_then(delta), it.next()) {
            (Some(inner), Some(outer), None) => Ok(Action::SetGaps { inner, outer }),
            _ => Err("setgaps takes two values: inner outer (each `8`, `+2` or `-2`)"),
        }
    }),
    ("submap", |a| if crate::ident_ok(a) { Ok(Action::Submap(a.into())) } else { Err("submap takes a name (letters, digits, - _)") }),
    ("reload", |a| none(a, Action::Reload)),
    ("exit", |a| none(a, Action::Exit)),
];

/// Parse `dispatcher` with its argument.
pub fn parse(name: &str, arg: &str) -> Result<Action, &'static str> {
    let (_, p) = DISPATCHERS.iter().find(|(n, _)| *n == name).ok_or("unknown dispatcher")?;
    p(arg.trim())
}

fn none(a: &str, act: Action) -> Result<Action, &'static str> {
    if a.is_empty() {
        Ok(act)
    } else {
        Err("this dispatcher takes no argument")
    }
}

fn program_id(a: &str) -> Result<&str, &'static str> {
    // the program manager's ids: 1..=16 of a-z 0-9 _ - .
    if !a.is_empty() && a.len() <= 16 && a.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')) {
        Ok(a)
    } else if a.contains(' ') {
        Err("exec takes a program id; programs cannot be started with arguments yet")
    } else {
        Err("exec takes an installed program's id (1-16 of a-z 0-9 _ - .)")
    }
}

fn dir(a: &str) -> Result<Dir, &'static str> {
    match a {
        "l" | "left" => Ok(Dir::Left),
        "r" | "right" => Ok(Dir::Right),
        "u" | "up" => Ok(Dir::Up),
        "d" | "down" => Ok(Dir::Down),
        _ => Err("direction must be l, r, u or d"),
    }
}

fn ws(a: &str) -> Result<Ws, &'static str> {
    match a {
        "+1" | "next" => Ok(Ws::Next),
        "-1" | "prev" => Ok(Ws::Prev),
        "previous" => Ok(Ws::Previous),
        "empty" => Ok(Ws::Empty),
        n => match n.parse::<u8>() {
            Ok(n @ 1..=10) => Ok(Ws::Number(n)),
            _ => Err("workspace must be 1-10, +1, -1, previous or empty"),
        },
    }
}

pub(crate) fn two_ints(a: &str) -> Option<(i32, i32)> {
    let mut it = a.split_whitespace();
    let (x, y) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    it.next().is_none().then_some((x, y))
}

fn delta(s: &str) -> Option<Delta> {
    if let Some(d) = s.strip_prefix('+') {
        d.parse().ok().map(Delta::Add)
    } else if s.starts_with('-') {
        s.parse().ok().map(Delta::Add)
    } else {
        s.parse().ok().filter(|v: &i32| *v >= 0).map(Delta::Set)
    }
}

pub(crate) fn layout_name(a: &str) -> Result<&str, &'static str> {
    if gliese_layout::LAYOUTS.contains(&a) {
        Ok(a)
    } else {
        Err("unknown layout (dwindle or master)")
    }
}

pub(crate) fn orientation(a: &str) -> Option<Orientation> {
    Some(match a {
        "left" => Orientation::Left,
        "right" => Orientation::Right,
        "top" => Orientation::Top,
        "bottom" => Orientation::Bottom,
        "center" => Orientation::Center,
        _ => return None,
    })
}

fn layout_msg(a: &str) -> Result<LayoutMsg, &'static str> {
    let mut it = a.split_whitespace();
    let what = it.next().unwrap_or("");
    let arg = it.next();
    if it.next().is_some() {
        return Err("layoutmsg takes a message and at most one value");
    }
    match (what, arg) {
        ("togglesplit", None) => Ok(LayoutMsg::ToggleSplit),
        ("swapsplit", None) => Ok(LayoutMsg::SwapSplit),
        ("swapwithmaster", None) => Ok(LayoutMsg::SwapWithMaster),
        ("addmaster", None) => Ok(LayoutMsg::AddMaster),
        ("removemaster", None) => Ok(LayoutMsg::RemoveMaster),
        ("mfact", Some(v)) => crate::signed_ratio(v).map(LayoutMsg::MasterFactor).ok_or("mfact takes a change like +0.05 or -0.05"),
        ("orientation", Some(o)) => orientation(o).map(LayoutMsg::Orientation).ok_or("orientation must be left, right, top, bottom or center"),
        _ => Err("unknown layout message (togglesplit, swapsplit, swapwithmaster, addmaster, removemaster, mfact ±f, orientation o)"),
    }
}
