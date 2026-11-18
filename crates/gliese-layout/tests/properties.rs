//! Property tests for the gap arithmetic and every layout (`PLAN.md` §4.2), with a small deterministic generator (no crates).
use gliese_layout::*;

/// xorshift64*: the same cases on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

/// What every arrangement must satisfy, squeezed or not.
fn always(area: Rect, a: &Arranged, n: usize, what: &str) {
    assert_eq!(a.tiles.len(), n, "{what}: one tile a window");
    let mut ids: Vec<_> = a.tiles.iter().map(|t| t.0).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "{what}: no window twice");
    for (i, (_, r)) in a.tiles.iter().enumerate() {
        assert!(area.contains_rect(r), "{what}: tile {r:?} outside {area:?}");
        for (_, o) in &a.tiles[i + 1..] {
            assert!(!r.overlaps(o), "{what}: {r:?} overlaps {o:?}");
        }
    }
}

/// What an arrangement with room for everything must satisfy: tiles of at least a pixel, no tile closer than `gap` to another, and
/// nothing lost: every pixel of the area is in a tile or in a channel exactly `gap` wide between two tiles (or where two channels
/// cross).
fn exact(area: Rect, a: &Arranged, gap: i32, what: &str) {
    for (_, r) in &a.tiles {
        assert!(!r.is_empty(), "{what}: empty tile");
    }
    for (_, r) in &a.tiles {
        // and no tile closer than `gap` on any side
        for (_, o) in &a.tiles {
            if o == r {
                continue;
            }
            let near = Rect::new(r.x - gap + 1, r.y - gap + 1, r.w + 2 * gap - 2, r.h + 2 * gap - 2);
            if gap > 0 {
                assert!(!near.overlaps(o), "{what}: {o:?} closer than {gap} to {r:?}");
            }
        }
    }
    // nothing lost (rasterised; the areas in these tests are small enough): an uncovered pixel lies in a gap channel, a tile on both
    // sides within `gap` along x or along y
    let covered = |px: i32, py: i32| a.tiles.iter().any(|(_, r)| r.contains(px, py));
    for py in area.y..area.bottom() {
        for px in area.x..area.right() {
            if covered(px, py) {
                continue;
            }
            // the channel through it, along x or along y, is exactly `gap` wide: tiles on both sides, `gap` uncovered pixels between
            let width = |dx: i32, dy: i32| {
                let lo = (1..=gap).find(|&d| covered(px - d * dx, py - d * dy))?;
                let hi = (1..=gap).find(|&d| covered(px + d * dx, py + d * dy))?;
                Some(lo + hi - 1)
            };
            let along_x = width(1, 0) == Some(gap);
            let along_y = width(0, 1) == Some(gap);
            assert!(gap > 0 && (along_x || along_y || corner(&a.tiles, px, py, gap)), "{what}: pixel ({px},{py}) is in no tile and no gap");
        }
    }
}

/// A pixel where two gap channels cross (between four tiles, or at a T): tiles on two sides diagonally.
fn corner(tiles: &[(WinId, Rect)], px: i32, py: i32, gap: i32) -> bool {
    let near = Rect::new(px - gap, py - gap, 2 * gap + 1, 2 * gap + 1);
    tiles.iter().filter(|(_, r)| r.overlaps(&near)).count() >= 2
}

#[test]
fn split_is_exact() {
    for (len, gap) in [(100, 0), (100, 6), (101, 7), (7, 5), (3, 1)] {
        for ratio in [Ratio(Ratio::MIN), Ratio::HALF, Ratio(Ratio::ONE - Ratio::MIN), Ratio(3333)] {
            let r = Rect::new(10, 20, len, 50);
            let (a, b, sq) = split(r, ratio, gap, Axis::X);
            assert!(!sq);
            assert_eq!(a.x, 10);
            assert_eq!(b.x, a.right() + gap, "{len} {gap}");
            assert_eq!(b.right(), r.right());
            assert!(a.w >= 1 && b.w >= 1);
        }
    }
    // no room: squeezed, never overlapping, never outside
    let (a, b, sq) = split(Rect::new(0, 0, 5, 5), Ratio::HALF, 10, Axis::Y);
    assert!(sq && !a.overlaps(&b) && b.bottom() <= 5 && a.h >= 1);
    let (a, b, sq) = split(Rect::new(0, 0, 1, 5), Ratio::HALF, 0, Axis::X);
    assert!(sq && a.w == 1 && b.is_empty());
}

#[test]
fn stack_shares_the_remainder_with_the_first() {
    let mut v = Vec::new();
    assert!(!stack(Rect::new(0, 0, 100, 10), 3, 5, Axis::X, &mut v));
    // usable 90: 30 each
    assert_eq!(v, vec![Rect::new(0, 0, 30, 10), Rect::new(35, 0, 30, 10), Rect::new(70, 0, 30, 10)]);
    v.clear();
    stack(Rect::new(0, 0, 102, 10), 4, 2, Axis::X, &mut v);
    // usable 96: 24 each
    assert_eq!(v.iter().map(|r| r.w).collect::<Vec<_>>(), [24, 24, 24, 24]);
    v.clear();
    stack(Rect::new(0, 0, 10, 23), 3, 1, Axis::Y, &mut v);
    // usable 21: 7 each
    assert_eq!(v.iter().map(|r| (r.y, r.h)).collect::<Vec<_>>(), [(0, 7), (8, 7), (16, 7)]);
    v.clear();
    stack(Rect::new(0, 0, 10, 24), 3, 1, Axis::Y, &mut v);
    // usable 22: 8, 7, 7
    assert_eq!(v.iter().map(|r| (r.y, r.h)).collect::<Vec<_>>(), [(0, 8), (9, 7), (17, 7)]);
}

#[test]
fn outer_gaps_and_smart_gaps() {
    let s = LayoutSettings::default();
    let mut l = by_name("dwindle", &s).unwrap();
    let screen = Rect::new(0, 0, 1024, 768);
    let g = Gaps { inner: 6, outer: Sides::new(10, 20, 30, 40), smart: true };
    l.insert(1, None, screen, 6);
    assert_eq!(arrange(&*l, screen, &g).tiles, vec![(1, screen)], "one window with smart gaps: the whole area");
    l.insert(2, Some(1), screen, 6);
    let a = arrange(&*l, screen, &g);
    let inner = Rect::new(40, 10, 1024 - 60, 768 - 40);
    assert_eq!(a.tiles[0].1.x, inner.x);
    assert_eq!(a.tiles[1].1.right(), inner.right());
    assert_eq!(a.tiles[0].1.y, inner.y);
    assert_eq!(a.tiles[0].1.bottom(), inner.bottom());
    let g = Gaps { smart: false, ..g };
    l.remove(2);
    assert_eq!(arrange(&*l, screen, &g).tiles, vec![(1, inner)]);
}

#[test]
fn every_layout_keeps_its_promises() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut checked_exactly = 0;
    for case in 0..400 {
        let name = LAYOUTS[case % LAYOUTS.len()];
        let orientation = [Orientation::Left, Orientation::Right, Orientation::Top, Orientation::Bottom, Orientation::Center][rng.range(0, 4) as usize];
        let s = LayoutSettings {
            dwindle_split_ratio: Ratio(rng.range(Ratio::MIN as i32, (Ratio::ONE - Ratio::MIN) as i32) as u16),
            dwindle_force_split: rng.range(0, 2) as u8,
            dwindle_preserve_split: rng.range(0, 1) == 1,
            master_mfact: Ratio(rng.range(2000, 8000) as u16),
            master_nmaster: rng.range(1, 3) as u8,
            master_orientation: orientation,
            master_new_on_top: rng.range(0, 1) == 1,
        };
        let mut l = by_name(name, &s).unwrap();
        // small areas, so the rasterised check stays quick; odd sizes on purpose
        let area = Rect::new(rng.range(0, 30), rng.range(0, 30), rng.range(60, 241), rng.range(40, 181));
        let gap = rng.range(0, 12);
        let n = rng.range(1, 16) as usize;
        let mut focused = None;
        for id in 1..=n as u32 {
            l.insert(id, focused, area, gap);
            // the focus jumps around now and then, so dwindle trees are not all spirals
            focused = Some(if rng.range(0, 3) == 0 { rng.range(1, id as i32) as u32 } else { id });
        }
        // a few closes, resizes and swaps
        let mut live: Vec<u32> = (1..=n as u32).collect();
        for _ in 0..rng.range(0, 4) {
            match rng.range(0, 2) {
                0 if live.len() > 1 => {
                    let i = rng.range(0, live.len() as i32 - 1) as usize;
                    l.remove(live.remove(i));
                }
                1 => {
                    let w = live[rng.range(0, live.len() as i32 - 1) as usize];
                    l.resize(w, rng.range(-80, 80), rng.range(-80, 80), area, gap);
                }
                _ => {
                    let (a, b) = (live[rng.range(0, live.len() as i32 - 1) as usize], live[rng.range(0, live.len() as i32 - 1) as usize]);
                    l.swap(a, b);
                }
            }
        }
        let what = format!("case {case}: {name} {orientation:?} {area:?} gap {gap} n {}", live.len());
        let mut a = Arranged::default();
        l.arrange(area, gap, &mut a);
        always(area, &a, live.len(), &what);
        if !a.squeezed {
            exact(area, &a, gap, &what);
            checked_exactly += 1;
        }
        let mut again = Arranged::default();
        l.arrange(area, gap, &mut again);
        assert_eq!(a, again, "{what}: deterministic");
        let mut order = l.windows();
        order.sort();
        live.sort();
        assert_eq!(order, live, "{what}: the layout knows exactly the live windows");
    }
    // (most cases must have had room, or the exact checks prove little)
    assert!(checked_exactly >= 200, "only {checked_exactly} of 400 cases were checked exactly");
    println!("{checked_exactly} of 400 cases checked exactly");
}

#[test]
fn golden_dwindle_1024x768() {
    let s = LayoutSettings::default();
    let mut l = by_name("dwindle", &s).unwrap();
    let screen = Rect::new(0, 0, 1024, 768);
    let g = Gaps::default(); // inner 6, outer 12
    let area = tile_area(screen, &g, 2); // 12,12 1000x744
    let mut ids = Vec::new();
    for (n, want) in [
        (1, vec![Rect::new(12, 12, 1000, 744)]),
        (2, vec![Rect::new(12, 12, 497, 744), Rect::new(515, 12, 497, 744)]),
        (3, vec![Rect::new(12, 12, 497, 744), Rect::new(515, 12, 497, 369), Rect::new(515, 387, 497, 369)]),
    ] {
        let id = n as u32;
        l.insert(id, ids.last().copied(), area, g.inner);
        ids.push(id);
        let a = arrange(&*l, screen, &g);
        assert!(!a.squeezed);
        assert_eq!(a.tiles.iter().map(|t| t.1).collect::<Vec<_>>(), want, "{n} windows");
    }
    // five: the spiral goes on (each new window splits the newest tile)
    for id in [4, 5] {
        l.insert(id, Some(id - 1), area, g.inner);
    }
    let a = arrange(&*l, screen, &g);
    assert_eq!(
        a.tiles.iter().map(|t| t.1).collect::<Vec<_>>(),
        vec![
            Rect::new(12, 12, 497, 744),
            Rect::new(515, 12, 497, 369),
            Rect::new(515, 387, 246, 369),
            Rect::new(767, 387, 245, 182),
            Rect::new(767, 575, 245, 181),
        ]
    );
}

#[test]
fn golden_master_1024x768() {
    let s = LayoutSettings::default(); // mfact 0.55, left
    let mut l = by_name("master", &s).unwrap();
    let screen = Rect::new(0, 0, 1024, 768);
    let g = Gaps::default();
    for id in 1..=3 {
        l.insert(id, None, screen, g.inner);
    }
    let a = arrange(&*l, screen, &g);
    // usable width 994: master round(994 * 0.55) = 547
    assert_eq!(
        a.tiles.iter().map(|t| t.1).collect::<Vec<_>>(),
        vec![Rect::new(12, 12, 547, 744), Rect::new(565, 12, 447, 369), Rect::new(565, 387, 447, 369)]
    );
    assert!(l.message(LayoutMsg::SwapWithMaster, Some(3)));
    assert_eq!(l.windows(), vec![3, 2, 1]);
}

#[test]
fn dwindle_resize_and_messages() {
    let s = LayoutSettings::default();
    let mut l = by_name("dwindle", &s).unwrap();
    let area = Rect::new(0, 0, 1006, 500);
    for id in 1..=2 {
        l.insert(id, Some(1), area, 6);
    }
    let w = |l: &dyn Layout| {
        let mut a = Arranged::default();
        l.arrange(area, 6, &mut a);
        a.tiles.iter().map(|t| t.1.w).collect::<Vec<_>>()
    };
    assert_eq!(w(&*l), [500, 500]);
    l.resize(1, 100, 0, area, 6);
    assert_eq!(w(&*l), [600, 400], "the left window grows to the right");
    l.resize(2, 50, 0, area, 6);
    assert_eq!(w(&*l), [550, 450], "the right window grows to the left");
    l.resize(1, 5000, 0, area, 6);
    assert_eq!(w(&*l), [950, 50], "never below 5%");
    assert!(l.message(LayoutMsg::ToggleSplit, Some(1)));
    let mut a = Arranged::default();
    l.arrange(area, 6, &mut a);
    assert_eq!(a.tiles[0].1.w, 1006, "after togglesplit the two are one above the other");
    assert!(!l.message(LayoutMsg::AddMaster, Some(1)), "a master message means nothing to dwindle");
}
