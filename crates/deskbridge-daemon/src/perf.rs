use deskbridge_core::InputEvent;

pub const PERF_WINDOW_MS: u128 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    MouseMove,
    MouseAbs,
    MouseButton,
    Wheel,
    Key,
    Text,
}

pub fn event_kind(event: &InputEvent) -> EventKind {
    match event {
        InputEvent::MouseMove { .. } => EventKind::MouseMove,
        InputEvent::MouseAbs { .. } => EventKind::MouseAbs,
        InputEvent::MouseButton { .. } => EventKind::MouseButton,
        InputEvent::Wheel { .. } => EventKind::Wheel,
        InputEvent::Key { .. } => EventKind::Key,
        InputEvent::Text { .. } => EventKind::Text,
    }
}

pub fn rate_per_second(count: usize, window_ms: u128) -> f64 {
    if window_ms == 0 {
        return 0.0;
    }

    count as f64 * 1_000.0 / window_ms as f64
}

pub fn percentile(values: &mut [u128], pct: u32) -> Option<u128> {
    if values.is_empty() {
        return None;
    }

    values.sort_unstable();
    let pct = pct.min(100) as usize;
    let index = ((values.len() - 1) * pct).div_ceil(100);
    values.get(index).copied()
}

pub fn format_us(value: Option<u128>) -> String {
    match value {
        Some(value) if value >= 1_000 => format!("{:.2}ms", value as f64 / 1_000.0),
        Some(value) => format!("{value}us"),
        None => "n/a".to_string(),
    }
}

pub fn format_ms(value: Option<u128>) -> String {
    value
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "n/a".to_string())
}
