use anyhow::{Result, anyhow};
use async_trait::async_trait;
use deskbridge_core::{Button, InputEvent, InputPacket, KeyState, Size};
use std::collections::HashSet;
use tracing::{info, warn};

#[async_trait]
pub trait InputSink: Send {
    async fn apply(&mut self, packet: &InputPacket) -> Result<()>;

    async fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
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
    cursor_pos: Option<(i32, i32)>,
    pressed_buttons: PressedButtons,
    pressed_keys: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub size: Size,
    pub location: Option<(i32, i32)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PressedButtons {
    left: bool,
    right: bool,
}

impl EnigoSink {
    pub fn new() -> Result<Self> {
        let settings = enigo::Settings::default();
        let enigo = enigo::Enigo::new(&settings)
            .map_err(|err| anyhow!("failed to initialize input injection: {err}"))?;
        Ok(Self {
            enigo,
            cursor_pos: None,
            pressed_buttons: PressedButtons::default(),
            pressed_keys: HashSet::new(),
        })
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

        macos_declare_user_activity();

        match &packet.event {
            InputEvent::MouseMove { dx, dy } => move_mouse_rel(
                &mut self.enigo,
                &mut self.cursor_pos,
                *dx,
                *dy,
                self.pressed_buttons.has_primary_drag(),
            )?,
            InputEvent::MouseAbs { x, y } => move_mouse_abs(
                &mut self.enigo,
                &mut self.cursor_pos,
                *x,
                *y,
                self.pressed_buttons.has_primary_drag(),
            )?,
            InputEvent::MouseButton { button, state } => {
                self.enigo
                    .button(map_button(*button), map_direction(*state))
                    .map_err(|err| anyhow!("mouse button failed: {err}"))?;
                self.pressed_buttons.apply(*button, *state);
                if !self.pressed_buttons.has_primary_drag() {
                    self.cursor_pos = current_mouse_location(&mut self.enigo);
                }
            }
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
                apply_pressed_key(&mut self.pressed_keys, key, *state);
            }
            InputEvent::Text { text } => self
                .enigo
                .text(text)
                .map_err(|err| anyhow!("text input failed: {err}"))?,
        }

        Ok(())
    }

    async fn release_all(&mut self) -> Result<()> {
        use enigo::{Direction, Keyboard, Mouse};

        for button in self.pressed_buttons.release_buttons() {
            if let Err(err) = self.enigo.button(map_button(button), Direction::Release) {
                warn!(button = ?button, error = %err, "failed to release stuck mouse button");
            }
        }

        let mut keys = self.pressed_keys.drain().collect::<Vec<_>>();
        keys.sort();
        for key in keys {
            let Some(mapped) = map_key(&key) else {
                continue;
            };
            if let Err(err) = self.enigo.key(mapped, Direction::Release) {
                warn!(key, error = %err, "failed to release stuck key");
            }
        }

        Ok(())
    }
}

impl PressedButtons {
    fn apply(&mut self, button: Button, state: KeyState) {
        let pressed = match state {
            KeyState::Pressed => Some(true),
            KeyState::Released => Some(false),
            KeyState::Clicked => None,
        };
        let Some(pressed) = pressed else {
            return;
        };

        match button {
            Button::Left => self.left = pressed,
            Button::Right => self.right = pressed,
            Button::Middle | Button::Back | Button::Forward => {}
        }
    }

    fn has_primary_drag(&self) -> bool {
        self.left || self.right
    }

    fn release_buttons(&mut self) -> Vec<Button> {
        let mut buttons = Vec::new();
        if self.left {
            buttons.push(Button::Left);
            self.left = false;
        }
        if self.right {
            buttons.push(Button::Right);
            self.right = false;
        }
        buttons
    }
}

fn apply_pressed_key(pressed_keys: &mut HashSet<String>, key: &str, state: KeyState) {
    let key = key.trim().to_ascii_lowercase();
    match state {
        KeyState::Pressed => {
            pressed_keys.insert(key);
        }
        KeyState::Released => {
            pressed_keys.remove(&key);
        }
        KeyState::Clicked => {}
    }
}

#[cfg(target_os = "macos")]
fn macos_declare_user_activity() {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use std::sync::atomic::{AtomicU64, Ordering};

    const MIN_INTERVAL_MS: u64 = 1_000;
    const USER_ACTIVE_LOCAL: u32 = 1;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionDeclareUserActivity(
            assertion_name: CFStringRef,
            user_type: u32,
            assertion_id: *mut u32,
        ) -> i32;
    }

    static LAST_DECLARED_MS: AtomicU64 = AtomicU64::new(0);

    let now = deskbridge_core::now_ms().min(u64::MAX as u128) as u64;
    let last = LAST_DECLARED_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < MIN_INTERVAL_MS {
        return;
    }
    if LAST_DECLARED_MS
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let reason = CFString::new("DeskBridge remote input");
    let mut assertion_id = 0_u32;
    let _ = unsafe {
        IOPMAssertionDeclareUserActivity(
            reason.as_concrete_TypeRef(),
            USER_ACTIVE_LOCAL,
            &mut assertion_id,
        )
    };
}

#[cfg(not(target_os = "macos"))]
fn macos_declare_user_activity() {}

fn move_mouse_rel(
    enigo: &mut enigo::Enigo,
    cursor_pos: &mut Option<(i32, i32)>,
    dx: i32,
    dy: i32,
    dragging: bool,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use enigo::{Coordinate, Mouse};

        if dragging {
            enigo
                .move_mouse(dx, dy, Coordinate::Rel)
                .map_err(|err| anyhow!("mouse drag failed: {err}"))?;
            let fallback = cursor_pos
                .as_ref()
                .map(|(x, y)| (x.saturating_add(dx), y.saturating_add(dy)));
            *cursor_pos = current_mouse_location(enigo).or(fallback);
            return Ok(());
        }

        let (x, y) = match *cursor_pos {
            Some(pos) => pos,
            None => enigo
                .location()
                .map_err(|err| anyhow!("failed to read mouse location: {err}"))?,
        };
        *cursor_pos = Some(macos_move_mouse_evented(
            x.saturating_add(dx),
            y.saturating_add(dy),
            Some((x, y)),
        )?);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = dragging;
        use enigo::{Coordinate, Mouse};
        let result = enigo
            .move_mouse(dx, dy, Coordinate::Rel)
            .map_err(|err| anyhow!("mouse move failed: {err}"));
        if result.is_ok()
            && let Some((x, y)) = cursor_pos.as_mut()
        {
            *x = x.saturating_add(dx);
            *y = y.saturating_add(dy);
        }
        result
    }
}

fn move_mouse_abs(
    enigo: &mut enigo::Enigo,
    cursor_pos: &mut Option<(i32, i32)>,
    x: i32,
    y: i32,
    dragging: bool,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use enigo::{Coordinate, Mouse};

        if dragging {
            enigo
                .move_mouse(x, y, Coordinate::Abs)
                .map_err(|err| anyhow!("absolute mouse drag failed: {err}"))?;
            *cursor_pos = current_mouse_location(enigo).or(Some((x, y)));
            return Ok(());
        }

        let previous = match *cursor_pos {
            Some(pos) => Some(pos),
            None => current_mouse_location(enigo),
        };
        *cursor_pos = Some(macos_move_mouse_evented(x, y, previous)?);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = dragging;
        use enigo::{Coordinate, Mouse};
        let result = enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|err| anyhow!("mouse move failed: {err}"));
        if result.is_ok() {
            *cursor_pos = Some((x, y));
        }
        result
    }
}

fn current_mouse_location(enigo: &mut enigo::Enigo) -> Option<(i32, i32)> {
    use enigo::Mouse;
    enigo.location().ok()
}

#[cfg(target_os = "macos")]
fn macos_move_mouse_evented(x: i32, y: i32, previous: Option<(i32, i32)>) -> Result<(i32, i32)> {
    use core_graphics::display::CGDisplay;
    use core_graphics::event::{
        CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    };
    use core_graphics::geometry::CGPoint;

    let bounds = CGDisplay::main().bounds();
    let max_x = (bounds.size.width as i32).saturating_sub(1).max(0);
    let max_y = (bounds.size.height as i32).saturating_sub(1).max(0);
    let clamped_x = x.clamp(0, max_x);
    let clamped_y = y.clamp(0, max_y);
    let point = CGPoint::new(
        bounds.origin.x + clamped_x as f64,
        bounds.origin.y + clamped_y as f64,
    );
    CGDisplay::warp_mouse_cursor_position(point)
        .map_err(|err| anyhow!("absolute mouse warp failed: {err:?}"))?;

    let source = macos_mouse_event_source()?;
    let event =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|_| anyhow!("failed to create macOS mouse moved event"))?;
    if let Some((previous_x, previous_y)) = previous {
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_DELTA_X,
            clamped_x.saturating_sub(previous_x) as i64,
        );
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_DELTA_Y,
            clamped_y.saturating_sub(previous_y) as i64,
        );
    }
    event.set_integer_value_field(
        EventField::EVENT_SOURCE_USER_DATA,
        enigo::EVENT_MARKER as i64,
    );
    event.post(CGEventTapLocation::HID);

    Ok((clamped_x, clamped_y))
}

#[cfg(target_os = "macos")]
fn macos_mouse_event_source() -> Result<core_graphics::event_source::CGEventSource> {
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use std::cell::RefCell;

    thread_local! {
        static SOURCE: RefCell<Option<CGEventSource>> = const { RefCell::new(None) };
    }

    SOURCE.with(|source| {
        let mut source = source.borrow_mut();
        if let Some(source) = source.as_ref() {
            return Ok(source.clone());
        }

        let created = CGEventSource::new(CGEventSourceStateID::Private)
            .map_err(|_| anyhow!("failed to create macOS mouse event source"))?;
        *source = Some(created.clone());
        Ok(created)
    })
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
        let ch = key.chars().next()?;
        return Some(physical_ascii_key(ch).unwrap_or(enigo::Key::Unicode(ch)));
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
        "digit0" => physical_digit_key('0')?,
        "digit1" => physical_digit_key('1')?,
        "digit2" => physical_digit_key('2')?,
        "digit3" => physical_digit_key('3')?,
        "digit4" => physical_digit_key('4')?,
        "digit5" => physical_digit_key('5')?,
        "digit6" => physical_digit_key('6')?,
        "digit7" => physical_digit_key('7')?,
        "digit8" => physical_digit_key('8')?,
        "digit9" => physical_digit_key('9')?,
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

fn physical_ascii_key(ch: char) -> Option<enigo::Key> {
    if ch.is_ascii_digit() {
        return physical_digit_key(ch);
    }
    if ch.is_ascii_alphabetic() {
        return physical_letter_key(ch);
    }
    None
}

#[cfg(target_os = "macos")]
fn physical_digit_key(ch: char) -> Option<enigo::Key> {
    Some(enigo::Key::Other(match ch {
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '5' => 23,
        '6' => 22,
        '7' => 26,
        '8' => 28,
        '9' => 25,
        '0' => 29,
        _ => return None,
    }))
}

#[cfg(target_os = "macos")]
fn physical_letter_key(ch: char) -> Option<enigo::Key> {
    Some(enigo::Key::Other(match ch.to_ascii_lowercase() {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        'o' => 31,
        'u' => 32,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        'k' => 40,
        'n' => 45,
        'm' => 46,
        _ => return None,
    }))
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
fn physical_digit_key(ch: char) -> Option<enigo::Key> {
    Some(match ch {
        '0' => enigo::Key::Num0,
        '1' => enigo::Key::Num1,
        '2' => enigo::Key::Num2,
        '3' => enigo::Key::Num3,
        '4' => enigo::Key::Num4,
        '5' => enigo::Key::Num5,
        '6' => enigo::Key::Num6,
        '7' => enigo::Key::Num7,
        '8' => enigo::Key::Num8,
        '9' => enigo::Key::Num9,
        _ => return None,
    })
}

#[cfg(target_os = "windows")]
fn physical_letter_key(ch: char) -> Option<enigo::Key> {
    Some(match ch.to_ascii_lowercase() {
        'a' => enigo::Key::A,
        'b' => enigo::Key::B,
        'c' => enigo::Key::C,
        'd' => enigo::Key::D,
        'e' => enigo::Key::E,
        'f' => enigo::Key::F,
        'g' => enigo::Key::G,
        'h' => enigo::Key::H,
        'i' => enigo::Key::I,
        'j' => enigo::Key::J,
        'k' => enigo::Key::K,
        'l' => enigo::Key::L,
        'm' => enigo::Key::M,
        'n' => enigo::Key::N,
        'o' => enigo::Key::O,
        'p' => enigo::Key::P,
        'q' => enigo::Key::Q,
        'r' => enigo::Key::R,
        's' => enigo::Key::S,
        't' => enigo::Key::T,
        'u' => enigo::Key::U,
        'v' => enigo::Key::V,
        'w' => enigo::Key::W,
        'x' => enigo::Key::X,
        'y' => enigo::Key::Y,
        'z' => enigo::Key::Z,
        _ => return None,
    })
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
fn physical_digit_key(ch: char) -> Option<enigo::Key> {
    ch.is_ascii_digit().then_some(enigo::Key::Unicode(ch))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn physical_letter_key(ch: char) -> Option<enigo::Key> {
    ch.is_ascii_alphabetic()
        .then_some(enigo::Key::Unicode(ch.to_ascii_lowercase()))
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

    #[test]
    fn tracks_primary_mouse_buttons_for_drag_events() {
        let mut buttons = PressedButtons::default();
        assert!(!buttons.has_primary_drag());

        buttons.apply(Button::Left, KeyState::Pressed);
        assert!(buttons.has_primary_drag());

        buttons.apply(Button::Left, KeyState::Released);
        assert!(!buttons.has_primary_drag());

        buttons.apply(Button::Right, KeyState::Pressed);
        assert!(buttons.has_primary_drag());

        buttons.apply(Button::Middle, KeyState::Pressed);
        assert!(buttons.has_primary_drag());

        buttons.apply(Button::Right, KeyState::Released);
        assert!(!buttons.has_primary_drag());

        buttons.apply(Button::Left, KeyState::Clicked);
        assert!(!buttons.has_primary_drag());
    }

    #[test]
    fn tracks_pressed_keys_for_session_cleanup() {
        let mut keys = HashSet::new();
        apply_pressed_key(&mut keys, "Alt", KeyState::Pressed);
        apply_pressed_key(&mut keys, "a", KeyState::Clicked);
        assert!(keys.contains("alt"));
        assert!(!keys.contains("a"));

        apply_pressed_key(&mut keys, "alt", KeyState::Released);
        assert!(keys.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn maps_symbol_keys_to_macos_physical_keycodes() {
        assert_eq!(map_key("equal"), Some(enigo::Key::Other(24)));
        assert_eq!(map_key("minus"), Some(enigo::Key::Other(27)));
        assert_eq!(map_key("slash"), Some(enigo::Key::Other(44)));
        assert_eq!(map_key("1"), Some(enigo::Key::Other(18)));
        assert_eq!(map_key("4"), Some(enigo::Key::Other(21)));
        assert_eq!(map_key("digit1"), Some(enigo::Key::Other(18)));
        assert_eq!(map_key("a"), Some(enigo::Key::Other(0)));
        assert_eq!(map_key("A"), Some(enigo::Key::Other(0)));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn maps_ascii_keys_to_windows_physical_keys() {
        assert_eq!(map_key("1"), Some(enigo::Key::Num1));
        assert_eq!(map_key("digit1"), Some(enigo::Key::Num1));
        assert_eq!(map_key("a"), Some(enigo::Key::A));
        assert_eq!(map_key("A"), Some(enigo::Key::A));
    }
}
