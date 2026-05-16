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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub size: Size,
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

        Some(Transition {
            target_screen: link.to.clone(),
            target_edge: edge.opposite(),
            x: mapped_x,
            y: mapped_y,
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
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
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
}
