use anyhow::{Result, anyhow};
use async_trait::async_trait;
use deskbridge_core::{Button, InputEvent, InputPacket, KeyState};
use tracing::{info, warn};

#[async_trait]
pub trait InputSink: Send {
    async fn apply(&mut self, packet: &InputPacket) -> Result<()>;
}

pub struct LogSink;

#[async_trait]
impl InputSink for LogSink {
    async fn apply(&mut self, packet: &InputPacket) -> Result<()> {
        info!(seq = packet.seq, event = ?packet.event, "dry-run input event");
        Ok(())
    }
}

pub struct EnigoSink {
    enigo: enigo::Enigo,
}

impl EnigoSink {
    pub fn new() -> Result<Self> {
        let settings = enigo::Settings::default();
        let enigo = enigo::Enigo::new(&settings)
            .map_err(|err| anyhow!("failed to initialize input injection: {err}"))?;
        Ok(Self { enigo })
    }
}

#[async_trait]
impl InputSink for EnigoSink {
    async fn apply(&mut self, packet: &InputPacket) -> Result<()> {
        use enigo::{Axis, Coordinate, Keyboard, Mouse};

        match &packet.event {
            InputEvent::MouseMove { dx, dy } => self
                .enigo
                .move_mouse(*dx, *dy, Coordinate::Rel)
                .map_err(|err| anyhow!("mouse move failed: {err}"))?,
            InputEvent::MouseAbs { x, y } => self
                .enigo
                .move_mouse(*x, *y, Coordinate::Abs)
                .map_err(|err| anyhow!("mouse move failed: {err}"))?,
            InputEvent::MouseButton { button, state } => self
                .enigo
                .button(map_button(*button), map_direction(*state))
                .map_err(|err| anyhow!("mouse button failed: {err}"))?,
            InputEvent::Wheel { dx, dy } => {
                if *dx != 0 {
                    self.enigo
                        .scroll(*dx, Axis::Horizontal)
                        .map_err(|err| anyhow!("horizontal scroll failed: {err}"))?;
                }
                if *dy != 0 {
                    self.enigo
                        .scroll(*dy, Axis::Vertical)
                        .map_err(|err| anyhow!("vertical scroll failed: {err}"))?;
                }
            }
            InputEvent::Key { key, state } => {
                let Some(mapped) = map_key(key) else {
                    warn!(key, "unsupported key; ignoring");
                    return Ok(());
                };
                self.enigo
                    .key(mapped, map_direction(*state))
                    .map_err(|err| anyhow!("key event failed: {err}"))?;
            }
            InputEvent::Text { text } => self
                .enigo
                .text(text)
                .map_err(|err| anyhow!("text input failed: {err}"))?,
        }

        Ok(())
    }
}

fn map_button(button: Button) -> enigo::Button {
    match button {
        Button::Left => enigo::Button::Left,
        Button::Right => enigo::Button::Right,
        Button::Middle => enigo::Button::Middle,
        Button::Back => enigo::Button::Back,
        Button::Forward => enigo::Button::Forward,
    }
}

fn map_direction(state: KeyState) -> enigo::Direction {
    match state {
        KeyState::Pressed => enigo::Direction::Press,
        KeyState::Released => enigo::Direction::Release,
        KeyState::Clicked => enigo::Direction::Click,
    }
}

fn map_key(key: &str) -> Option<enigo::Key> {
    let key = key.trim();
    if key.chars().count() == 1 {
        return key.chars().next().map(enigo::Key::Unicode);
    }

    Some(match key.to_ascii_lowercase().as_str() {
        "alt" | "option" => enigo::Key::Alt,
        "backspace" => enigo::Key::Backspace,
        "capslock" => enigo::Key::CapsLock,
        "control" | "ctrl" => enigo::Key::Control,
        "delete" => enigo::Key::Delete,
        "down" | "downarrow" => enigo::Key::DownArrow,
        "end" => enigo::Key::End,
        "escape" | "esc" => enigo::Key::Escape,
        "f1" => enigo::Key::F1,
        "f2" => enigo::Key::F2,
        "f3" => enigo::Key::F3,
        "f4" => enigo::Key::F4,
        "f5" => enigo::Key::F5,
        "f6" => enigo::Key::F6,
        "f7" => enigo::Key::F7,
        "f8" => enigo::Key::F8,
        "f9" => enigo::Key::F9,
        "f10" => enigo::Key::F10,
        "f11" => enigo::Key::F11,
        "f12" => enigo::Key::F12,
        "home" => enigo::Key::Home,
        "left" | "leftarrow" => enigo::Key::LeftArrow,
        "meta" | "command" | "cmd" => enigo::Key::Meta,
        "return" | "enter" => enigo::Key::Return,
        "right" | "rightarrow" => enigo::Key::RightArrow,
        "shift" => enigo::Key::Shift,
        "space" => enigo::Key::Space,
        "tab" => enigo::Key::Tab,
        "up" | "uparrow" => enigo::Key::UpArrow,
        _ => return None,
    })
}
