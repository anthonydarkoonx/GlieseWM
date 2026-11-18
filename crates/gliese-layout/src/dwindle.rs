//! The dwindle layout: a binary tree. A new window splits the tile of the focused window in two; closing a window gives its whole
//! tile to its sibling.
use crate::{split, Arranged, Axis, Layout, LayoutMsg, LayoutSettings, Ratio, Rect, WinId};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Leaf(WinId),
    Split {
        a: usize,
        b: usize,
        ratio: Ratio,
        /// Fixed direction (`preserve_split`, `togglesplit`), or `None` to follow the tile's shape.
        axis: Option<Axis>,
    },
}

/// The tree lives in an arena: `nodes[i]`, with `None` for a free entry; `root` is the top.
#[derive(Clone, Debug)]
pub struct Dwindle {
    nodes: Vec<Option<Node>>,
    parent: Vec<Option<usize>>,
    root: Option<usize>,
    ratio: Ratio,
    force_split: u8,
    preserve_split: bool,
}

impl Dwindle {
    pub fn new(s: &LayoutSettings) -> Dwindle {
        Dwindle {
            nodes: Vec::new(),
            parent: Vec::new(),
            root: None,
            ratio: s.dwindle_split_ratio,
            force_split: s.dwindle_force_split,
            preserve_split: s.dwindle_preserve_split,
        }
    }

    fn alloc(&mut self, n: Node, parent: Option<usize>) -> usize {
        match self.nodes.iter().position(|x| x.is_none()) {
            Some(i) => {
                self.nodes[i] = Some(n);
                self.parent[i] = parent;
                i
            }
            None => {
                self.nodes.push(Some(n));
                self.parent.push(parent);
                self.nodes.len() - 1
            }
        }
    }

    fn node(&self, i: usize) -> &Node {
        self.nodes[i].as_ref().expect("a live node")
    }

    fn leaf_of(&self, id: WinId) -> Option<usize> {
        self.nodes.iter().position(|n| matches!(n, Some(Node::Leaf(x)) if *x == id))
    }

    /// The leaves in order (first child before second).
    fn leaves(&self, i: usize, out: &mut Vec<WinId>) {
        match self.node(i) {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                let (a, b) = (*a, *b);
                self.leaves(a, out);
                self.leaves(b, out);
            }
        }
    }

    /// The rectangle node `target` gets when the tree is arranged in `area` (and the path's splits, for resizing).
    fn rect_of(&self, target: usize, area: Rect, gap: i32) -> Option<Rect> {
        let mut path = Vec::new();
        let mut at = target;
        while let Some(p) = self.parent[at] {
            path.push((p, at));
            at = p;
        }
        if Some(at) != self.root {
            return None;
        }
        let mut r = area;
        for &(p, child) in path.iter().rev() {
            let Node::Split { a, ratio, axis, .. } = *self.node(p) else { unreachable!() };
            let ax = axis.unwrap_or(Axis::longer_side(&r));
            let (ra, rb, _) = split(r, ratio, gap, ax);
            r = if child == a { ra } else { rb };
        }
        Some(r)
    }

    fn arrange_node(&self, i: usize, r: Rect, gap: i32, out: &mut Arranged) {
        match *self.node(i) {
            Node::Leaf(id) => out.tiles.push((id, r)),
            Node::Split { a, b, ratio, axis } => {
                let ax = axis.unwrap_or(Axis::longer_side(&r));
                let (ra, rb, sq) = split(r, ratio, gap, ax);
                out.squeezed |= sq;
                self.arrange_node(a, ra, gap, out);
                self.arrange_node(b, rb, gap, out);
            }
        }
    }

    /// The split directly above a leaf.
    fn parent_split(&self, id: WinId) -> Option<usize> {
        self.leaf_of(id).and_then(|l| self.parent[l])
    }
}

impl Layout for Dwindle {
    fn name(&self) -> &'static str {
        "dwindle"
    }

    fn insert(&mut self, id: WinId, focused: Option<WinId>, area: Rect, gap: i32) {
        if self.contains(id) {
            return;
        }
        let Some(root) = self.root else {
            self.root = Some(self.alloc(Node::Leaf(id), None));
            return;
        };
        // the tile to split: the focused window's, else the last window's
        let target = focused.and_then(|f| self.leaf_of(f)).unwrap_or_else(|| {
            let mut all = Vec::new();
            self.leaves(root, &mut all);
            self.leaf_of(*all.last().expect("a tree has a leaf")).expect("its leaf")
        });
        let tile = self.rect_of(target, area, gap).unwrap_or(area);
        let axis = self.preserve_split.then(|| Axis::longer_side(&tile));
        let up = self.parent[target];
        let old = self.node(target).clone();
        // the target node becomes the split; its window and the new one become its children
        let old_leaf = self.alloc(old, Some(target));
        let new_leaf = self.alloc(Node::Leaf(id), Some(target));
        let (a, b) = if self.force_split == 1 { (new_leaf, old_leaf) } else { (old_leaf, new_leaf) };
        self.nodes[target] = Some(Node::Split { a, b, ratio: self.ratio, axis });
        self.parent[target] = up;
    }

    fn remove(&mut self, id: WinId) {
        let Some(leaf) = self.leaf_of(id) else { return };
        self.nodes[leaf] = None;
        let Some(p) = self.parent[leaf].take() else {
            self.root = None;
            return;
        };
        // the sibling takes the parent's place
        let Node::Split { a, b, .. } = *self.node(p) else { unreachable!() };
        let sib = if a == leaf { b } else { a };
        let moved = self.nodes[sib].take();
        self.nodes[p] = moved;
        if let Some(Node::Split { a, b, .. }) = self.nodes[p] {
            self.parent[a] = Some(p);
            self.parent[b] = Some(p);
        }
        self.parent[sib] = None;
    }

    fn contains(&self, id: WinId) -> bool {
        self.leaf_of(id).is_some()
    }

    fn windows(&self) -> Vec<WinId> {
        let mut v = Vec::new();
        if let Some(r) = self.root {
            self.leaves(r, &mut v);
        }
        v
    }

    fn arrange(&self, area: Rect, gap: i32, out: &mut Arranged) {
        if let Some(r) = self.root {
            self.arrange_node(r, area, gap, out);
        }
    }

    fn swap(&mut self, a: WinId, b: WinId) -> bool {
        let (Some(la), Some(lb)) = (self.leaf_of(a), self.leaf_of(b)) else { return false };
        self.nodes[la] = Some(Node::Leaf(b));
        self.nodes[lb] = Some(Node::Leaf(a));
        true
    }

    fn resize(&mut self, id: WinId, dx: i32, dy: i32, area: Rect, gap: i32) {
        let Some(leaf) = self.leaf_of(id) else { return };
        // for each axis, the nearest split above the window that cuts along it
        for (delta, want) in [(dx, Axis::X), (dy, Axis::Y)] {
            if delta == 0 {
                continue;
            }
            let mut child = leaf;
            while let Some(p) = self.parent[child] {
                let Some(prect) = self.rect_of(p, area, gap) else { break };
                let Node::Split { a, ratio, axis, .. } = *self.node(p) else { unreachable!() };
                let ax = axis.unwrap_or(Axis::longer_side(&prect));
                if ax == want {
                    let usable = (prect.len(ax) - gap).max(1);
                    // growing the first part moves the divider forward; growing the second moves it back
                    let d = (delta as i64 * Ratio::ONE as i64 / usable as i64) as i32;
                    let new = ratio.plus(if child == a { d } else { -d });
                    if let Some(Node::Split { ratio, .. }) = self.nodes[p].as_mut() {
                        *ratio = new;
                    }
                    break;
                }
                child = p;
            }
        }
    }

    fn message(&mut self, msg: LayoutMsg, focused: Option<WinId>) -> bool {
        let Some(p) = focused.and_then(|f| self.parent_split(f)) else { return false };
        match msg {
            LayoutMsg::ToggleSplit => {
                // (a split that followed the tile's shape is fixed to the other direction from now on; the shape is not known here, so
                // `X` flips to `Y` and an unfixed split becomes `Y`, which is the flip of the usual wide tile)
                if let Some(Node::Split { axis, .. }) = self.nodes[p].as_mut() {
                    *axis = Some(axis.map_or(Axis::Y, Axis::flip));
                }
                true
            }
            LayoutMsg::SwapSplit => {
                if let Some(Node::Split { a, b, ratio, .. }) = self.nodes[p].as_mut() {
                    core::mem::swap(a, b);
                    *ratio = Ratio(Ratio::ONE - ratio.0);
                }
                true
            }
            _ => false,
        }
    }
}
