//! gliese.conf: every construct, and every kind of error with its line and column.
use gliese_config::*;

fn ok(text: &str) -> Config {
    match parse("t.conf", text, None) {
        Ok(c) => c,
        Err(e) => panic!("{text:?}: {:?}", e.iter().map(|e| e.to_string()).collect::<Vec<_>>()),
    }
}

/// The errors' "line:col: message" for a text.
fn errs(text: &str) -> Vec<String> {
    parse("t.conf", text, None).expect_err(text).iter().map(|e| format!("{}:{}: {}", e.line, e.col, e.msg)).collect()
}

#[test]
fn the_default_parses_and_has_super_enter() {
    let c = default_config();
    let b = c.bind_for(None, mods::SUPER, Key::Code(28), false).expect("Super+Enter is bound");
    assert_eq!(b.action, Action::Exec("terminal".into()));
    assert_eq!(c.general.gaps_in, 6);
    assert_eq!(c.general.gaps_out, Sides::all(12));
    assert_eq!(c.general.layout, "dwindle");
    assert!(c.binds.iter().any(|b| b.submap.as_deref() == Some("resize") && b.flags.repeat));
    assert!(c.binds.iter().any(|b| b.flags.mouse && b.action == Action::MouseMove && b.key == Key::Mouse(1)));
    // the documented defaults and the file's say the same, apart from the binds
    let plain = Config { binds: c.binds.clone(), ..Config::default() };
    assert_eq!(plain, c, "Config::default() and gliese.conf disagree on a setting");
}

#[test]
fn sections_flat_keys_and_variables() {
    let c = ok("$g = 9\ngeneral {\n  gaps_in = $g\n  gaps_out = 1 2 3 4\n  col.active_border = rgb(ff0000)\n}\nmaster:mfact = 0.6\nmaster {\n orientation = center\n}\n");
    assert_eq!(c.general.gaps_in, 9);
    assert_eq!(c.general.gaps_out, Sides::new(1, 2, 3, 4));
    assert_eq!(c.general.active_border, 0xFFFF_0000);
    assert_eq!(c.layouts.master_mfact, Ratio(6000));
    assert_eq!(c.layouts.master_orientation, Orientation::Center);
    // rgba is RRGGBBAA
    assert_eq!(ok("general:col.inactive_border = rgba(11223380)").general.inactive_border, 0x8011_2233);
    // a later value wins
    assert_eq!(ok("general:gaps_in = 1\ngeneral:gaps_in = 2").general.gaps_in, 2);
}

#[test]
fn binds() {
    let c = ok("$mod = SUPER\nbind = $mod SHIFT, 2, movetoworkspacesilent, 2\nbinde = , Right, resizeactive, 20 0\nbindr = SUPER, SUPER, exec, terminal\nbind = CTRL ALT, code:183, layoutmsg, mfact -0.05\nbind = SUPER, mouse:middle, killactive\n");
    assert_eq!(c.binds.len(), 5);
    assert_eq!(c.binds[0].mods, mods::SUPER | mods::SHIFT);
    assert_eq!(c.binds[0].key, Key::Code(3));
    assert_eq!(c.binds[0].action, Action::MoveToWorkspace { ws: Ws::Number(2), silent: true });
    assert!(c.binds[1].flags.repeat && c.binds[1].mods == 0);
    assert_eq!(c.binds[1].action, Action::ResizeActive(20, 0));
    assert!(c.binds[2].flags.release && c.binds[2].key == Key::Code(125));
    assert_eq!(c.binds[3].action, Action::LayoutMsg(gliese_layout::LayoutMsg::MasterFactor(-500)));
    assert_eq!(c.binds[4].key, Key::Mouse(3));
    assert_eq!(ok("bind = SUPER, equal, setgaps, +2 8").binds[0].action, Action::SetGaps { inner: Delta::Add(2), outer: Delta::Set(8) });
    assert_eq!(ok("bind = SUPER, W, workspace, empty").binds[0].action, Action::Workspace(Ws::Empty));
    // the same key in another submap is another bind
    ok("bind = , Escape, killactive\nbind = SUPER, R, submap, x\nsubmap = x\nbind = , Escape, submap, reset\nsubmap = reset");
}

#[test]
fn rules_workspaces_exec_once() {
    let c = ok("windowrule = float, title:*Properties*\nwindowrule = workspace 3 silent, program:files, title:Copy*\nwindowrule = size 640 480, program:calc\nworkspace = 9, gaps_in:0, gaps_out:0, layout:master\nexec-once = terminal\n");
    assert_eq!(c.rules[0].action, RuleAction::Float);
    assert!(c.rules[0].matches("File Properties", "files"));
    assert!(!c.rules[0].matches("Files", "files"));
    assert_eq!(c.rules[1].action, RuleAction::Workspace { n: 3, silent: true });
    assert!(c.rules[1].matches("Copying 3 files", "files") && !c.rules[1].matches("Copying", "terminal"));
    assert_eq!(c.rules[2].action, RuleAction::Size(640, 480));
    let w = c.workspace(9).unwrap();
    assert_eq!((w.gaps_in, w.gaps_out, w.layout.as_deref()), (Some(0), Some(Sides::all(0)), Some("master")));
    assert_eq!(c.exec_once, vec!["terminal".to_string()]);
}

#[test]
fn errors_say_where() {
    assert_eq!(errs("general {\n  gap_in = 5\n}"), ["2:3: unknown key `general:gap_in`"]);
    assert_eq!(errs("general:gaps_in = lots"), ["1:19: general:gaps_in: expected a whole number"]);
    assert_eq!(errs("general:gaps_in = 999"), ["1:19: general:gaps_in: number out of range"]);
    assert_eq!(errs("dwindle:split_ratio = 0.99"), ["1:23: dwindle:split_ratio: must be between 0.05 and 0.95"]);
    assert_eq!(errs("general:layout = spiral"), ["1:18: general:layout: unknown layout (dwindle or master)"]);
    assert_eq!(errs("general:col.active_border = red"), ["1:29: general:col.active_border: expected a colour: 0xAARRGGBB, rgb(RRGGBB) or rgba(RRGGBBAA)"]);
    assert_eq!(errs("gaps_in = 5"), ["1:1: unknown keyword `gaps_in` (settings go in a section: `general { gaps_in = … }`)"]);
    assert_eq!(errs("just words"), ["1:1: expected `key = value`"]);
    assert_eq!(errs("}"), ["1:1: `}` without an open section"]);
    assert_eq!(errs("general {\n gaps_in = 1"), ["2:1: section `general` is not closed with `}`"]);
    assert_eq!(errs("bind = $mod, Q, killactive"), ["1:8: `$mod` is not defined (define it above: `$mod = …`)"]);
    assert_eq!(errs("bind = HYPER, Q, killactive"), ["1:8: unknown modifier `HYPER` (SUPER, SHIFT, CTRL, ALT)"]);
    assert_eq!(errs("bind = SUPER, Qq, killactive"), ["1:8: unknown key `Qq`"]);
    assert_eq!(errs("bind = SUPER, Q, kill"), ["1:8: unknown dispatcher `kill`"]);
    assert_eq!(errs("bind = SUPER, Q, killactive, now"), ["1:8: this dispatcher takes no argument"]);
    assert_eq!(errs("bind = SUPER, 1, workspace, 11"), ["1:8: workspace must be 1-10, +1, -1, previous or empty"]);
    assert_eq!(errs("bind = SUPER, Return, exec, nano /data/x"), ["1:8: exec takes a program id; programs cannot be started with arguments yet"]);
    assert_eq!(errs("bind = CTRL ALT, Delete, killactive"), ["1:8: Ctrl+Alt+Delete and Ctrl+Shift+Escape are the system's and cannot be bound"]);
    assert_eq!(errs("bind = CTRL SHIFT, Escape, reload"), ["1:8: Ctrl+Alt+Delete and Ctrl+Shift+Escape are the system's and cannot be bound"]);
    assert_eq!(errs("bind = SUPER, Q, killactive\nbind = SUPER, q, reload"), ["2:8: this key is already bound on line 1"]);
    assert_eq!(errs("bindx = SUPER, Q, killactive"), ["1:1: unknown keyword `bindx` (bind, binde, bindr or bindm)"]);
    assert_eq!(errs("bindm = SUPER, mouse:left, killactive"), ["1:9: bindm takes movewindow or resizewindow (with no argument)"]);
    assert_eq!(errs("bindm = SUPER, A, movewindow"), ["1:9: bindm needs a mouse button (mouse:left, mouse:right, …)"]);
    assert_eq!(errs("bind = SUPER, A, movewindow"), ["1:8: movewindow without a direction, and resizewindow, are for bindm"]);
    assert_eq!(errs("bind = SUPER, R, submap, resize"), ["1:1: submap `resize` is not defined (`submap = resize` … `submap = reset`)"]);
    assert_eq!(errs("submap = trap\nbind = , A, killactive\nsubmap = reset"), ["1:1: submap `trap` has no bind back out (`bind = , Escape, submap, reset`): you would be stuck in it"]);
    assert_eq!(errs("windowrule = float"), ["1:14: a windowrule needs at least one match: title:GLOB or program:GLOB"]);
    assert_eq!(errs("windowrule = explode, title:x"), ["1:14: unknown windowrule action (float, tile, fullscreen, center, pin, workspace N [silent], size W H)"]);
    assert_eq!(errs("workspace = 0, gaps_in:0"), ["1:13: workspace takes a number 1-10 first"]);
    assert_eq!(errs("source = x.conf"), ["1:10: `source` is not available here"]);
}

#[test]
fn every_error_is_reported_not_just_the_first() {
    let e = errs("general:gaps_in = x\ngeneral:border_size = 99\nbind = SUPER, Q, nope");
    assert_eq!(e.len(), 3, "{e:?}");
    assert!(parse("t.conf", &"x\n".repeat(100), None).unwrap_err().len() == MAX_ERRORS);
}

#[test]
fn source_includes_and_reports_in_the_included_file() {
    let mut files = |p: &str| match p {
        "binds.conf" => Some("bind = SUPER, Q, killactive\n".to_string()),
        "bad.conf" => Some("\ngeneral:gaps_in = -1\n".to_string()),
        "loop.conf" => Some("source = loop.conf\n".to_string()),
        _ => None,
    };
    let c = parse("main.conf", "source = binds.conf\n", Some(&mut files)).unwrap();
    assert_eq!(c.binds.len(), 1);
    let e = parse("main.conf", "source = bad.conf\n", Some(&mut files)).unwrap_err();
    assert_eq!(e[0].to_string(), "bad.conf:2:19: general:gaps_in: number out of range");
    let e = parse("main.conf", "source = loop.conf\n", Some(&mut files)).unwrap_err();
    assert!(e[0].msg.contains("nests too deep"), "{:?}", e);
    let e = parse("main.conf", "source = none.conf\n", Some(&mut files)).unwrap_err();
    assert_eq!(e[0].to_string(), "main.conf:1:10: cannot read `none.conf`");
}

#[test]
fn globs() {
    assert!(glob("*", ""));
    assert!(glob("a*c", "abbbc"));
    assert!(glob("a?c", "abc") && !glob("a?c", "ac"));
    assert!(glob("*Properties*", "File Properties - x"));
    assert!(!glob("abc", "abcd") && !glob("abc*d", "abc"));
    assert!(glob("**a", "xxa"));
}

/// Random lines never panic the parser.
#[test]
fn garbage_never_panics() {
    let mut x = 0x1234_5678_9abc_def0u64;
    let pieces = ["bind", "=", ",", "$", "{", "}", "general", ":", "gaps_in", " ", "SUPER", "mouse:", "code:", "submap", "0x", "rgb(", "#", "\n", "é", "workspace", "windowrule", "title:", "9", "-", "+", ".", "source"];
    for _ in 0..3000 {
        let mut s = String::new();
        for _ in 0..(x % 40) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            s.push_str(pieces[(x % pieces.len() as u64) as usize]);
        }
        let _ = parse("fuzz", &s, None);
    }
}
