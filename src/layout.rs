//! The workspace layout: a single [`pane_grid`] holding every view.
//!
//! Panes can be docked to any side, resized, and closed. The map is a *fixed*
//! pane: it has no title bar (so it cannot be picked up or dragged) and no
//! close control. Dockable panels are opened from the rail and return to it
//! when closed.

use crate::widget::pane_grid::{self, Axis as PaneAxis, Edge};

/// A view that can occupy a pane in the workspace grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Panel {
    Map,
    Vehicles,
    Parameters,
    Links,
    Logs,
}

/// A serialisable mirror of [`pane_grid::Configuration`] for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LayoutConfig {
    Split {
        axis: SplitAxis,
        ratio: f32,
        a: Box<LayoutConfig>,
        b: Box<LayoutConfig>,
    },
    Pane(Panel),
}

/// A serialisable mirror of [`pane_grid::Axis`].
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum SplitAxis {
    Vertical,
    Horizontal,
}

impl From<PaneAxis> for SplitAxis {
    fn from(axis: PaneAxis) -> Self {
        match axis {
            PaneAxis::Vertical => SplitAxis::Vertical,
            PaneAxis::Horizontal => SplitAxis::Horizontal,
        }
    }
}

impl From<SplitAxis> for PaneAxis {
    fn from(axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Vertical => PaneAxis::Vertical,
            SplitAxis::Horizontal => PaneAxis::Horizontal,
        }
    }
}

impl LayoutConfig {
    fn into_configuration(self) -> pane_grid::Configuration<Panel> {
        match self {
            LayoutConfig::Split { axis, ratio, a, b } => pane_grid::Configuration::Split {
                axis: axis.into(),
                ratio,
                a: Box::new(a.into_configuration()),
                b: Box::new(b.into_configuration()),
            },
            LayoutConfig::Pane(panel) => pane_grid::Configuration::Pane(panel),
        }
    }
}

impl Panel {
    /// Panels offered on the rail, in order. The map is not one of them.
    pub const DOCKABLE: [Panel; 4] = [Panel::Vehicles, Panel::Parameters, Panel::Links, Panel::Logs];

    pub fn title(self) -> &'static str {
        match self {
            Panel::Map => "Map",
            Panel::Vehicles => "Vehicles",
            Panel::Parameters => "Parameters",
            Panel::Links => "Links",
            Panel::Logs => "Log",
        }
    }

    /// A short placeholder for the rail button. Icons land here later.
    pub fn badge(self) -> &'static str {
        match self {
            Panel::Map => "M",
            Panel::Vehicles => "V",
            Panel::Parameters => "P",
            Panel::Links => "C",
            Panel::Logs => "L",
        }
    }

    /// Where the panel opens by default; it can be dragged elsewhere after.
    pub fn default_edge(self) -> Edge {
        match self {
            Panel::Logs => Edge::Bottom,
            _ => Edge::Left,
        }
    }

    /// Fixed panels cannot be dragged or closed.
    pub fn is_fixed(self) -> bool {
        matches!(self, Panel::Map)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Clicked(pane_grid::Pane),
    Dragged(pane_grid::DragEvent),
    Resized(pane_grid::ResizeEvent),
    /// Open a dockable panel (no-op if already open).
    Open(Panel),
    Close(pane_grid::Pane),
    Maximize(pane_grid::Pane),
    Restore,
}

impl Message {
    /// Whether this message can change the persisted arrangement.
    pub fn changes_layout(&self) -> bool {
        matches!(
            self,
            Message::Dragged(pane_grid::DragEvent::Dropped { .. })
                | Message::Resized(_)
                | Message::Open(_)
                | Message::Close(_)
                | Message::Maximize(_)
                | Message::Restore
        )
    }
}

pub struct Layout {
    panes: pane_grid::State<Panel>,
    focus: Option<pane_grid::Pane>,
}

impl Layout {
    pub fn new() -> Self {
        let (panes, _) = pane_grid::State::new(Panel::Map);
        let mut layout = Self {
            panes,
            focus: None,
        };

        // Start with the vehicle HUD visible alongside the map.
        layout.open(Panel::Vehicles);

        layout
    }

    pub fn panes(&self) -> &pane_grid::State<Panel> {
        &self.panes
    }

    /// Rebuild the layout from a persisted configuration. Falls back to the
    /// default layout if the configuration has no map pane.
    pub fn from_configuration(config: LayoutConfig) -> Self {
        let layout = Self {
            panes: pane_grid::State::with_configuration(config.into_configuration()),
            focus: None,
        };

        if layout.is_open(Panel::Map) {
            layout
        } else {
            Self::new()
        }
    }

    /// Snapshot the current arrangement for persistence.
    pub fn configuration(&self) -> Option<LayoutConfig> {
        Self::node_to_config(self.panes.layout(), &self.panes)
    }

    fn node_to_config(
        node: &pane_grid::Node,
        panes: &pane_grid::State<Panel>,
    ) -> Option<LayoutConfig> {
        match node {
            pane_grid::Node::Split { axis, ratio, a, b, .. } => Some(LayoutConfig::Split {
                axis: SplitAxis::from(*axis),
                ratio: *ratio,
                a: Box::new(Self::node_to_config(a, panes)?),
                b: Box::new(Self::node_to_config(b, panes)?),
            }),
            pane_grid::Node::Pane(pane) => Some(LayoutConfig::Pane(*panes.get(*pane)?)),
        }
    }

    pub fn is_focused(&self, pane: pane_grid::Pane) -> bool {
        self.focus == Some(pane)
    }

    /// Whether a panel is currently present in the grid.
    pub fn is_open(&self, panel: Panel) -> bool {
        self.panes.iter().any(|(_, p)| *p == panel)
    }

    fn pane_of(&self, panel: Panel) -> Option<pane_grid::Pane> {
        self.panes
            .iter()
            .find(|(_, p)| **p == panel)
            .map(|(pane, _)| *pane)
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Clicked(pane) => self.focus = Some(pane),
            Message::Dragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                // The fixed map pane may never be moved.
                if self.panes.get(pane) == Some(&Panel::Map) {
                    return;
                }
                self.panes.drop(pane, target);
            }
            Message::Dragged(_) => {}
            Message::Resized(event) => self.panes.resize(event.split, event.ratio),
            Message::Open(panel) => self.open(panel),
            Message::Close(pane) => {
                if self.panes.get(pane) == Some(&Panel::Map) {
                    return;
                }
                if let Some((_, sibling)) = self.panes.close(pane)
                    && self.focus == Some(pane)
                {
                    self.focus = Some(sibling);
                }
            }
            Message::Maximize(pane) => self.panes.maximize(pane),
            Message::Restore => self.panes.restore(),
        }
    }

    fn open(&mut self, panel: Panel) {
        if let Some(pane) = self.pane_of(panel) {
            self.focus = Some(pane);
            return;
        }

        let Some(map) = self.pane_of(Panel::Map) else {
            return;
        };

        let Some((new_pane, _)) = self.panes.split(PaneAxis::Vertical, map, panel) else {
            return;
        };

        // Open as a new column on the far left, next to the rail.
        self.panes.move_to_edge(new_pane, panel.default_edge());
        self.focus = Some(new_pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_through_config() {
        let layout = Layout::new();
        assert!(layout.is_open(Panel::Map));
        assert!(layout.is_open(Panel::Vehicles));

        let config = layout.configuration().expect("snapshot");
        let serialized = ron::to_string(&config).expect("serialize");
        let restored: LayoutConfig = ron::from_str(&serialized).expect("deserialize");

        let rebuilt = Layout::from_configuration(restored);
        assert!(rebuilt.is_open(Panel::Map));
        assert!(rebuilt.is_open(Panel::Vehicles));

        // The rebuilt layout snapshots to the same structure.
        let reserialized =
            ron::to_string(&rebuilt.configuration().expect("snapshot")).expect("serialize");
        assert_eq!(serialized, reserialized);
    }

    #[test]
    fn configuration_without_map_falls_back_to_default() {
        let config = LayoutConfig::Pane(Panel::Vehicles);
        let layout = Layout::from_configuration(config);
        assert!(layout.is_open(Panel::Map));
    }
}
