//! The built-in commands that make text: each takes its arguments and, in a pipe, the text before it, and returns its output.
//! (`cd`, `clear`, `exit`, `history` and `nano` change the shell itself and are in `lib.rs`.)
use crate::{resolve, FsError, Sys};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Every built-in, with a line of help.
pub const BUILTINS: &[(&str, &str)] = &[
    ("help", "this list"),
    ("cd DIR", "change folder (no DIR: /data)"),
    ("pwd", "the current folder"),
    ("ls [DIR]", "what is in a folder (folders end in /)"),
    ("cat [FILE...]", "print files (or the text piped in)"),
    ("head [-n N] [FILE]", "the first N lines (10)"),
    ("tail [-n N] [FILE]", "the last N lines (10)"),
    ("wc [FILE]", "lines, words and bytes"),
    ("grep [-i] [-v] TEXT [FILE]", "lines that contain TEXT"),
    ("sort [-r] [FILE]", "lines in order"),
    ("echo TEXT", "print TEXT"),
    ("stat PATH", "size and kind"),
    ("mkdir DIR", "make a folder"),
    ("rm PATH", "delete a file or an empty folder"),
    ("cp FROM TO", "copy a file"),
    ("mv FROM TO", "move or rename"),
    ("nano FILE", "edit a file (^O write out, ^X exit); `nano $GLIESE_CONF` edits GlieseWM's config"),
    ("reload", "make GlieseWM read gliese.conf again"),
    ("prog list|start|stop|state ID", "installed programs (the program manager)"),
    ("fetch", "the machine at a glance (/system/bin/fetch is the HTTP one)"),
    ("uptime", "time since the machine started"),
    ("free", "memory"),
    ("history", "commands typed before"),
    ("clear", "clear the window (also Ctrl-L)"),
    ("exit", "close this window"),
];

const TEXT_BUILTINS: &[&str] = &["help", "pwd", "ls", "cat", "head", "tail", "wc", "grep", "sort", "echo", "stat", "mkdir", "rm", "cp", "mv", "reload", "prog", "uptime", "free"];

/// Built-in in the widest sense: the ones here and the ones the shell handles itself.
pub fn is_builtin(name: &str) -> bool {
    TEXT_BUILTINS.contains(&name) || matches!(name, "cd" | "clear" | "exit" | "history" | "nano" | "edit" | "fetch")
}

fn fs(e: FsError, path: &str) -> String {
    format!("{path}: {}", e.text())
}

/// The text of a file argument, or of the input when there is none.
fn text_of(sys: &mut dyn Sys, cwd: &str, file: Option<&str>, input: Option<&str>) -> Result<String, String> {
    match file {
        Some(f) => {
            let p = resolve(cwd, f);
            let b = sys.read(&p).map_err(|e| fs(e, &p))?;
            Ok(String::from_utf8_lossy(&b).into_owned())
        }
        None => Ok(input.unwrap_or("").into()),
    }
}

/// `-n N` (or `-N`) before the file; the count and what is left.
fn count_arg<'a>(args: &[&'a str]) -> Result<(usize, Vec<&'a str>), String> {
    let mut n = 10;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if *a == "-n" {
            n = it.next().and_then(|v| v.parse().ok()).ok_or("-n needs a number")?;
        } else if let Some(v) = a.strip_prefix('-').and_then(|v| v.parse().ok()) {
            n = v;
        } else {
            rest.push(*a);
        }
    }
    Ok((n, rest))
}

fn lines_out<'a>(it: impl Iterator<Item = &'a str>) -> String {
    let mut s = String::new();
    for l in it {
        s.push_str(l);
        s.push('\n');
    }
    s
}

pub fn run(sys: &mut dyn Sys, cwd: &str, name: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
    match name {
        "help" => {
            let w = BUILTINS.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
            let mut s = String::from("Built-in commands (any other name runs a program from /system/bin or /data/bin):\n");
            for (c, h) in BUILTINS {
                s.push_str(&format!("  {c:w$}  {h}\n"));
            }
            s.push_str("Pipes (|) and > / >> work between built-in commands. Ctrl-C stops a program, PageUp/PageDown scroll.\n");
            Ok(s)
        }
        "pwd" => Ok(format!("{cwd}\n")),
        "echo" => Ok(format!("{}\n", args.join(" "))),
        "ls" => {
            let p = resolve(cwd, args.first().copied().unwrap_or("."));
            let mut list = sys.list(&p).map_err(|e| fs(e, &p))?;
            list.sort_by(|a, b| a.name.cmp(&b.name));
            let mut s = String::new();
            for e in &list {
                s.push_str(&e.name);
                if e.dir {
                    s.push('/');
                }
                s.push('\n');
            }
            Ok(s)
        }
        "cat" => {
            if args.is_empty() {
                return Ok(input.unwrap_or("").into());
            }
            let mut s = String::new();
            for a in args {
                s.push_str(&text_of(sys, cwd, Some(a), None)?);
            }
            Ok(s)
        }
        "head" | "tail" => {
            let (n, rest) = count_arg(args)?;
            let text = text_of(sys, cwd, rest.first().copied(), input)?;
            let lines: Vec<&str> = text.lines().collect();
            Ok(if name == "head" { lines_out(lines.iter().take(n).copied()) } else { lines_out(lines[lines.len().saturating_sub(n)..].iter().copied()) })
        }
        "wc" => {
            let text = text_of(sys, cwd, args.first().copied(), input)?;
            Ok(format!("{} {} {}\n", text.lines().count(), text.split_whitespace().count(), text.len()))
        }
        "grep" => {
            let (mut ignore, mut invert, mut rest) = (false, false, Vec::new());
            for a in args {
                match *a {
                    "-i" => ignore = true,
                    "-v" => invert = true,
                    x => rest.push(x),
                }
            }
            let pat = rest.first().ok_or("grep needs the text to look for")?;
            let text = text_of(sys, cwd, rest.get(1).copied(), input)?;
            let (pat, low) = (pat.to_string(), pat.to_lowercase());
            Ok(lines_out(text.lines().filter(|l| (if ignore { l.to_lowercase().contains(&low) } else { l.contains(&pat) }) != invert)))
        }
        "sort" => {
            let rev = args.contains(&"-r");
            let file = args.iter().find(|a| **a != "-r").copied();
            let text = text_of(sys, cwd, file, input)?;
            let mut lines: Vec<&str> = text.lines().collect();
            lines.sort();
            if rev {
                lines.reverse();
            }
            Ok(lines_out(lines.into_iter()))
        }
        "stat" => {
            let p = resolve(cwd, args.first().ok_or("stat needs a path")?);
            let (size, dir) = sys.stat(&p).map_err(|e| fs(e, &p))?;
            Ok(if dir { format!("{p}: folder\n") } else { format!("{p}: file, {size} bytes\n") })
        }
        "mkdir" => {
            let p = resolve(cwd, args.first().ok_or("mkdir needs a name")?);
            sys.mkdir(&p).map_err(|e| fs(e, &p))?;
            Ok(String::new())
        }
        "rm" => {
            if args.is_empty() {
                return Err("rm needs a path".into());
            }
            for a in args {
                let p = resolve(cwd, a);
                sys.remove(&p).map_err(|e| fs(e, &p))?;
            }
            Ok(String::new())
        }
        "cp" | "mv" => {
            let [from, to] = args else { return Err(format!("{name} takes two paths: FROM TO")) };
            let (from, mut to) = (resolve(cwd, from), resolve(cwd, to));
            // into a folder: keep the name
            if matches!(sys.stat(&to), Ok((_, true))) {
                let base = from.rsplit('/').next().unwrap_or("");
                to = resolve(&to, base);
            }
            if name == "mv" {
                sys.rename(&from, &to).map_err(|e| fs(e, &from))?;
            } else {
                let data = sys.read(&from).map_err(|e| fs(e, &from))?;
                sys.write(&to, &data, false).map_err(|e| fs(e, &to))?;
            }
            Ok(String::new())
        }
        "reload" => {
            sys.reload()?;
            Ok("asked GlieseWM to read gliese.conf again\n".into())
        }
        "prog" => sys.prog(args),
        "uptime" => {
            let s = sys.uptime_ms() / 1000;
            Ok(format!("up {}:{:02}:{:02}\n", s / 3600, s / 60 % 60, s % 60))
        }
        "free" => match sys.memory() {
            Some((total, free)) => Ok(format!("memory: {} MiB, {} MiB free\n", total >> 20, free >> 20)),
            None => Err("not known".into()),
        },
        _ => Err("not a built-in command".into()),
    }
}
