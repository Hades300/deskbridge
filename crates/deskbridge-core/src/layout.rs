use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}

impl FromStr for Edge {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            _ => Err(format!(
                "expected one of: left, right, top, bottom; got {value:?}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// A physical display within a machine, expressed in that machine's local
/// desktop coordinates (relative to the screen's top-left). When a screen lists
/// monitors, routing snaps incoming cursor positions onto the nearest monitor so
/// the pointer never lands in a gap between non-adjacent displays.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Monitor {
    pub origin: Point,
    pub size: Size,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub size: Size,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<Monitor>,
}

/// A rectangle in a screen's local coordinate space.
#[derive(Debug, Clone, Copy)]
struct LocalRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl LocalRect {
    fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

impl Screen {
    /// Local rectangles for each monitor, falling back to the whole desktop
    /// bounding box when no monitors are configured (the legacy single-display
    /// case).
    fn monitor_rects(&self) -> Vec<LocalRect> {
        if self.monitors.is_empty() {
            return vec![LocalRect {
                x: 0,
                y: 0,
                w: self.size.width,
                h: self.size.height,
            }];
        }
        self.monitors
            .iter()
            .map(|monitor| LocalRect {
                x: monitor.origin.x.max(0) as u32,
                y: monitor.origin.y.max(0) as u32,
                w: monitor.size.width,
                h: monitor.size.height,
            })
            .collect()
    }

    /// Snap a point (in this screen's local coordinates) onto the nearest
    /// monitor. Points already on a monitor are returned unchanged.
    pub fn clamp_point_to_monitor(&self, x: u32, y: u32) -> (u32, u32) {
        let rects = self.monitor_rects();
        if rects.iter().any(|rect| rect.contains(x, y)) {
            return (x, y);
        }

        let mut best: Option<((u32, u32), u64)> = None;
        for rect in &rects {
            let max_x = rect.x + rect.w.saturating_sub(1);
            let max_y = rect.y + rect.h.saturating_sub(1);
            let cx = x.clamp(rect.x, max_x);
            let cy = y.clamp(rect.y, max_y);
            let dx = (cx as i64 - x as i64).unsigned_abs();
            let dy = (cy as i64 - y as i64).unsigned_abs();
            let dist = dx * dx + dy * dy;
            if best.is_none_or(|(_, best_dist)| dist < best_dist) {
                best = Some(((cx, cy), dist));
            }
        }
        best.map(|(point, _)| point).unwrap_or((x, y))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Link {
    pub from: String,
    pub edge: Edge,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Layout {
    pub screens: Vec<Screen>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub target_screen: String,
    pub target_edge: Edge,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LayoutError {
    #[error("layout must include at least one screen")]
    Empty,
    #[error("duplicate screen name '{0}'")]
    DuplicateScreen(String),
    #[error("link references unknown screen '{0}'")]
    UnknownScreen(String),
    #[error("screen '{0}' has invalid size")]
    InvalidSize(String),
    #[error("screen '{screen}' has an invalid monitor: {reason}")]
    InvalidMonitor { screen: String, reason: String },
}

impl Layout {
    pub fn single(screen_name: impl Into<String>) -> Self {
        Self {
            screens: vec![Screen {
                name: screen_name.into(),
                size: Size {
                    width: 1920,
                    height: 1080,
                },
                origin: None,
                monitors: Vec::new(),
            }],
            links: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        if self.screens.is_empty() {
            return Err(LayoutError::Empty);
        }

        let mut names = HashSet::new();
        for screen in &self.screens {
            if screen.size.width == 0 || screen.size.height == 0 {
                return Err(LayoutError::InvalidSize(screen.name.clone()));
            }
            if !names.insert(screen.name.clone()) {
                return Err(LayoutError::DuplicateScreen(screen.name.clone()));
            }
            for monitor in &screen.monitors {
                if monitor.size.width == 0 || monitor.size.height == 0 {
                    return Err(LayoutError::InvalidMonitor {
                        screen: screen.name.clone(),
                        reason: "monitor has zero size".to_string(),
                    });
                }
                if monitor.origin.x < 0 || monitor.origin.y < 0 {
                    return Err(LayoutError::InvalidMonitor {
                        screen: screen.name.clone(),
                        reason: "monitor origin must be within the desktop".to_string(),
                    });
                }
                let right = monitor.origin.x as i64 + monitor.size.width as i64;
                let bottom = monitor.origin.y as i64 + monitor.size.height as i64;
                if right > screen.size.width as i64 || bottom > screen.size.height as i64 {
                    return Err(LayoutError::InvalidMonitor {
                        screen: screen.name.clone(),
                        reason: "monitor extends past the desktop bounds".to_string(),
                    });
                }
            }
        }

        for link in &self.links {
            if !names.contains(&link.from) {
                return Err(LayoutError::UnknownScreen(link.from.clone()));
            }
            if !names.contains(&link.to) {
                return Err(LayoutError::UnknownScreen(link.to.clone()));
            }
        }

        Ok(())
    }

    pub fn allowed_clients(&self, server_name: &str) -> Vec<String> {
        self.screens
            .iter()
            .filter(|screen| screen.name != server_name)
            .map(|screen| screen.name.clone())
            .collect()
    }

    pub fn set_screen_size_preserving_links(&mut self, screen_name: &str, size: Size) -> bool {
        let old_layout = self.clone();
        let Some(screen) = self
            .screens
            .iter_mut()
            .find(|screen| screen.name == screen_name)
        else {
            return false;
        };

        if screen.size == size {
            return true;
        }

        screen.size = size;
        self.reconcile_linked_origins(&old_layout, screen_name);
        true
    }

    pub fn transition(&self, from: &str, edge: Edge, x: u32, y: u32) -> Option<Transition> {
        let screen_by_name = self
            .screens
            .iter()
            .map(|screen| (screen.name.as_str(), screen))
            .collect::<HashMap<_, _>>();
        let source = screen_by_name.get(from)?;
        let link = self
            .links
            .iter()
            .find(|link| link.from == from && link.edge == edge)?;
        let target = screen_by_name.get(link.to.as_str())?;

        if source.origin.is_some() && target.origin.is_some() {
            let transition = positioned_transition(source, target, edge, x, y)?;
            let (tx, ty) = target.clamp_point_to_monitor(transition.0, transition.1);
            return Some(Transition {
                target_screen: link.to.clone(),
                target_edge: edge.opposite(),
                x: tx,
                y: ty,
            });
        }

        let (mapped_x, mapped_y) = match edge {
            Edge::Left => (
                target.size.width.saturating_sub(2),
                scale(y, source.size.height, target.size.height),
            ),
            Edge::Right => (1, scale(y, source.size.height, target.size.height)),
            Edge::Top => (
                scale(x, source.size.width, target.size.width),
                target.size.height.saturating_sub(2),
            ),
            Edge::Bottom => (scale(x, source.size.width, target.size.width), 1),
        };

        let (mapped_x, mapped_y) = target.clamp_point_to_monitor(mapped_x, mapped_y);
        Some(Transition {
            target_screen: link.to.clone(),
            target_edge: edge.opposite(),
            x: mapped_x,
            y: mapped_y,
        })
    }

    fn reconcile_linked_origins(&mut self, old_layout: &Layout, changed_screen: &str) {
        for link in self.links.clone() {
            if link.from == changed_screen || link.to == changed_screen {
                self.reconcile_target_origin(old_layout, &link);
            }
        }
    }

    fn reconcile_target_origin(&mut self, old_layout: &Layout, link: &Link) {
        let Some(old_source) = old_layout.screen(&link.from) else {
            return;
        };
        let Some(old_target) = old_layout.screen(&link.to) else {
            return;
        };
        let Some(new_source) = self.screen(&link.from).cloned() else {
            return;
        };
        let Some(target_index) = self
            .screens
            .iter()
            .position(|screen| screen.name == link.to)
        else {
            return;
        };

        let Some(source_origin) = new_source.origin else {
            return;
        };
        let Some(mut target_origin) = self.screens[target_index].origin else {
            return;
        };
        let Some(old_source_rect) = ScreenRect::from_screen(old_source) else {
            return;
        };
        let Some(old_target_rect) = ScreenRect::from_screen(old_target) else {
            return;
        };

        let source_width = size_to_i32(new_source.size.width);
        let source_height = size_to_i32(new_source.size.height);
        let target_width = size_to_i32(self.screens[target_index].size.width);
        let target_height = size_to_i32(self.screens[target_index].size.height);

        match link.edge {
            Edge::Left => {
                target_origin.x = source_origin.x.saturating_sub(target_width);
                target_origin.y = reconcile_vertical_origin(
                    source_origin.y,
                    source_height,
                    target_height,
                    old_source_rect,
                    old_target_rect,
                );
            }
            Edge::Right => {
                target_origin.x = source_origin.x.saturating_add(source_width);
                target_origin.y = reconcile_vertical_origin(
                    source_origin.y,
                    source_height,
                    target_height,
                    old_source_rect,
                    old_target_rect,
                );
            }
            Edge::Top => {
                target_origin.x = reconcile_horizontal_origin(
                    source_origin.x,
                    source_width,
                    target_width,
                    old_source_rect,
                    old_target_rect,
                );
                target_origin.y = source_origin.y.saturating_sub(target_height);
            }
            Edge::Bottom => {
                target_origin.x = reconcile_horizontal_origin(
                    source_origin.x,
                    source_width,
                    target_width,
                    old_source_rect,
                    old_target_rect,
                );
                target_origin.y = source_origin.y.saturating_add(source_height);
            }
        }

        self.screens[target_index].origin = Some(target_origin);
    }

    fn screen(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|screen| screen.name == name)
    }
}

fn reconcile_vertical_origin(
    source_top: i32,
    source_height: i32,
    target_height: i32,
    old_source: ScreenRect,
    old_target: ScreenRect,
) -> i32 {
    if near(old_target.bottom, old_source.bottom) {
        return source_top
            .saturating_add(source_height)
            .saturating_sub(target_height);
    }
    if near(old_target.top, old_source.top) {
        return source_top;
    }

    source_top.saturating_add(old_target.top.saturating_sub(old_source.top))
}

fn reconcile_horizontal_origin(
    source_left: i32,
    source_width: i32,
    target_width: i32,
    old_source: ScreenRect,
    old_target: ScreenRect,
) -> i32 {
    if near(old_target.right, old_source.right) {
        return source_left
            .saturating_add(source_width)
            .saturating_sub(target_width);
    }
    if near(old_target.left, old_source.left) {
        return source_left;
    }

    source_left.saturating_add(old_target.left.saturating_sub(old_source.left))
}

fn near(left: i32, right: i32) -> bool {
    left.abs_diff(right) <= 1
}

fn size_to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn positioned_transition(
    source: &Screen,
    target: &Screen,
    edge: Edge,
    x: u32,
    y: u32,
) -> Option<(u32, u32)> {
    let source_rect = ScreenRect::from_screen(source)?;
    let target_rect = ScreenRect::from_screen(target)?;

    match edge {
        Edge::Left | Edge::Right => {
            let overlap_start = source_rect.top.max(target_rect.top);
            let overlap_end = source_rect.bottom.min(target_rect.bottom);
            if overlap_start > overlap_end {
                return None;
            }

            let global_y = source_rect.top + y.min(source.size.height.saturating_sub(1)) as i32;
            if global_y < overlap_start || global_y > overlap_end {
                return None;
            }

            let target_y = (global_y - target_rect.top)
                .clamp(0, target.size.height.saturating_sub(1) as i32)
                as u32;
            let target_x = match edge {
                Edge::Left => target.size.width.saturating_sub(2),
                Edge::Right => 1,
                Edge::Top | Edge::Bottom => unreachable!(),
            };
            Some((target_x, target_y))
        }
        Edge::Top | Edge::Bottom => {
            let overlap_start = source_rect.left.max(target_rect.left);
            let overlap_end = source_rect.right.min(target_rect.right);
            if overlap_start > overlap_end {
                return None;
            }

            let global_x = source_rect.left + x.min(source.size.width.saturating_sub(1)) as i32;
            if global_x < overlap_start || global_x > overlap_end {
                return None;
            }

            let target_x = (global_x - target_rect.left)
                .clamp(0, target.size.width.saturating_sub(1) as i32)
                as u32;
            let target_y = match edge {
                Edge::Top => target.size.height.saturating_sub(2),
                Edge::Bottom => 1,
                Edge::Left | Edge::Right => unreachable!(),
            };
            Some((target_x, target_y))
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ScreenRect {
    fn from_screen(screen: &Screen) -> Option<Self> {
        let origin = screen.origin?;
        Some(Self {
            left: origin.x,
            top: origin.y,
            right: origin.x + screen.size.width.saturating_sub(1) as i32,
            bottom: origin.y + screen.size.height.saturating_sub(1) as i32,
        })
    }
}

fn scale(value: u32, from_max: u32, to_max: u32) -> u32 {
    if from_max <= 1 || to_max <= 1 {
        return 0;
    }
    let ratio = value.min(from_max - 1) as f64 / (from_max - 1) as f64;
    (ratio * (to_max - 1) as f64).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_screen_layout() -> Layout {
        Layout {
            screens: vec![
                Screen {
                    name: "windows".to_string(),
                    size: Size {
                        width: 1920,
                        height: 1080,
                    },
                    origin: None,
                    monitors: Vec::new(),
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
                    origin: None,
                    monitors: Vec::new(),
                },
            ],
            links: vec![Link {
                from: "windows".to_string(),
                edge: Edge::Right,
                to: "mac".to_string(),
            }],
        }
    }

    #[test]
    fn validates_known_screens() {
        two_screen_layout().validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_screens() {
        let mut layout = two_screen_layout();
        layout.screens.push(layout.screens[0].clone());
        assert_eq!(
            layout.validate(),
            Err(LayoutError::DuplicateScreen("windows".to_string()))
        );
    }

    /// A tall macOS desktop made of two stacked displays with a vertical gap
    /// between y=1000 and y=1200.
    fn mac_with_gap() -> Screen {
        Screen {
            name: "mac".to_string(),
            size: Size {
                width: 1728,
                height: 2000,
            },
            origin: None,
            monitors: vec![
                Monitor {
                    origin: Point { x: 0, y: 0 },
                    size: Size {
                        width: 1728,
                        height: 1000,
                    },
                },
                Monitor {
                    origin: Point { x: 0, y: 1200 },
                    size: Size {
                        width: 1728,
                        height: 800,
                    },
                },
            ],
        }
    }

    #[test]
    fn clamp_keeps_points_already_on_a_monitor() {
        let mac = mac_with_gap();
        assert_eq!(mac.clamp_point_to_monitor(100, 500), (100, 500));
        assert_eq!(mac.clamp_point_to_monitor(100, 1500), (100, 1500));
    }

    #[test]
    fn clamp_snaps_gap_points_onto_the_nearest_monitor() {
        let mac = mac_with_gap();
        // Just below the top monitor -> snaps up to its last row.
        assert_eq!(mac.clamp_point_to_monitor(100, 1050).1, 999);
        // Just above the bottom monitor -> snaps down to its first row.
        assert_eq!(mac.clamp_point_to_monitor(100, 1150).1, 1200);
    }

    #[test]
    fn single_display_screen_leaves_in_bounds_points_untouched() {
        let screen = Screen {
            name: "w".to_string(),
            size: Size {
                width: 100,
                height: 100,
            },
            origin: None,
            monitors: Vec::new(),
        };
        assert_eq!(screen.clamp_point_to_monitor(40, 90), (40, 90));
    }

    #[test]
    fn transition_into_multi_monitor_target_never_lands_in_a_gap() {
        let layout = Layout {
            screens: vec![
                Screen {
                    name: "windows".to_string(),
                    size: Size {
                        width: 1920,
                        height: 1080,
                    },
                    origin: None,
                    monitors: Vec::new(),
                },
                mac_with_gap(),
            ],
            links: vec![Link {
                from: "windows".to_string(),
                edge: Edge::Right,
                to: "mac".to_string(),
            }],
        };

        let mac = mac_with_gap();
        for source_y in [0u32, 270, 540, 700, 810, 900, 1079] {
            let transition = layout
                .transition("windows", Edge::Right, 1919, source_y)
                .unwrap();
            let on_monitor = mac
                .monitor_rects()
                .iter()
                .any(|rect| rect.contains(transition.x, transition.y));
            assert!(
                on_monitor,
                "entry ({}, {}) for source_y={source_y} fell in a gap",
                transition.x, transition.y
            );
        }
    }

    #[test]
    fn rejects_monitor_outside_the_desktop() {
        let mut layout = two_screen_layout();
        layout.screens[1].monitors = vec![Monitor {
            origin: Point { x: 0, y: 0 },
            size: Size {
                width: 9999,
                height: 10,
            },
        }];
        assert!(matches!(
            layout.validate(),
            Err(LayoutError::InvalidMonitor { .. })
        ));
    }

    #[test]
    fn maps_right_edge_to_left_edge_of_target() {
        let transition = two_screen_layout()
            .transition("windows", Edge::Right, 1919, 540)
            .unwrap();
        assert_eq!(transition.target_screen, "mac");
        assert_eq!(transition.target_edge, Edge::Left);
        assert_eq!(transition.x, 1);
        assert!(transition.y > 550 && transition.y < 570);
    }

    #[test]
    fn positioned_screens_only_route_through_overlapping_edge_segment() {
        let layout = Layout {
            screens: vec![
                Screen {
                    name: "windows".to_string(),
                    size: Size {
                        width: 1920,
                        height: 1080,
                    },
                    origin: Some(Point { x: 0, y: 0 }),
                    monitors: Vec::new(),
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 900,
                    },
                    origin: Some(Point { x: 1920, y: 180 }),
                    monitors: Vec::new(),
                },
            ],
            links: vec![Link {
                from: "windows".to_string(),
                edge: Edge::Right,
                to: "mac".to_string(),
            }],
        };

        assert!(
            layout
                .transition("windows", Edge::Right, 1919, 100)
                .is_none()
        );
        let transition = layout
            .transition("windows", Edge::Right, 1919, 540)
            .unwrap();
        assert_eq!(transition.target_screen, "mac");
        assert_eq!(transition.x, 1);
        assert_eq!(transition.y, 360);
    }

    #[test]
    fn screen_size_update_preserves_bottom_aligned_side_portal() {
        let mut layout = Layout {
            screens: vec![
                Screen {
                    name: "windows".to_string(),
                    size: Size {
                        width: 1920,
                        height: 1080,
                    },
                    origin: Some(Point { x: 0, y: 0 }),
                    monitors: Vec::new(),
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
                    origin: Some(Point { x: -1728, y: -37 }),
                    monitors: Vec::new(),
                },
            ],
            links: vec![Link {
                from: "windows".to_string(),
                edge: Edge::Left,
                to: "mac".to_string(),
            }],
        };

        assert!(layout.transition("windows", Edge::Left, 0, 1079).is_some());

        assert!(layout.set_screen_size_preserving_links(
            "mac",
            Size {
                width: 1512,
                height: 982,
            },
        ));

        let mac = layout.screen("mac").unwrap();
        assert_eq!(mac.origin, Some(Point { x: -1512, y: 98 }));
        let transition = layout.transition("windows", Edge::Left, 0, 1079).unwrap();
        assert_eq!(transition.target_screen, "mac");
        assert_eq!(transition.x, 1510);
        assert_eq!(transition.y, 981);
    }

    #[test]
    fn screen_size_update_preserves_top_aligned_side_portal() {
        let mut layout = Layout {
            screens: vec![
                Screen {
                    name: "windows".to_string(),
                    size: Size {
                        width: 1920,
                        height: 1080,
                    },
                    origin: Some(Point { x: 0, y: 0 }),
                    monitors: Vec::new(),
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
                    origin: Some(Point { x: 1920, y: 0 }),
                    monitors: Vec::new(),
                },
            ],
            links: vec![Link {
                from: "windows".to_string(),
                edge: Edge::Right,
                to: "mac".to_string(),
            }],
        };

        assert!(layout.set_screen_size_preserving_links(
            "mac",
            Size {
                width: 1512,
                height: 982,
            },
        ));

        let mac = layout.screen("mac").unwrap();
        assert_eq!(mac.origin, Some(Point { x: 1920, y: 0 }));
        assert!(layout.transition("windows", Edge::Right, 1919, 0).is_some());
        assert!(
            layout
                .transition("windows", Edge::Right, 1919, 1079)
                .is_none()
        );
    }
}
