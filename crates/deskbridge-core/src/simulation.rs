use crate::{Edge, InputEvent, InputRouter, Layout, LayoutError};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedRouteEvent {
    pub index: usize,
    pub target_screen: String,
    pub event: InputEvent,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RouteSimulationError {
    #[error("layout error: {0}")]
    Layout(#[from] LayoutError),
    #[error("layout does not include screen '{0}'")]
    UnknownScreen(String),
    #[error("no linked transition from '{screen}' on edge {edge:?}")]
    MissingTransition { screen: String, edge: Edge },
    #[error("transition targeted '{actual}', expected '{expected}'")]
    UnexpectedTarget { expected: String, actual: String },
    #[error("remote screen stopped receiving routed input")]
    RemoteInactive,
}

pub fn simulate_route(
    layout: &Layout,
    local_screen: &str,
    expected_target: &str,
    edge: Edge,
    steps: usize,
    dx: i32,
    dy: i32,
) -> Result<Vec<SimulatedRouteEvent>, RouteSimulationError> {
    let (x, y) = edge_point(layout, local_screen, edge)?;
    let mut router = InputRouter::new(layout.clone(), local_screen.to_string())?;
    let first = router.observe_local_pointer(x, y).ok_or_else(|| {
        RouteSimulationError::MissingTransition {
            screen: local_screen.to_string(),
            edge,
        }
    })?;

    if first.target_screen != expected_target {
        return Err(RouteSimulationError::UnexpectedTarget {
            expected: expected_target.to_string(),
            actual: first.target_screen,
        });
    }

    let mut events = vec![SimulatedRouteEvent {
        index: 0,
        target_screen: expected_target.to_string(),
        event: first.event,
    }];

    for index in 1..=steps {
        let routed = router
            .route_if_remote_active(InputEvent::MouseMove { dx, dy })
            .ok_or(RouteSimulationError::RemoteInactive)?;
        if routed.target_screen != expected_target {
            return Err(RouteSimulationError::UnexpectedTarget {
                expected: expected_target.to_string(),
                actual: routed.target_screen,
            });
        }
        events.push(SimulatedRouteEvent {
            index,
            target_screen: expected_target.to_string(),
            event: routed.event,
        });
    }

    Ok(events)
}

fn edge_point(
    layout: &Layout,
    screen_name: &str,
    edge: Edge,
) -> Result<(u32, u32), RouteSimulationError> {
    let screen = layout
        .screens
        .iter()
        .find(|screen| screen.name == screen_name)
        .ok_or_else(|| RouteSimulationError::UnknownScreen(screen_name.to_string()))?;
    let max_x = screen.size.width.saturating_sub(1);
    let max_y = screen.size.height.saturating_sub(1);
    let mid_x = screen.size.width / 2;
    let mid_y = screen.size.height / 2;

    Ok(match edge {
        Edge::Left => (0, mid_y),
        Edge::Right => (max_x, mid_y),
        Edge::Top => (mid_x, 0),
        Edge::Bottom => (mid_x, max_y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Link, Screen, Size};

    fn layout() -> Layout {
        Layout {
            screens: vec![
                Screen {
                    name: "windows".to_string(),
                    size: Size {
                        width: 1920,
                        height: 1080,
                    },
                    origin: None,
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
                    origin: None,
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
    fn simulates_edge_crossing_and_continued_remote_mouse_motion() {
        let events = simulate_route(&layout(), "windows", "mac", Edge::Right, 3, 80, -2).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event, InputEvent::MouseAbs { x: 1, y: 559 });
        assert_eq!(events[1].event, InputEvent::MouseMove { dx: 80, dy: -2 });
        assert_eq!(events[3].event, InputEvent::MouseMove { dx: 80, dy: -2 });
    }

    #[test]
    fn rejects_unlinked_edges() {
        assert_eq!(
            simulate_route(&layout(), "windows", "mac", Edge::Left, 1, 10, 0),
            Err(RouteSimulationError::MissingTransition {
                screen: "windows".to_string(),
                edge: Edge::Left,
            })
        );
    }
}
