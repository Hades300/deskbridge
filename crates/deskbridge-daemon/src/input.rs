use anyhow::{Result, anyhow};
use async_trait::async_trait;
use deskbridge_core::{Button, InputEvent, InputPacket, KeyState, Size};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub size: Size,
    pub location: Option<(i32, i32)>,
}

impl EnigoSink {
    pub fn new() -> Result<Self> {
        let settings = enigo::Settings::default();
        let enigo = enigo::Enigo::new(&settings)
            .map_err(|err| anyhow!("failed to initialize input injection: {err}"))?;
        Ok(Self { enigo })
    }

    pub fn move_mouse_rel_evented_for_diagnostics(&mut self, dx: i32, dy: i32) -> Result<()> {
        use enigo::{Coordinate, Mouse};

        self.enigo
            .move_mouse(dx, dy, Coordinate::Rel)
            .map_err(|err| anyhow!("mouse move failed: {err}"))
    }
}

pub fn display_info() -> Result<DisplayInfo> {
    use enigo::Mouse;

    let settings = enigo::Settings::default();
    let enigo = enigo::Enigo::new(&settings)
        .map_err(|err| anyhow!("failed to initialize input inspection: {err}"))?;
    let (width, height) = enigo
        .main_display()
        .map_err(|err| anyhow!("failed to read main display size: {err}"))?;
    let location = enigo.location().ok();

    if width <= 0 || height <= 0 {
        return Err(anyhow!(
            "main display returned invalid size {width}x{height}"
        ));
    }

    Ok(DisplayInfo {
        size: Size {
            width: width as u32,
            height: height as u32,
        },
        location,
    })
}

#[async_trait]
impl InputSink for EnigoSink {
    async fn apply(&mut self, packet: &InputPacket) -> Result<()> {
        use enigo::{Axis, Keyboard, Mouse};

        match &packet.event {
            InputEvent::MouseMove { dx, dy } => move_mouse_rel(&mut self.enigo, *dx, *dy)?,
            InputEvent::MouseAbs { x, y } => move_mouse_abs(&mut self.enigo, *x, *y)?,
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

fn move_mouse_rel(enigo: &mut enigo::Enigo, dx: i32, dy: i32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use enigo::Mouse;

        let (x, y) = enigo
            .location()
            .map_err(|err| anyhow!("failed to read mouse location: {err}"))?;
        macos_warp_mouse(x.saturating_add(dx), y.saturating_add(dy))
    }

    #[cfg(not(target_os = "macos"))]
    {
        use enigo::{Coordinate, Mouse};
        enigo
            .move_mouse(dx, dy, Coordinate::Rel)
            .map_err(|err| anyhow!("mouse move failed: {err}"))
    }
}

fn move_mouse_abs(enigo: &mut enigo::Enigo, x: i32, y: i32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = enigo;
        macos_warp_mouse(x, y)
    }

    #[cfg(not(target_os = "macos"))]
    {
        use enigo::{Coordinate, Mouse};
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|err| anyhow!("mouse move failed: {err}"))
    }
}

#[cfg(target_os = "macos")]
fn macos_warp_mouse(x: i32, y: i32) -> Result<()> {
    use core_graphics::display::CGDisplay;
    use core_graphics::geometry::CGPoint;

    let bounds = CGDisplay::main().bounds();
    let max_x = (bounds.size.width as i32).saturating_sub(1).max(0);
    let max_y = (bounds.size.height as i32).saturating_sub(1).max(0);
    let point = CGPoint::new(
        bounds.origin.x + x.clamp(0, max_x) as f64,
        bounds.origin.y + y.clamp(0, max_y) as f64,
    );
    CGDisplay::warp_mouse_cursor_position(point)
        .map_err(|err| anyhow!("absolute mouse warp failed: {err:?}"))
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
        "capslock" | "caps_lock" => enigo::Key::CapsLock,
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
        "f13" => enigo::Key::F13,
        "f14" => enigo::Key::F14,
        "f15" => enigo::Key::F15,
        "f16" => enigo::Key::F16,
        "f17" => enigo::Key::F17,
        "f18" => enigo::Key::F18,
        "f19" => enigo::Key::F19,
        "f20" => enigo::Key::F20,
        "home" => enigo::Key::Home,
        "insert" | "ins" => map_insert_key()?,
        "left" | "leftarrow" => enigo::Key::LeftArrow,
        "left_shift" | "lshift" => enigo::Key::LShift,
        "right_shift" | "rshift" => enigo::Key::RShift,
        "left_control" | "lcontrol" | "left_ctrl" | "lctrl" => enigo::Key::LControl,
        "right_control" | "rcontrol" | "right_ctrl" | "rctrl" => enigo::Key::RControl,
        "meta" | "command" | "cmd" | "win" | "windows" | "super" => enigo::Key::Meta,
        "left_meta" | "left_command" | "left_cmd" | "left_win" | "left_windows" => enigo::Key::Meta,
        "right_meta" | "right_command" | "right_cmd" | "right_win" | "right_windows" => {
            right_meta_key()
        }
        "minus" => physical_minus_key(),
        "equal" => physical_equal_key(),
        "comma" => physical_comma_key(),
        "period" => physical_period_key(),
        "slash" => physical_slash_key(),
        "semicolon" => physical_semicolon_key(),
        "quote" => physical_quote_key(),
        "left_bracket" => physical_left_bracket_key(),
        "right_bracket" => physical_right_bracket_key(),
        "backslash" | "intl_backslash" => physical_backslash_key(),
        "grave" | "backquote" => physical_grave_key(),
        "numpad0" | "kp0" => enigo::Key::Numpad0,
        "numpad1" | "kp1" => enigo::Key::Numpad1,
        "numpad2" | "kp2" => enigo::Key::Numpad2,
        "numpad3" | "kp3" => enigo::Key::Numpad3,
        "numpad4" | "kp4" => enigo::Key::Numpad4,
        "numpad5" | "kp5" => enigo::Key::Numpad5,
        "numpad6" | "kp6" => enigo::Key::Numpad6,
        "numpad7" | "kp7" => enigo::Key::Numpad7,
        "numpad8" | "kp8" => enigo::Key::Numpad8,
        "numpad9" | "kp9" => enigo::Key::Numpad9,
        "numpad_add" | "kp_add" => enigo::Key::Add,
        "numpad_decimal" | "kp_decimal" => enigo::Key::Decimal,
        "numpad_divide" | "kp_divide" => enigo::Key::Divide,
        "numpad_multiply" | "kp_multiply" => enigo::Key::Multiply,
        "numpad_subtract" | "kp_subtract" => enigo::Key::Subtract,
        "pagedown" | "page_down" => enigo::Key::PageDown,
        "pageup" | "page_up" => enigo::Key::PageUp,
        "pause" => map_pause_key()?,
        "printscreen" | "print_screen" | "snapshot" => map_print_screen_key()?,
        "return" | "enter" => enigo::Key::Return,
        "right" | "rightarrow" => enigo::Key::RightArrow,
        "shift" => enigo::Key::Shift,
        "space" => enigo::Key::Space,
        "tab" => enigo::Key::Tab,
        "up" | "uparrow" => enigo::Key::UpArrow,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn physical_minus_key() -> enigo::Key {
    enigo::Key::Other(27)
}

#[cfg(target_os = "macos")]
fn physical_equal_key() -> enigo::Key {
    enigo::Key::Other(24)
}

#[cfg(target_os = "macos")]
fn physical_comma_key() -> enigo::Key {
    enigo::Key::Other(43)
}

#[cfg(target_os = "macos")]
fn physical_period_key() -> enigo::Key {
    enigo::Key::Other(47)
}

#[cfg(target_os = "macos")]
fn physical_slash_key() -> enigo::Key {
    enigo::Key::Other(44)
}

#[cfg(target_os = "macos")]
fn physical_semicolon_key() -> enigo::Key {
    enigo::Key::Other(41)
}

#[cfg(target_os = "macos")]
fn physical_quote_key() -> enigo::Key {
    enigo::Key::Other(39)
}

#[cfg(target_os = "macos")]
fn physical_left_bracket_key() -> enigo::Key {
    enigo::Key::Other(33)
}

#[cfg(target_os = "macos")]
fn physical_right_bracket_key() -> enigo::Key {
    enigo::Key::Other(30)
}

#[cfg(target_os = "macos")]
fn physical_backslash_key() -> enigo::Key {
    enigo::Key::Other(42)
}

#[cfg(target_os = "macos")]
fn physical_grave_key() -> enigo::Key {
    enigo::Key::Other(50)
}

#[cfg(target_os = "macos")]
fn right_meta_key() -> enigo::Key {
    enigo::Key::RCommand
}

#[cfg(target_os = "macos")]
fn map_insert_key() -> Option<enigo::Key> {
    None
}

#[cfg(target_os = "macos")]
fn map_pause_key() -> Option<enigo::Key> {
    None
}

#[cfg(target_os = "macos")]
fn map_print_screen_key() -> Option<enigo::Key> {
    None
}

#[cfg(target_os = "windows")]
fn physical_minus_key() -> enigo::Key {
    enigo::Key::OEMMinus
}

#[cfg(target_os = "windows")]
fn physical_equal_key() -> enigo::Key {
    enigo::Key::OEMPlus
}

#[cfg(target_os = "windows")]
fn physical_comma_key() -> enigo::Key {
    enigo::Key::OEMComma
}

#[cfg(target_os = "windows")]
fn physical_period_key() -> enigo::Key {
    enigo::Key::OEMPeriod
}

#[cfg(target_os = "windows")]
fn physical_slash_key() -> enigo::Key {
    enigo::Key::OEM2
}

#[cfg(target_os = "windows")]
fn physical_semicolon_key() -> enigo::Key {
    enigo::Key::OEM1
}

#[cfg(target_os = "windows")]
fn physical_quote_key() -> enigo::Key {
    enigo::Key::OEM7
}

#[cfg(target_os = "windows")]
fn physical_left_bracket_key() -> enigo::Key {
    enigo::Key::OEM4
}

#[cfg(target_os = "windows")]
fn physical_right_bracket_key() -> enigo::Key {
    enigo::Key::OEM6
}

#[cfg(target_os = "windows")]
fn physical_backslash_key() -> enigo::Key {
    enigo::Key::OEM5
}

#[cfg(target_os = "windows")]
fn physical_grave_key() -> enigo::Key {
    enigo::Key::OEM3
}

#[cfg(target_os = "windows")]
fn right_meta_key() -> enigo::Key {
    enigo::Key::RWin
}

#[cfg(target_os = "windows")]
fn map_insert_key() -> Option<enigo::Key> {
    Some(enigo::Key::Insert)
}

#[cfg(target_os = "windows")]
fn map_pause_key() -> Option<enigo::Key> {
    Some(enigo::Key::Pause)
}

#[cfg(target_os = "windows")]
fn map_print_screen_key() -> Option<enigo::Key> {
    Some(enigo::Key::PrintScr)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_minus_key() -> enigo::Key {
    enigo::Key::Unicode('-')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_equal_key() -> enigo::Key {
    enigo::Key::Unicode('=')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_comma_key() -> enigo::Key {
    enigo::Key::Unicode(',')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_period_key() -> enigo::Key {
    enigo::Key::Unicode('.')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_slash_key() -> enigo::Key {
    enigo::Key::Unicode('/')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_semicolon_key() -> enigo::Key {
    enigo::Key::Unicode(';')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_quote_key() -> enigo::Key {
    enigo::Key::Unicode('\'')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_left_bracket_key() -> enigo::Key {
    enigo::Key::Unicode('[')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_right_bracket_key() -> enigo::Key {
    enigo::Key::Unicode(']')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_backslash_key() -> enigo::Key {
    enigo::Key::Unicode('\\')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_grave_key() -> enigo::Key {
    enigo::Key::Unicode('`')
}

#[cfg(all(unix, not(target_os = "macos")))]
fn right_meta_key() -> enigo::Key {
    enigo::Key::Meta
}

#[cfg(all(unix, not(target_os = "macos")))]
fn map_insert_key() -> Option<enigo::Key> {
    Some(enigo::Key::Insert)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn map_pause_key() -> Option<enigo::Key> {
    Some(enigo::Key::Pause)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn map_print_screen_key() -> Option<enigo::Key> {
    Some(enigo::Key::PrintScr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_modifier_aliases() {
        assert_eq!(map_key("shift"), Some(enigo::Key::Shift));
        assert_eq!(map_key("left_shift"), Some(enigo::Key::LShift));
        assert_eq!(map_key("right_shift"), Some(enigo::Key::RShift));
        assert_eq!(map_key("meta"), Some(enigo::Key::Meta));
        assert_eq!(map_key("win"), Some(enigo::Key::Meta));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn maps_symbol_keys_to_macos_physical_keycodes() {
        assert_eq!(map_key("equal"), Some(enigo::Key::Other(24)));
        assert_eq!(map_key("minus"), Some(enigo::Key::Other(27)));
        assert_eq!(map_key("slash"), Some(enigo::Key::Other(44)));
    }
}
