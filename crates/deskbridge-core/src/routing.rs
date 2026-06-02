use crate::{Edge, InputEvent, Layout, LayoutError};

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedInput {
    pub target_screen: String,
    pub event: InputEvent,
    pub portal: Option<PortalTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalTransition {
    pub source_screen: String,
    pub source_edge: Edge,
    pub source_x: u32,
    pub source_y: u32,
    pub target_screen: String,
    pub target_edge: Edge,
    pub target_x: u32,
    pub target_y: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteOutcome {
    pub input: Option<RoutedInput>,
    pub portal: Option<PortalTransition>,
}

#[derive(Debug, Clone)]
pub struct InputRouter {
    layout: Layout,
    local_screen: String,
    active_screen: String,
    edge_threshold: u32,
    switch_delay_ms: u128,
    corner_size: u32,
    pending_edge: Option<PendingEdge>,
    remote_pointer: Option<RemotePointer>,
}

#[derive(Debug, Clone)]
struct RemotePointer {
    screen: String,
    return_edge: Edge,
    local_edge: Edge,
    x: i32,
    y: i32,
}

/// Tracks a linked edge the local pointer is currently resting against while a
/// configured switch delay has not yet elapsed.
#[derive(Debug, Clone)]
struct PendingEdge {
    edge: Edge,
    since_ms: u128,
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
            switch_delay_ms: 0,
            corner_size: 0,
            pending_edge: None,
            remote_pointer: None,
        })
    }

    pub fn active_screen(&self) -> &str {
        &self.active_screen
    }

    pub fn with_edge_threshold(mut self, threshold: u32) -> Self {
        self.edge_threshold = threshold;
        self
    }

    /// Require the pointer to rest against a linked edge for this many
    /// milliseconds before the screen switches. `0` keeps the original
    /// switch-on-contact behavior.
    pub fn with_switch_delay_ms(mut self, switch_delay_ms: u64) -> Self {
        self.switch_delay_ms = switch_delay_ms as u128;
        self
    }

    /// Suppress edge switching when the pointer is within this many pixels of a
    /// perpendicular edge (i.e. resting in a screen corner). `0` disables the
    /// corner dead zone.
    pub fn with_corner_size(mut self, corner_size: u32) -> Self {
        self.corner_size = corner_size;
        self
    }

    pub fn release_to_local(&mut self) {
        self.active_screen.clone_from(&self.local_screen);
        self.remote_pointer = None;
        self.pending_edge = None;
    }

    pub fn observe_local_pointer(&mut self, x: u32, y: u32) -> Option<RoutedInput> {
        self.observe_local_pointer_outcome(x, y).input
    }

    pub fn observe_local_pointer_outcome(&mut self, x: u32, y: u32) -> RouteOutcome {
        self.observe_local_pointer_outcome_at(x, y, crate::now_ms())
    }

    pub fn observe_local_pointer_outcome_at(
        &mut self,
        x: u32,
        y: u32,
        now_ms: u128,
    ) -> RouteOutcome {
        if self.active_screen != self.local_screen {
            return RouteOutcome::default();
        }

        let Some(edge) = self.edge_for_pointer(x, y) else {
            self.pending_edge = None;
            return RouteOutcome::default();
        };
        let Some(transition) = self.layout.transition(&self.local_screen, edge, x, y) else {
            self.pending_edge = None;
            return RouteOutcome::default();
        };

        if self.switch_delay_ms > 0 && !self.dwell_satisfied(edge, now_ms) {
            return RouteOutcome::default();
        }
        self.pending_edge = None;
        let (source_x, source_y) = self.edge_point(&self.local_screen, edge, x as i32, y as i32);
        let portal = PortalTransition {
            source_screen: self.local_screen.clone(),
            source_edge: edge,
            source_x,
            source_y,
            target_screen: transition.target_screen.clone(),
            target_edge: transition.target_edge,
            target_x: transition.x,
            target_y: transition.y,
        };
        self.active_screen.clone_from(&transition.target_screen);
        self.remote_pointer = Some(RemotePointer {
            screen: transition.target_screen.clone(),
            return_edge: transition.target_edge,
            local_edge: edge,
            x: transition.x as i32,
            y: transition.y as i32,
        });

        RouteOutcome {
            input: Some(RoutedInput {
                target_screen: transition.target_screen,
                event: InputEvent::MouseAbs {
                    x: transition.x as i32,
                    y: transition.y as i32,
                },
                portal: Some(portal.clone()),
            }),
            portal: Some(portal),
        }
    }

    pub fn route_if_remote_active(&mut self, event: InputEvent) -> Option<RoutedInput> {
        self.route_if_remote_active_outcome(event).input
    }

    pub fn route_if_remote_active_outcome(&mut self, event: InputEvent) -> RouteOutcome {
        if self.active_screen == self.local_screen {
            return RouteOutcome::default();
        }

        if self.remote_event_returns_to_local(&event) {
            let portal = self.return_portal_transition(&event);
            self.release_to_local();
            return RouteOutcome {
                input: None,
                portal,
            };
        }

        self.update_remote_pointer(&event);

        RouteOutcome {
            input: Some(RoutedInput {
                target_screen: self.active_screen.clone(),
                event,
                portal: None,
            }),
            portal: None,
        }
    }

    fn return_portal_transition(&self, event: &InputEvent) -> Option<PortalTransition> {
        let pointer = self.remote_pointer.as_ref()?;
        let InputEvent::MouseMove { dx, dy } = event else {
            return None;
        };

        let next_x = pointer.x.saturating_add(*dx);
        let next_y = pointer.y.saturating_add(*dy);
        let (source_x, source_y) =
            self.edge_point(&pointer.screen, pointer.return_edge, next_x, next_y);
        let (target_x, target_y) = self.map_point_between_screens(
            &pointer.screen,
            &self.local_screen,
            pointer.return_edge,
            pointer.local_edge,
            source_x,
            source_y,
        )?;

        Some(PortalTransition {
            source_screen: pointer.screen.clone(),
            source_edge: pointer.return_edge,
            source_x,
            source_y,
            target_screen: self.local_screen.clone(),
            target_edge: pointer.local_edge,
            target_x,
            target_y,
        })
    }

    fn edge_point(&self, screen_name: &str, edge: Edge, x: i32, y: i32) -> (u32, u32) {
        let Some((width, height)) = self.screen_size(screen_name) else {
            return (0, 0);
        };
        let max_x = width.saturating_sub(1);
        let max_y = height.saturating_sub(1);
        let clamped_x = x.clamp(0, max_x) as u32;
        let clamped_y = y.clamp(0, max_y) as u32;

        match edge {
            Edge::Left => (0, clamped_y),
            Edge::Right => (max_x as u32, clamped_y),
            Edge::Top => (clamped_x, 0),
            Edge::Bottom => (clamped_x, max_y as u32),
        }
    }

    fn map_point_between_screens(
        &self,
        from: &str,
        to: &str,
        from_edge: Edge,
        to_edge: Edge,
        x: u32,
        y: u32,
    ) -> Option<(u32, u32)> {
        let source = self
            .layout
            .screens
            .iter()
            .find(|screen| screen.name == from)?;
        let target = self
            .layout
            .screens
            .iter()
            .find(|screen| screen.name == to)?;

        if let (Some(source_origin), Some(target_origin)) = (source.origin, target.origin) {
            return match from_edge {
                Edge::Left | Edge::Right => {
                    let global_y = source_origin.y.saturating_add(y as i32);
                    let target_y = global_y
                        .saturating_sub(target_origin.y)
                        .clamp(0, target.size.height.saturating_sub(1) as i32)
                        as u32;
                    Some((edge_axis_coordinate(to_edge, target.size.width), target_y))
                }
                Edge::Top | Edge::Bottom => {
                    let global_x = source_origin.x.saturating_add(x as i32);
                    let target_x = global_x
                        .saturating_sub(target_origin.x)
                        .clamp(0, target.size.width.saturating_sub(1) as i32)
                        as u32;
                    Some((target_x, edge_axis_coordinate(to_edge, target.size.height)))
                }
            };
        }

        match from_edge {
            Edge::Left | Edge::Right => Some((
                edge_axis_coordinate(to_edge, target.size.width),
                scale_axis(y, source.size.height, target.size.height),
            )),
            Edge::Top | Edge::Bottom => Some((
                scale_axis(x, source.size.width, target.size.width),
                edge_axis_coordinate(to_edge, target.size.height),
            )),
        }
    }

    fn remote_event_returns_to_local(&self, event: &InputEvent) -> bool {
        let Some(pointer) = &self.remote_pointer else {
            return false;
        };
        let InputEvent::MouseMove { dx, dy } = event else {
            return false;
        };

        let next_x = pointer.x.saturating_add(*dx);
        let next_y = pointer.y.saturating_add(*dy);
        let threshold = self.edge_threshold as i32;

        match pointer.return_edge {
            Edge::Left => next_x <= threshold && *dx < 0,
            Edge::Right => {
                self.screen_size(&pointer.screen)
                    .is_some_and(|(width, _)| next_x >= width - 1 - threshold)
                    && *dx > 0
            }
            Edge::Top => next_y <= threshold && *dy < 0,
            Edge::Bottom => {
                self.screen_size(&pointer.screen)
                    .is_some_and(|(_, height)| next_y >= height - 1 - threshold)
                    && *dy > 0
            }
        }
    }

    fn update_remote_pointer(&mut self, event: &InputEvent) {
        let Some(screen_name) = self
            .remote_pointer
            .as_ref()
            .map(|pointer| pointer.screen.clone())
        else {
            return;
        };

        let Some((width, height)) = self.screen_size(&screen_name) else {
            return;
        };

        let Some(pointer) = &mut self.remote_pointer else {
            return;
        };

        match event {
            InputEvent::MouseMove { dx, dy } => {
                pointer.x = (pointer.x + *dx).clamp(0, width - 1);
                pointer.y = (pointer.y + *dy).clamp(0, height - 1);
            }
            InputEvent::MouseAbs { x, y } => {
                pointer.x = (*x).clamp(0, width - 1);
                pointer.y = (*y).clamp(0, height - 1);
            }
            InputEvent::MouseButton { .. }
            | InputEvent::Wheel { .. }
            | InputEvent::Key { .. }
            | InputEvent::Text { .. } => {}
        }
    }

    fn screen_size(&self, screen_name: &str) -> Option<(i32, i32)> {
        self.layout
            .screens
            .iter()
            .find(|screen| screen.name == screen_name)
            .map(|screen| (screen.size.width as i32, screen.size.height as i32))
    }

    /// Returns true once the pointer has rested against `edge` for at least the
    /// configured switch delay. The first contact arms a timer and reports
    /// `false`; later contacts on the same edge clear once the delay elapses.
    fn dwell_satisfied(&mut self, edge: Edge, now_ms: u128) -> bool {
        match &self.pending_edge {
            Some(pending) if pending.edge == edge => {
                now_ms.saturating_sub(pending.since_ms) >= self.switch_delay_ms
            }
            _ => {
                self.pending_edge = Some(PendingEdge {
                    edge,
                    since_ms: now_ms,
                });
                false
            }
        }
    }

    fn edge_for_pointer(&self, x: u32, y: u32) -> Option<Edge> {
        let screen = self
            .layout
            .screens
            .iter()
            .find(|screen| screen.name == self.local_screen)?;
        let max_x = screen.size.width.saturating_sub(1);
        let max_y = screen.size.height.saturating_sub(1);

        // A pointer parked in a corner is usually heading for a hot corner,
        // Start menu, or window control rather than the neighboring screen, so
        // suppress switching while it is within the configured corner zone.
        let in_horizontal_corner =
            x <= self.corner_size || max_x.saturating_sub(x) <= self.corner_size;
        let in_vertical_corner =
            y <= self.corner_size || max_y.saturating_sub(y) <= self.corner_size;

        if x <= self.edge_threshold {
            return (!in_vertical_corner).then_some(Edge::Left);
        }
        if max_x.saturating_sub(x) <= self.edge_threshold {
            return (!in_vertical_corner).then_some(Edge::Right);
        }
        if y <= self.edge_threshold {
            return (!in_horizontal_corner).then_some(Edge::Top);
        }
        if max_y.saturating_sub(y) <= self.edge_threshold {
            return (!in_horizontal_corner).then_some(Edge::Bottom);
        }

        None
    }
}

fn edge_axis_coordinate(edge: Edge, size: u32) -> u32 {
    match edge {
        Edge::Left | Edge::Top => 0,
        Edge::Right | Edge::Bottom => size.saturating_sub(1),
    }
}

fn scale_axis(value: u32, from_max: u32, to_max: u32) -> u32 {
    if from_max <= 1 || to_max <= 1 {
        return 0;
    }
    let ratio = value.min(from_max - 1) as f64 / (from_max - 1) as f64;
    (ratio * (to_max - 1) as f64).round() as u32
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
        assert_eq!(
            routed.portal,
            Some(PortalTransition {
                source_screen: "windows".to_string(),
                source_edge: Edge::Right,
                source_x: 1919,
                source_y: 540,
                target_screen: "mac".to_string(),
                target_edge: Edge::Left,
                target_x: 1,
                target_y: 559,
            })
        );
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
    fn routes_relative_mouse_after_crossing_linked_edge() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        assert_eq!(
            router.observe_local_pointer(1919, 540).unwrap().event,
            InputEvent::MouseAbs { x: 1, y: 559 }
        );

        let routed = router
            .route_if_remote_active(InputEvent::MouseMove { dx: 50, dy: -3 })
            .unwrap();

        assert_eq!(routed.target_screen, "mac");
        assert_eq!(routed.event, InputEvent::MouseMove { dx: 50, dy: -3 });
    }

    #[test]
    fn moving_back_across_entry_edge_releases_to_local() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        router.observe_local_pointer(1919, 540).unwrap();

        assert_eq!(
            router.route_if_remote_active(InputEvent::MouseMove { dx: 50, dy: 0 }),
            Some(RoutedInput {
                target_screen: "mac".to_string(),
                event: InputEvent::MouseMove { dx: 50, dy: 0 },
                portal: None,
            })
        );
        assert_eq!(
            router.route_if_remote_active(InputEvent::MouseMove { dx: -60, dy: 0 }),
            None
        );
        assert_eq!(router.active_screen(), "windows");
        assert_eq!(
            router.route_if_remote_active(InputEvent::Key {
                key: "a".to_string(),
                state: crate::KeyState::Clicked,
            }),
            None
        );
    }

    #[test]
    fn return_to_local_reports_portal_transition() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        router.observe_local_pointer(1919, 540).unwrap();
        router
            .route_if_remote_active(InputEvent::MouseMove { dx: 50, dy: 0 })
            .unwrap();

        let outcome =
            router.route_if_remote_active_outcome(InputEvent::MouseMove { dx: -60, dy: 0 });

        assert_eq!(outcome.input, None);
        assert_eq!(
            outcome.portal,
            Some(PortalTransition {
                source_screen: "mac".to_string(),
                source_edge: Edge::Left,
                source_x: 0,
                source_y: 559,
                target_screen: "windows".to_string(),
                target_edge: Edge::Right,
                target_x: 1919,
                target_y: 540,
            })
        );
        assert_eq!(router.active_screen(), "windows");
    }

    #[test]
    fn moving_parallel_to_entry_edge_stays_remote() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        router.observe_local_pointer(1919, 540).unwrap();

        let routed = router
            .route_if_remote_active(InputEvent::MouseMove { dx: 0, dy: 25 })
            .unwrap();

        assert_eq!(router.active_screen(), "mac");
        assert_eq!(routed.target_screen, "mac");
        assert_eq!(routed.event, InputEvent::MouseMove { dx: 0, dy: 25 });
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

    #[test]
    fn switch_delay_requires_dwell_before_switching() {
        let mut router = InputRouter::new(layout(), "windows")
            .unwrap()
            .with_switch_delay_ms(200);

        // First contact arms the dwell timer but does not switch.
        assert_eq!(
            router
                .observe_local_pointer_outcome_at(1919, 540, 1_000)
                .input,
            None
        );
        assert_eq!(router.active_screen(), "windows");

        // Still within the delay window: no switch yet.
        assert_eq!(
            router
                .observe_local_pointer_outcome_at(1919, 545, 1_100)
                .input,
            None
        );
        assert_eq!(router.active_screen(), "windows");

        // Delay elapsed while resting on the edge: now it switches.
        let routed = router
            .observe_local_pointer_outcome_at(1919, 545, 1_250)
            .input
            .unwrap();
        assert_eq!(router.active_screen(), "mac");
        assert_eq!(routed.target_screen, "mac");
    }

    #[test]
    fn leaving_edge_before_dwell_cancels_the_switch() {
        let mut router = InputRouter::new(layout(), "windows")
            .unwrap()
            .with_switch_delay_ms(200);

        assert_eq!(
            router
                .observe_local_pointer_outcome_at(1919, 540, 1_000)
                .input,
            None
        );
        // Pointer moves back into the interior, cancelling the pending switch.
        assert_eq!(
            router
                .observe_local_pointer_outcome_at(900, 540, 1_100)
                .input,
            None
        );
        // Returning to the edge restarts the timer; an immediate read does not
        // switch even though wall time has advanced past the original arm.
        assert_eq!(
            router
                .observe_local_pointer_outcome_at(1919, 540, 1_300)
                .input,
            None
        );
        assert_eq!(router.active_screen(), "windows");
    }

    #[test]
    fn corner_dead_zone_suppresses_accidental_switch() {
        let mut router = InputRouter::new(layout(), "windows")
            .unwrap()
            .with_corner_size(40);

        // Top-right corner: on the right edge but also within the top corner
        // zone, so it should not switch.
        assert_eq!(router.observe_local_pointer(1919, 5), None);
        assert_eq!(router.active_screen(), "windows");

        // Mid-edge, clear of the corner: switches as usual.
        let routed = router.observe_local_pointer(1919, 540).unwrap();
        assert_eq!(routed.target_screen, "mac");
    }

    #[test]
    fn local_pointer_updates_do_not_release_remote_screen() {
        let mut router = InputRouter::new(layout(), "windows").unwrap();
        router.observe_local_pointer(1919, 540).unwrap();
        assert_eq!(router.active_screen(), "mac");

        assert_eq!(router.observe_local_pointer(1800, 540), None);
        assert_eq!(router.active_screen(), "mac");

        let routed = router
            .route_if_remote_active(InputEvent::Key {
                key: "a".to_string(),
                state: crate::KeyState::Clicked,
            })
            .unwrap();
        assert_eq!(routed.target_screen, "mac");
    }
}
