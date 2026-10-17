use super::{Axis, Pane, Split};
use iced::{Rectangle, Size};

use std::collections::BTreeMap;

/// A layout node of a [`PaneGrid`].
///
/// [`PaneGrid`]: super::PaneGrid
#[derive(Debug, Clone)]
pub enum Node {
    /// The region of this [`Node`] is split into two.
    Split {
        /// The [`Split`] of this [`Node`].
        id: Split,

        /// The direction of the split.
        axis: Axis,

        /// The ratio of the split in [0.0, 1.0].
        ratio: f32,

        /// The left/top [`Node`] of the split.
        a: Box<Node>,

        /// The right/bottom [`Node`] of the split.
        b: Box<Node>,
    },
    /// The region of this [`Node`] is taken by a [`Pane`].
    Pane(Pane),
}

#[derive(Debug)]
enum Count {
    Split {
        horizontal: usize,
        vertical: usize,
        a: Box<Count>,
        b: Box<Count>,
    },
    Pane,
}

impl Count {
    fn horizontal(&self) -> usize {
        match self {
            Count::Split { horizontal, .. } => *horizontal,
            Count::Pane => 0,
        }
    }

    fn vertical(&self) -> usize {
        match self {
            Count::Split { vertical, .. } => *vertical,
            Count::Pane => 0,
        }
    }
}

impl Node {
    /// Returns an iterator over each [`Split`] in this [`Node`].
    pub fn splits(&self) -> impl Iterator<Item = &Split> {
        let mut unvisited_nodes = vec![self];

        std::iter::from_fn(move || {
            while let Some(node) = unvisited_nodes.pop() {
                if let Node::Split { id, a, b, .. } = node {
                    unvisited_nodes.push(a);
                    unvisited_nodes.push(b);

                    return Some(id);
                }
            }

            None
        })
    }

    fn count(&self) -> Count {
        match self {
            Node::Split { a, b, axis, .. } => {
                let a = a.count();
                let b = b.count();

                let (horizontal, vertical) = match axis {
                    Axis::Horizontal => (
                        1 + a.horizontal() + b.horizontal(),
                        a.vertical().max(b.vertical()),
                    ),
                    Axis::Vertical => (
                        a.horizontal().max(b.horizontal()),
                        1 + a.vertical() + b.vertical(),
                    ),
                };

                Count::Split {
                    horizontal,
                    vertical,
                    a: Box::new(a),
                    b: Box::new(b),
                }
            }
            Node::Pane(_) => Count::Pane,
        }
    }

    /// Returns the rectangular region for each [`Pane`] in the [`Node`] given
    /// the spacing between panes and the total available space.
    pub fn pane_regions(
        &self,
        spacing: f32,
        min_size: f32,
        bounds: Size,
    ) -> BTreeMap<Pane, Rectangle> {
        let mut regions = BTreeMap::new();
        let count = self.count();

        self.compute_regions(
            spacing,
            min_size,
            &Rectangle {
                x: 0.0,
                y: 0.0,
                width: bounds.width,
                height: bounds.height,
            },
            &count,
            &mut regions,
        );

        regions
    }

    /// Returns the axis, rectangular region, and ratio for each [`Split`] in
    /// the [`Node`] given the spacing between panes and the total available
    /// space.
    pub fn split_regions(
        &self,
        spacing: f32,
        min_size: f32,
        bounds: Size,
    ) -> BTreeMap<Split, (Axis, Rectangle, f32)> {
        let mut splits = BTreeMap::new();
        let count = self.count();

        self.compute_splits(
            spacing,
            min_size,
            &Rectangle {
                x: 0.0,
                y: 0.0,
                width: bounds.width,
                height: bounds.height,
            },
            &count,
            &mut splits,
        );

        splits
    }

    pub(crate) fn find(&mut self, pane: Pane) -> Option<&mut Node> {
        match self {
            Node::Split { a, b, .. } => a.find(pane).or_else(move || b.find(pane)),
            Node::Pane(p) => {
                if *p == pane {
                    Some(self)
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn split(&mut self, id: Split, axis: Axis, new_pane: Pane) {
        *self = Node::Split {
            id,
            axis,
            ratio: 0.5,
            a: Box::new(self.clone()),
            b: Box::new(Node::Pane(new_pane)),
        };
    }

    pub(crate) fn split_inverse(&mut self, id: Split, axis: Axis, pane: Pane) {
        *self = Node::Split {
            id,
            axis,
            ratio: 0.5,
            a: Box::new(Node::Pane(pane)),
            b: Box::new(self.clone()),
        };
    }

    pub(crate) fn update(&mut self, f: &impl Fn(&mut Node)) {
        if let Node::Split { a, b, .. } = self {
            a.update(f);
            b.update(f);
        }

        f(self);
    }

    pub(crate) fn resize(&mut self, split: Split, percentage: f32) -> bool {
        match self {
            Node::Split {
                id,
                axis,
                ratio,
                a,
                b,
            } => {
                if *id == split {
                    let previous = *ratio;
                    *ratio = percentage;

                    // Keep every other divider in place: only the panes adjacent
                    // to this one trade space. Each side of the split absorbs the
                    // change into its *neighbouring* leaf (the last of the left
                    // side, the first of the right side), leaving the rest of the
                    // tree untouched. Without this, a nested split's ratio stays
                    // relative to its shrinking group, so its divider drifts too.
                    //
                    // Only same-axis splits are adjusted; a cross-axis subtree
                    // absorbs as a whole (its ratio must not change, or the
                    // perpendicular fractions would drift).
                    let axis = *axis;
                    grow_last_leaf(a, axis, previous, percentage);
                    grow_first_leaf(b, axis, 1.0 - previous, 1.0 - percentage);

                    true
                } else if a.resize(split, percentage) {
                    true
                } else {
                    b.resize(split, percentage)
                }
            }
            Node::Pane(_) => false,
        }
    }

    pub(crate) fn remove(&mut self, pane: Pane) -> Option<Pane> {
        match self {
            Node::Split { a, b, .. } => {
                if a.pane() == Some(pane) {
                    *self = *b.clone();
                    Some(self.first_pane())
                } else if b.pane() == Some(pane) {
                    *self = *a.clone();
                    Some(self.first_pane())
                } else {
                    a.remove(pane).or_else(|| b.remove(pane))
                }
            }
            Node::Pane(_) => None,
        }
    }

    fn pane(&self) -> Option<Pane> {
        match self {
            Node::Split { .. } => None,
            Node::Pane(pane) => Some(*pane),
        }
    }

    fn first_pane(&self) -> Pane {
        match self {
            Node::Split { a, .. } => a.first_pane(),
            Node::Pane(pane) => *pane,
        }
    }

    fn compute_regions(
        &self,
        spacing: f32,
        min_size: f32,
        current: &Rectangle,
        count: &Count,
        regions: &mut BTreeMap<Pane, Rectangle>,
    ) {
        match (self, count) {
            (
                Node::Split {
                    axis, ratio, a, b, ..
                },
                Count::Split {
                    a: count_a,
                    b: count_b,
                    ..
                },
            ) => {
                let (a_factor, b_factor) = match axis {
                    Axis::Horizontal => (count_a.horizontal(), count_b.horizontal()),
                    Axis::Vertical => (count_a.vertical(), count_b.vertical()),
                };

                let (region_a, region_b, _ratio) = axis.split(
                    current,
                    *ratio,
                    spacing,
                    min_size * (a_factor + 1) as f32 + spacing * a_factor as f32,
                    min_size * (b_factor + 1) as f32 + spacing * b_factor as f32,
                );

                a.compute_regions(spacing, min_size, &region_a, count_a, regions);
                b.compute_regions(spacing, min_size, &region_b, count_b, regions);
            }
            (Node::Pane(pane), Count::Pane) => {
                let _ = regions.insert(*pane, *current);
            }
            _ => {
                unreachable!("Node configuration and count do not match")
            }
        }
    }

    fn compute_splits(
        &self,
        spacing: f32,
        min_size: f32,
        current: &Rectangle,
        count: &Count,
        splits: &mut BTreeMap<Split, (Axis, Rectangle, f32)>,
    ) {
        match (self, count) {
            (
                Node::Split {
                    axis,
                    ratio,
                    a,
                    b,
                    id,
                },
                Count::Split {
                    a: count_a,
                    b: count_b,
                    ..
                },
            ) => {
                let (a_factor, b_factor) = match axis {
                    Axis::Horizontal => (count_a.horizontal(), count_b.horizontal()),
                    Axis::Vertical => (count_a.vertical(), count_b.vertical()),
                };

                let (region_a, region_b, ratio) = axis.split(
                    current,
                    *ratio,
                    spacing,
                    min_size * (a_factor + 1) as f32 + spacing * a_factor as f32,
                    min_size * (b_factor + 1) as f32 + spacing * b_factor as f32,
                );

                let _ = splits.insert(*id, (*axis, *current, ratio));

                a.compute_splits(spacing, min_size, &region_a, count_a, splits);
                b.compute_splits(spacing, min_size, &region_b, count_b, splits);
            }
            (Node::Pane(_), Count::Pane) => {}
            _ => {
                unreachable!("Node configuration and split count do not match")
            }
        }
    }
}

impl std::hash::Hash for Node {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Node::Split {
                id,
                axis,
                ratio,
                a,
                b,
            } => {
                id.hash(state);
                axis.hash(state);
                ((ratio * 100_000.0) as u32).hash(state);
                a.hash(state);
                b.hash(state);
            }
            Node::Pane(pane) => {
                pane.hash(state);
            }
        }
    }
}

/// Resize `node` from `old` to `new` along `axis`, keeping its last
/// (right/bottom) leaf's siblings fixed; that last leaf absorbs the change.
/// Cross-axis splits are left untouched. `old`/`new` are sizes of `node` in any
/// consistent unit (the maths is scale-invariant).
fn grow_last_leaf(node: &mut Node, axis: Axis, old: f32, new: f32) {
    let Node::Split {
        axis: node_axis,
        ratio,
        b,
        ..
    } = node
    else {
        return;
    };

    if *node_axis != axis || old <= 0.0 || new <= 0.0 {
        return;
    }

    let ratio_before = *ratio;
    let a_old = old * ratio_before;
    let b_old = old * (1.0 - ratio_before);

    // Keep `a` fixed; `b` takes the rest.
    let b_new = (new - a_old).clamp(0.0, new);
    *ratio = 1.0 - b_new / new;

    grow_last_leaf(b, axis, b_old, b_new);
}

/// Resize `node` from `old` to `new` along `axis`, keeping its first
/// (left/top) leaf's siblings fixed; that first leaf absorbs the change.
/// Cross-axis splits are left untouched.
fn grow_first_leaf(node: &mut Node, axis: Axis, old: f32, new: f32) {
    let Node::Split {
        axis: node_axis,
        ratio,
        a,
        ..
    } = node
    else {
        return;
    };

    if *node_axis != axis || old <= 0.0 || new <= 0.0 {
        return;
    }

    let ratio_before = *ratio;
    let a_old = old * ratio_before;
    let b_old = old * (1.0 - ratio_before);

    // Keep `b` fixed; `a` takes the rest.
    let a_new = (new - b_old).clamp(0.0, new);
    *ratio = a_new / new;

    grow_first_leaf(a, axis, a_old, a_new);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: usize) -> Node {
        Node::Pane(Pane(id))
    }

    #[test]
    fn resizing_outer_split_keeps_far_divider_fixed() {
        // A vertical `[A, [B, C]]` tree.
        let mut node = Node::Split {
            id: Split(0),
            axis: Axis::Vertical,
            ratio: 0.5,
            a: Box::new(pane(0)),
            b: Box::new(Node::Split {
                id: Split(1),
                axis: Axis::Vertical,
                ratio: 0.5,
                a: Box::new(pane(1)),
                b: Box::new(pane(2)),
            }),
        };

        let size = Size::new(300.0, 100.0);
        let before = node.pane_regions(0.0, 0.0, size);
        let c_before = before[&Pane(2)];

        assert!(node.resize(Split(0), 0.6));

        let after = node.pane_regions(0.0, 0.0, size);

        // The far pane (C) is untouched, so its divider did not move...
        assert!((after[&Pane(2)].x - c_before.x).abs() < 1e-3);
        assert!((after[&Pane(2)].width - c_before.width).abs() < 1e-3);
        // ...A grew, and its neighbour B absorbed the difference.
        assert!((after[&Pane(0)].width - 180.0).abs() < 1e-3);
        assert!((after[&Pane(1)].width - 45.0).abs() < 1e-3);
    }

    #[test]
    fn resizing_one_axis_does_not_change_the_other() {
        // A vertical split whose right side is a horizontal split: `[A, [B / C]]`.
        let mut node = Node::Split {
            id: Split(0),
            axis: Axis::Vertical,
            ratio: 0.5,
            a: Box::new(pane(0)),
            b: Box::new(Node::Split {
                id: Split(1),
                axis: Axis::Horizontal,
                ratio: 0.5,
                a: Box::new(pane(1)),
                b: Box::new(pane(2)),
            }),
        };

        let size = Size::new(300.0, 100.0);
        let before = node.pane_regions(0.0, 0.0, size);

        assert!(node.resize(Split(0), 0.6));

        let after = node.pane_regions(0.0, 0.0, size);

        // The horizontal split (the vertical fraction) must not move.
        assert!((after[&Pane(1)].y - before[&Pane(1)].y).abs() < 1e-3);
        assert!((after[&Pane(1)].height - before[&Pane(1)].height).abs() < 1e-3);
        assert!((after[&Pane(2)].y - before[&Pane(2)].y).abs() < 1e-3);
        assert!((after[&Pane(2)].height - before[&Pane(2)].height).abs() < 1e-3);
    }
}
