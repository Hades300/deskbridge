use crate::{Edge, InputEvent, Layout, LayoutError};

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedInput {
    pub target_screen: String,
    pub event: InputEvent,
}

#[derive(Debug, Clone)]
pub struct InputRouter {
    layout: Layout,
    local_screen: String,
    active_screen: String,
    edge_threshold: u32,
}

impl InputRouter {
    pub fn new(layout: Layout, local_screen: impl Into<String>) -> Result<Self, LayoutError> {
        layout.validate()?;
        let local_screen = local_screen.into();
        if !layout
            .screens
            .iter()
            .any(|screen| screen.name == local_screen)
        {
            return Err(LayoutError::UnknownScreen(local_screen));
        }

        Ok(Self {
            active_screen: local_screen.clone(),
            local_screen,
            layout,
            edge_threshold: 2,
        })
    }

    pub fn active_screen(&self) -> &str {
        &self.active_screen
    }

    pub fn with_edge_threshold(mut self, threshold: u32) -> Self {
        self.edge_threshold = threshold;
        self
    }

    pub fn release_to_local(&mut self) {
        self.active_screen.clone_from(&self.local_screen);
    }

    pub fn observe_local_pointer(&mut self, x: u32, y: u32) -> Option<RoutedInput> {
        if self.active_screen != self.local_screen {
            return None;
        }

        let edge = self.edge_for_pointer(x, y)?;
        let transition = self.layout.transition(&self.local_screen, edge, x, y)?;
        self.active_screen.clone_from(&transition.target_screen);

        Some(RoutedInput {
            target_screen: transition.target_screen,
            event: InputEvent::MouseAbs {
                x: transition.x as i32,
                y: transition.y as i32,
            },
        })
    }

    pub fn route_if_remote_active(&self, event: InputEvent) -> Option<RoutedInput> {
        if self.active_screen == self.local_screen {
            return None;
        }

        Some(RoutedInput {
            target_screen: self.active_screen.clone(),
            event,
        })
    }

    fn edge_for_pointer(&self, x: u32, y: u32) -> Option<Edge> {
        let screen = self
            .layout
            .screens
            .iter()
            .find(|screen| screen.name == self.local_screen)?;
        let max_x = screen.size.width.saturating_sub(1);
        let max_y = screen.size.height.saturating_sub(1);

        if x <= self.edge_threshold {
            return Some(Edge::Left);
        }
        if max_x.saturating_sub(x) <= self.edge_threshold {
            return Some(Edge::Right);
        }
        if y <= self.edge_threshold {
            return Some(Edge::Top);
        }
        if max_y.saturating_sub(y) <= self.edge_threshold {
            return Some(Edge::Bottom);
        }

        None
    }
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
    fn stays_local_until_pointer_hits_a_linked_edge() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        assert_eq!(router.observe_local_pointer(960, 540), None);
        assert_eq!(router.active_screen(), "windows");
    }

    #[test]
    fn right_edge_switches_to_remote_screen() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        let routed = router.observe_local_pointer(1919, 540).unwrap();
        assert_eq!(router.active_screen(), "mac");
        assert_eq!(routed.target_screen, "mac");
        assert_eq!(routed.event, InputEvent::MouseAbs { x: 1, y: 559 });
    }

    #[test]
    fn routes_keyboard_while_remote_screen_is_active() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        router.observe_local_pointer(1919, 540).unwrap();

        let routed = router
            .route_if_remote_active(InputEvent::Key {
                key: "a".to_string(),
                state: crate::KeyState::Clicked,
            })
            .unwrap();
        assert_eq!(routed.target_screen, "mac");
    }

    #[test]
    fn release_returns_input_to_local_screen() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        router.observe_local_pointer(1919, 540).unwrap();
        router.release_to_local();
        assert_eq!(router.active_screen(), "windows");
        assert_eq!(
            router.route_if_remote_active(InputEvent::MouseMove { dx: 1, dy: 0 }),
            None
        );
    }
}
