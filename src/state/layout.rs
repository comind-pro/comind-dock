//! Binary split tree — the correctness core of the pane model.
//! Pure data: no PTYs, no async, no emulator types.

use ratatui::layout::Rect;

use serde::{Deserialize, Serialize};

use super::ids::PaneId;

/// Split direction: where the *new* pane goes relative to the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    Right,
    Down,
}

/// Focus movement direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(PaneId),
    Split { dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// A divider line between two sibling subtrees, for rendering and mouse drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divider {
    /// The divider occupies this 1-cell-thick rect.
    pub rect: Rect,
    /// Direction of the split that owns it (Right → vertical line).
    pub dir: Dir,
    /// Pane adjacent on the left/top; with `after`, identifies the split.
    pub before: PaneId,
    /// Pane adjacent on the right/bottom.
    pub after: PaneId,
    /// Axis size of the split's usable area — converts drag cells to ratio.
    pub extent: u16,
}

const MIN_RATIO: f32 = 0.05;

impl Node {
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.collect_panes(out);
                b.collect_panes(out);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(p) => *p == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Replace `Leaf(target)` with a split of (target, new). `before` puts
    /// the new pane on the left/top side. Returns false if absent.
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: Dir, before: bool) -> bool {
        match self {
            Node::Leaf(p) if *p == target => {
                let (first, second) = if before { (new, target) } else { (target, new) };
                *self = Node::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(first)),
                    b: Box::new(Node::Leaf(second)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                a.split(target, new, dir, before) || b.split(target, new, dir, before)
            }
        }
    }

    /// Replace `Leaf(at)` with a 0.5 split of (at, subtree); `side` says
    /// where the subtree lands. Returns false (tree untouched) if absent.
    pub fn graft(&mut self, at: PaneId, subtree: Node, side: Side) -> bool {
        if !self.contains(at) {
            return false;
        }
        self.graft_inner(at, subtree, side);
        true
    }

    fn graft_inner(&mut self, at: PaneId, subtree: Node, side: Side) {
        match self {
            Node::Leaf(_) => {
                let old = std::mem::replace(self, Node::Leaf(PaneId(u64::MAX)));
                let (dir, before) = match side {
                    Side::Left => (Dir::Right, true),
                    Side::Right => (Dir::Right, false),
                    Side::Up => (Dir::Down, true),
                    Side::Down => (Dir::Down, false),
                };
                let (a, b) = if before { (subtree, old) } else { (old, subtree) };
                *self = Node::Split { dir, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
            }
            Node::Split { a, b, .. } => {
                if a.contains(at) {
                    a.graft_inner(at, subtree, side);
                } else {
                    b.graft_inner(at, subtree, side);
                }
            }
        }
    }

    /// Remove `Leaf(target)`, promoting its sibling. Returns false if absent
    /// or if the tree is a bare root leaf (caller closes the tab instead).
    pub fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                if matches!(**a, Node::Leaf(p) if p == target) {
                    *self = std::mem::replace(b, Node::Leaf(PaneId(u64::MAX)));
                    true
                } else if matches!(**b, Node::Leaf(p) if p == target) {
                    *self = std::mem::replace(a, Node::Leaf(PaneId(u64::MAX)));
                    true
                } else {
                    a.remove(target) || b.remove(target)
                }
            }
        }
    }

    /// Exchange two pane ids; tree shape is untouched.
    pub fn swap(&mut self, x: PaneId, y: PaneId) {
        match self {
            Node::Leaf(p) => {
                if *p == x {
                    *p = y;
                } else if *p == y {
                    *p = x;
                }
            }
            Node::Split { a, b, .. } => {
                a.swap(x, y);
                b.swap(x, y);
            }
        }
    }

    /// Adjust the ratio of the specific split whose divider separates
    /// `before` (in subtree a) from `after` (in subtree b) — used by border drag.
    pub fn resize_split(&mut self, before: PaneId, after: PaneId, delta: f32) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { ratio, a, b, .. } => {
                if a.contains(before) && b.contains(after) {
                    *ratio = (*ratio + delta).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                    true
                } else {
                    a.resize_split(before, after, delta) || b.resize_split(before, after, delta)
                }
            }
        }
    }

    /// Nudge the ratio of the nearest ancestor split of `axis` that contains
    /// `target`. Positive delta grows the side holding `target`.
    pub fn resize(&mut self, target: PaneId, axis: Dir, delta: f32) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { dir, ratio, a, b } => {
                // Prefer the deepest matching split so resize feels local.
                if a.resize(target, axis, delta) || b.resize(target, axis, delta) {
                    return true;
                }
                if *dir == axis {
                    let signed = if a.contains(target) {
                        delta
                    } else if b.contains(target) {
                        -delta
                    } else {
                        return false;
                    };
                    *ratio = (*ratio + signed).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Compute pane rects and divider segments. Siblings are separated by a
    /// 1-cell divider line.
    pub fn layout(&self, area: Rect) -> (Vec<(PaneId, Rect)>, Vec<Divider>) {
        let mut rects = Vec::new();
        let mut dividers = Vec::new();
        self.layout_into(area, &mut rects, &mut dividers);
        (rects, dividers)
    }

    fn layout_into(
        &self,
        area: Rect,
        rects: &mut Vec<(PaneId, Rect)>,
        dividers: &mut Vec<Divider>,
    ) {
        match self {
            Node::Leaf(id) => rects.push((*id, area)),
            Node::Split { dir: Dir::Right, ratio, a, b } => {
                if area.width < 3 {
                    // Too narrow to split visibly; first side wins.
                    a.layout_into(area, rects, dividers);
                    return;
                }
                let usable = area.width - 1;
                let wa = ((usable as f32 * ratio).round() as u16).clamp(1, usable - 1);
                let ra = Rect { width: wa, ..area };
                let dv = Rect { x: area.x + wa, width: 1, ..area };
                let rb = Rect { x: area.x + wa + 1, width: usable - wa, ..area };
                a.layout_into(ra, rects, dividers);
                dividers.push(Divider {
                    rect: dv,
                    dir: Dir::Right,
                    before: rightmost(a),
                    after: leftmost(b),
                    extent: usable,
                });
                b.layout_into(rb, rects, dividers);
            }
            Node::Split { dir: Dir::Down, ratio, a, b } => {
                if area.height < 3 {
                    a.layout_into(area, rects, dividers);
                    return;
                }
                let usable = area.height - 1;
                let ha = ((usable as f32 * ratio).round() as u16).clamp(1, usable - 1);
                let ra = Rect { height: ha, ..area };
                let dv = Rect { y: area.y + ha, height: 1, ..area };
                let rb = Rect { y: area.y + ha + 1, height: usable - ha, ..area };
                a.layout_into(ra, rects, dividers);
                dividers.push(Divider {
                    rect: dv,
                    dir: Dir::Down,
                    before: rightmost(a),
                    after: leftmost(b),
                    extent: usable,
                });
                b.layout_into(rb, rects, dividers);
            }
        }
    }
}

/// Bottom-right-most pane of a subtree — the pane adjacent to a divider.
fn rightmost(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { b, .. } => rightmost(b),
    }
}

/// Top-left-most pane of a subtree.
fn leftmost(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { a, .. } => leftmost(a),
    }
}

/// Geometric neighbor lookup over computed rects: the adjacent pane past the
/// focused edge with the largest cross-axis overlap. Works for any nesting.
pub fn neighbor(rects: &[(PaneId, Rect)], from: PaneId, side: Side) -> Option<PaneId> {
    let (_, fr) = rects.iter().find(|(id, _)| *id == from)?;
    let mut best: Option<(PaneId, u32)> = None;
    for (id, r) in rects {
        if *id == from {
            continue;
        }
        // Adjacent = separated exactly by the 1-cell divider.
        let adjacent = match side {
            Side::Left => r.x + r.width + 1 == fr.x,
            Side::Right => fr.x + fr.width + 1 == r.x,
            Side::Up => r.y + r.height + 1 == fr.y,
            Side::Down => fr.y + fr.height + 1 == r.y,
        };
        if !adjacent {
            continue;
        }
        let overlap = match side {
            Side::Left | Side::Right => overlap_len(fr.y, fr.height, r.y, r.height),
            Side::Up | Side::Down => overlap_len(fr.x, fr.width, r.x, r.width),
        };
        if overlap > 0 && best.is_none_or(|(_, b)| overlap > b) {
            best = Some((*id, overlap));
        }
    }
    best.map(|(id, _)| id)
}

fn overlap_len(a: u16, alen: u16, b: u16, blen: u16) -> u32 {
    let start = a.max(b) as i64;
    let end = (a as i64 + alen as i64).min(b as i64 + blen as i64);
    (end - start).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u64) -> PaneId {
        PaneId(n)
    }

    fn area() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    #[test]
    fn split_replaces_leaf() {
        let mut n = Node::Leaf(p(1));
        assert!(n.split(p(1), p(2), Dir::Right, false));
        assert_eq!(n.panes(), vec![p(1), p(2)]);
        assert!(!n.split(p(99), p(3), Dir::Down, false));
    }

    #[test]
    fn remove_promotes_sibling() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        n.split(p(2), p(3), Dir::Down, false);
        assert!(n.remove(p(2)));
        assert_eq!(n.panes(), vec![p(1), p(3)]);
        assert!(n.remove(p(3)));
        assert_eq!(n, Node::Leaf(p(1)));
        // Bare root leaf: caller's job.
        assert!(!n.remove(p(1)));
    }

    #[test]
    fn swap_keeps_shape() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        n.split(p(2), p(3), Dir::Down, false);
        let (rects_before, _) = n.layout(area());
        n.swap(p(1), p(3));
        let (rects_after, _) = n.layout(area());
        assert_eq!(rects_before.len(), rects_after.len());
        let find = |rects: &[(PaneId, Rect)], id| rects.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!(find(&rects_before, p(1)), find(&rects_after, p(3)));
        assert_eq!(find(&rects_before, p(3)), find(&rects_after, p(1)));
    }

    #[test]
    fn layout_covers_area_with_dividers() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        n.split(p(2), p(3), Dir::Down, false);
        let (rects, dividers) = n.layout(area());
        assert_eq!(rects.len(), 3);
        assert_eq!(dividers.len(), 2);
        let cells: u32 = rects.iter().map(|(_, r)| r.width as u32 * r.height as u32).sum();
        let divider_cells: u32 =
            dividers.iter().map(|d| d.rect.width as u32 * d.rect.height as u32).sum();
        assert_eq!(cells + divider_cells, 80 * 24);
        // No overlaps.
        for (i, (_, r1)) in rects.iter().enumerate() {
            for (_, r2) in rects.iter().skip(i + 1) {
                assert!(!r1.intersects(*r2), "{r1:?} overlaps {r2:?}");
            }
        }
    }

    #[test]
    fn resize_adjusts_nearest_axis_ancestor() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        let (before, _) = n.layout(area());
        assert!(n.resize(p(1), Dir::Right, 0.1));
        let (after, _) = n.layout(area());
        let w =
            |rects: &[(PaneId, Rect)], id| rects.iter().find(|(i, _)| *i == id).unwrap().1.width;
        assert!(w(&after, p(1)) > w(&before, p(1)));
        // No vertical split anywhere → vertical resize is a no-op.
        assert!(!n.resize(p(1), Dir::Down, 0.1));
    }

    #[test]
    fn resize_ratio_clamped() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        for _ in 0..100 {
            n.resize(p(1), Dir::Right, 0.1);
        }
        let Node::Split { ratio, .. } = n else { panic!() };
        assert!(ratio > 0.0 && ratio < 1.0);
    }

    #[test]
    fn neighbor_finds_adjacent_with_max_overlap() {
        // [1 | 2] with 2 split into 2-over-3.
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        n.split(p(2), p(3), Dir::Down, false);
        let (rects, _) = n.layout(area());
        assert_eq!(neighbor(&rects, p(1), Side::Right), Some(p(2)));
        assert_eq!(neighbor(&rects, p(2), Side::Left), Some(p(1)));
        assert_eq!(neighbor(&rects, p(2), Side::Down), Some(p(3)));
        assert_eq!(neighbor(&rects, p(3), Side::Up), Some(p(2)));
        assert_eq!(neighbor(&rects, p(1), Side::Left), None);
        assert_eq!(neighbor(&rects, p(1), Side::Up), None);
    }

    #[test]
    fn resize_split_targets_exact_divider() {
        // [[1|2]|3]: the outer divider separates 2 and 3; resizing it must not
        // touch the inner [1|2] split.
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(3), Dir::Right, false);
        n.split(p(1), p(2), Dir::Right, false);
        let inner_before = match &n {
            Node::Split { a, .. } => match &**a {
                Node::Split { ratio, .. } => *ratio,
                _ => panic!(),
            },
            _ => panic!(),
        };
        assert!(n.resize_split(p(2), p(3), 0.2));
        let (outer_after, inner_after) = match &n {
            Node::Split { ratio, a, .. } => match &**a {
                Node::Split { ratio: ir, .. } => (*ratio, *ir),
                _ => panic!(),
            },
            _ => panic!(),
        };
        assert!((outer_after - 0.7).abs() < 1e-6);
        assert_eq!(inner_after, inner_before);
    }

    #[test]
    fn tiny_area_degrades_gracefully() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        let (rects, dividers) = n.layout(Rect::new(0, 0, 2, 2));
        assert_eq!(rects.len(), 1);
        assert!(dividers.is_empty());
    }

    #[test]
    fn graft_inserts_subtree_on_each_side() {
        // Subtree [3|4] grafted left of leaf 1 in [1|2]:
        // expected pane order (in-order) becomes [3,4], 1, 2.
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        let mut sub = Node::Leaf(p(3));
        sub.split(p(3), p(4), Dir::Right, false);
        assert!(n.graft(p(1), sub, Side::Left));
        assert_eq!(n.panes(), vec![p(3), p(4), p(1), p(2)]);

        // Down puts the subtree second.
        let mut n = Node::Leaf(p(1));
        assert!(n.graft(p(1), Node::Leaf(p(2)), Side::Down));
        assert_eq!(n.panes(), vec![p(1), p(2)]);
        let Node::Split { dir, ratio, .. } = &n else { panic!() };
        assert_eq!(*dir, Dir::Down);
        assert!((ratio - 0.5).abs() < 1e-6);

        // Up puts it first.
        let mut n = Node::Leaf(p(1));
        assert!(n.graft(p(1), Node::Leaf(p(2)), Side::Up));
        assert_eq!(n.panes(), vec![p(2), p(1)]);
    }

    #[test]
    fn graft_right_puts_subtree_second() {
        let mut n = Node::Leaf(p(1));
        assert!(n.graft(p(1), Node::Leaf(p(2)), Side::Right));
        assert_eq!(n.panes(), vec![p(1), p(2)]);
        let Node::Split { dir, .. } = &n else { panic!() };
        assert_eq!(*dir, Dir::Right);
    }

    #[test]
    fn graft_missing_target_leaves_tree_untouched() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        let before = n.clone();
        assert!(!n.graft(p(99), Node::Leaf(p(3)), Side::Right));
        assert_eq!(n, before);
    }
}
