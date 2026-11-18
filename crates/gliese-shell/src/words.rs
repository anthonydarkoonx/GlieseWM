//! Splitting a command line into words, and paths.
use crate::{GLIESE_CONF, HOME};
use alloc::string::String;
use alloc::vec::Vec;

/// Words: separated by spaces; `"…"` and `'…'` keep spaces (`$` is expanded in double quotes only); `\` keeps the next character;
/// `|`, `>` and `>>` are words of their own; `~` at the start of a word is `$HOME`. Variables: `$HOME`, `$PWD`, `$GLIESE_CONF`.
pub fn split(line: &str, cwd: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    let expand = |name: &str| -> Result<String, String> {
        match name {
            "HOME" => Ok(HOME.into()),
            "PWD" => Ok(cwd.into()),
            "GLIESE_CONF" => Ok(GLIESE_CONF.into()),
            _ => Err(alloc::format!("${name} is not set (known: $HOME, $PWD, $GLIESE_CONF)")),
        }
    };
    let var = |chars: &mut core::iter::Peekable<core::str::Chars<'_>>| -> String {
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        name
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(core::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '|' | '>' => {
                if in_word {
                    words.push(core::mem::take(&mut cur));
                    in_word = false;
                }
                if c == '>' && chars.peek() == Some(&'>') {
                    chars.next();
                    words.push(">>".into());
                } else {
                    words.push(String::from(c));
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("a ' is not closed".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => cur.extend(chars.next()),
                        Some('$') => cur.push_str(&expand(&var(&mut chars))?),
                        Some(x) => cur.push(x),
                        None => return Err("a \" is not closed".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cur.extend(chars.next());
            }
            '$' => {
                in_word = true;
                let name = var(&mut chars);
                if name.is_empty() {
                    cur.push('$');
                } else {
                    cur.push_str(&expand(&name)?);
                }
            }
            '~' if !in_word && matches!(chars.peek(), None | Some('/') | Some(' ')) => {
                in_word = true;
                cur.push_str(HOME);
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// `p` relative to `cwd`, with `.` and `..` worked out (never above `/`).
pub fn resolve(cwd: &str, p: &str) -> String {
    let full = if p.starts_with('/') { String::from(p) } else { alloc::format!("{cwd}/{p}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in full.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            x => parts.push(x),
        }
    }
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn words() {
        assert_eq!(split("ls  -l /data", "/").unwrap(), vec!["ls", "-l", "/data"]);
        assert_eq!(split("echo \"a b\" 'c $HOME' $HOME", "/").unwrap(), vec!["echo", "a b", "c $HOME", "/data"]);
        assert_eq!(split("cat x|grep y>>out", "/").unwrap(), vec!["cat", "x", "|", "grep", "y", ">>", "out"]);
        assert_eq!(split("nano $GLIESE_CONF", "/").unwrap(), vec!["nano", GLIESE_CONF]);
        assert_eq!(split("cd ~/notes", "/").unwrap(), vec!["cd", "/data/notes"]);
        assert_eq!(split("echo a\\ b", "/").unwrap(), vec!["echo", "a b"]);
        assert!(split("echo \"open", "/").is_err());
        assert!(split("echo $NOPE", "/").is_err());
        assert_eq!(split("echo $ x", "/").unwrap(), vec!["echo", "$", "x"]);
    }

    #[test]
    fn paths() {
        assert_eq!(resolve("/data", "notes"), "/data/notes");
        assert_eq!(resolve("/data/a", "../b/./c"), "/data/b/c");
        assert_eq!(resolve("/", "../../x"), "/x");
        assert_eq!(resolve("/data", "/system"), "/system");
        assert_eq!(resolve("/data", ".."), "/");
    }
}
