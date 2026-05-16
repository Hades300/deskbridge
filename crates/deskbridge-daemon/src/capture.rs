use deskbridge_core::InputEvent;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    LocalPointer { x: u32, y: u32 },
    Input(InputEvent),
}

pub type CaptureSender = broadcast::Sender<CaptureEvent>;
pub type CaptureReceiver = broadcast::Receiver<CaptureEvent>;

pub fn channel() -> (CaptureSender, CaptureReceiver) {
    broadcast::channel(256)
}

#[cfg(target_os = "windows")]
pub mod windows {
    use super::{CaptureEvent, CaptureSender, InputEvent};
    use anyhow::{Result, anyhow};
    use deskbridge_core::{Button, KeyState};
    use std::mem::size_of;
    use std::sync::{Mutex, OnceLock};
    use std::{
        ptr::{null, null_mut},
        thread,
    };
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT,
        VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
    };
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
        RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetMessageW, HC_ACTION, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
        RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_INPUT, WM_KEYDOWN, WM_KEYUP,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
        WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
    };

    static CAPTURE_TX: OnceLock<Mutex<CaptureSender>> = OnceLock::new();

    pub struct WindowsHookCapture {
        raw_input_hwnd: HWND,
        mouse_hook: HHOOK,
        keyboard_hook: HHOOK,
    }

    impl WindowsHookCapture {
        pub fn install(sender: CaptureSender) -> Result<Self> {
            CAPTURE_TX
                .set(Mutex::new(sender))
                .map_err(|_| anyhow!("windows capture hook already installed"))?;

            let raw_input_hwnd = create_raw_input_window()?;
            register_raw_mouse(raw_input_hwnd)?;

            let mouse_hook =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), null_mut(), 0) };
            if mouse_hook.is_null() {
                unsafe {
                    DestroyWindow(raw_input_hwnd);
                }
                return Err(anyhow!("failed to install low-level mouse hook"));
            }

            let keyboard_hook =
                unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), null_mut(), 0) };
            if keyboard_hook.is_null() {
                unsafe {
                    UnhookWindowsHookEx(mouse_hook);
                    DestroyWindow(raw_input_hwnd);
                }
                return Err(anyhow!("failed to install low-level keyboard hook"));
            }

            Ok(Self {
                raw_input_hwnd,
                mouse_hook,
                keyboard_hook,
            })
        }

        pub fn run_message_loop(&self) -> Result<()> {
            let mut msg = MSG::default();
            loop {
                let result = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
                if result == -1 {
                    return Err(anyhow!("windows message loop failed"));
                }
                if result == 0 {
                    return Ok(());
                }

                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    impl Drop for WindowsHookCapture {
        fn drop(&mut self) {
            unsafe {
                UnhookWindowsHookEx(self.mouse_hook);
                UnhookWindowsHookEx(self.keyboard_hook);
                DestroyWindow(self.raw_input_hwnd);
            }
        }
    }

    pub fn spawn(sender: CaptureSender) -> Result<()> {
        thread::Builder::new()
            .name("deskbridge-windows-capture".to_string())
            .spawn(move || match WindowsHookCapture::install(sender) {
                Ok(capture) => {
                    if let Err(err) = capture.run_message_loop() {
                        eprintln!("deskbridge windows capture stopped: {err:#}");
                    }
                }
                Err(err) => eprintln!("deskbridge windows capture failed: {err:#}"),
            })
            .map(|_| ())
            .map_err(|err| anyhow!("failed to spawn windows capture thread: {err}"))
    }

    pub fn primary_screen_size() -> Option<(u32, u32)> {
        let width =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CXSCREEN) };
        let height =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CYSCREEN) };
        (width > 0 && height > 0).then_some((width as u32, height as u32))
    }

    fn create_raw_input_window() -> Result<HWND> {
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(anyhow!("failed to get module handle for raw input window"));
        }

        let class_name = wide_null("DeskBridgeRawInputWindow");
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(raw_input_window_proc),
            hInstance: instance as _,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };

        unsafe {
            RegisterClassW(&wnd_class);
        }

        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance as _,
                null(),
            )
        };
        if hwnd.is_null() {
            return Err(anyhow!("failed to create raw input message window"));
        }

        Ok(hwnd)
    }

    fn register_raw_mouse(hwnd: HWND) -> Result<()> {
        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        let ok = unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) };
        if ok == 0 {
            return Err(anyhow!("failed to register raw mouse input"));
        }
        Ok(())
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn raw_input_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_INPUT
            && let Some(event) = unsafe { raw_mouse_event_from_lparam(lparam) }
        {
            send(event);
        }

        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    unsafe fn raw_mouse_event_from_lparam(lparam: LPARAM) -> Option<CaptureEvent> {
        let mut size = 0_u32;
        let header_size = size_of::<RAWINPUTHEADER>() as u32;
        let input = lparam as HRAWINPUT;

        let result =
            unsafe { GetRawInputData(input, RID_INPUT, null_mut(), &mut size, header_size) };
        if result == u32::MAX || size == 0 {
            return None;
        }

        let mut buffer = vec![0_u8; size as usize];
        let read = unsafe {
            GetRawInputData(
                input,
                RID_INPUT,
                buffer.as_mut_ptr().cast(),
                &mut size,
                header_size,
            )
        };
        if read == u32::MAX || read != size {
            return None;
        }

        let raw = unsafe { &*(buffer.as_ptr() as *const RAWINPUT) };
        if raw.header.dwType != RIM_TYPEMOUSE {
            return None;
        }

        let mouse = unsafe { raw.data.mouse };
        if mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
            return None;
        }

        let dx = mouse.lLastX;
        let dy = mouse.lLastY;
        if dx == 0 && dy == 0 {
            return None;
        }

        Some(CaptureEvent::Input(InputEvent::MouseMove { dx, dy }))
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            for event in unsafe { mouse_events_from_hook(wparam, lparam) } {
                send(event);
            }
        }

        unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let event = unsafe { keyboard_event_from_hook(wparam, lparam) };
            if let Some(event) = event {
                send(event);
            }
        }

        unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
    }

    unsafe fn mouse_events_from_hook(wparam: WPARAM, lparam: LPARAM) -> Vec<CaptureEvent> {
        let hook = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };

        match wparam as u32 {
            WM_MOUSEMOVE => {
                let x = hook.pt.x;
                let y = hook.pt.y;
                vec![CaptureEvent::LocalPointer {
                    x: x.max(0) as u32,
                    y: y.max(0) as u32,
                }]
            }
            WM_LBUTTONDOWN => vec![mouse_button(Button::Left, KeyState::Pressed)],
            WM_LBUTTONUP => vec![mouse_button(Button::Left, KeyState::Released)],
            WM_RBUTTONDOWN => vec![mouse_button(Button::Right, KeyState::Pressed)],
            WM_RBUTTONUP => vec![mouse_button(Button::Right, KeyState::Released)],
            WM_MBUTTONDOWN => vec![mouse_button(Button::Middle, KeyState::Pressed)],
            WM_MBUTTONUP => vec![mouse_button(Button::Middle, KeyState::Released)],
            WM_MOUSEWHEEL => {
                let delta = ((hook.mouseData >> 16) as i16) as i32;
                vec![CaptureEvent::Input(InputEvent::Wheel { dx: 0, dy: delta })]
            }
            _ => Vec::new(),
        }
    }

    unsafe fn keyboard_event_from_hook(wparam: WPARAM, lparam: LPARAM) -> Option<CaptureEvent> {
        let hook = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let state = match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyState::Pressed,
            WM_KEYUP | WM_SYSKEYUP => KeyState::Released,
            _ => return None,
        };
        let key = key_name(hook.vkCode)?;
        Some(CaptureEvent::Input(InputEvent::Key { key, state }))
    }

    fn mouse_button(button: Button, state: KeyState) -> CaptureEvent {
        CaptureEvent::Input(InputEvent::MouseButton { button, state })
    }

    fn key_name(vk_code: u32) -> Option<String> {
        let vk_code = vk_code as u16;
        let key = match vk_code {
            VK_BACK => "backspace",
            VK_CONTROL => "control",
            VK_DELETE => "delete",
            VK_DOWN => "down",
            VK_ESCAPE => "escape",
            VK_LEFT => "left",
            VK_MENU => "alt",
            VK_RETURN => "enter",
            VK_RIGHT => "right",
            VK_SHIFT => "shift",
            VK_SPACE => "space",
            VK_TAB => "tab",
            VK_UP => "up",
            0x30..=0x39 | 0x41..=0x5A => {
                return char::from_u32(vk_code as u32).map(|ch| ch.to_string());
            }
            _ => return None,
        };

        Some(key.to_string())
    }

    fn send(event: CaptureEvent) {
        let Some(sender) = CAPTURE_TX.get() else {
            return;
        };
        if let Ok(sender) = sender.lock() {
            let _ = sender.send(event);
        }
    }
}

#[cfg(target_os = "macos")]
pub mod macos {
    use super::{CaptureEvent, CaptureSender, InputEvent};
    use anyhow::{Result, anyhow};
    use core_foundation::runloop::CFRunLoop;
    use core_graphics::event::{
        CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, CallbackResult, EventField, KeyCode,
    };
    use deskbridge_core::{Button, KeyState};
    use std::thread;

    pub fn spawn(sender: CaptureSender) -> Result<()> {
        thread::Builder::new()
            .name("deskbridge-macos-capture".to_string())
            .spawn(move || {
                if let Err(err) = run_event_tap(sender) {
                    eprintln!("deskbridge macos capture failed: {err:#}");
                }
            })
            .map(|_| ())
            .map_err(|err| anyhow!("failed to spawn macOS capture thread: {err}"))
    }

    pub fn primary_screen_size() -> Option<(u32, u32)> {
        let display = core_graphics::display::CGDisplay::main();
        let width = display.pixels_wide();
        let height = display.pixels_high();
        (width > 0 && height > 0).then_some((width as u32, height as u32))
    }

    fn run_event_tap(sender: CaptureSender) -> Result<()> {
        let events = vec![
            CGEventType::MouseMoved,
            CGEventType::LeftMouseDragged,
            CGEventType::RightMouseDragged,
            CGEventType::OtherMouseDragged,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::ScrollWheel,
            CGEventType::KeyDown,
            CGEventType::KeyUp,
        ];

        CGEventTap::with_enabled(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            events,
            move |_proxy, event_type, event| {
                for capture_event in capture_events_from_cg_event(event_type, event) {
                    let _ = sender.send(capture_event);
                }
                CallbackResult::Keep
            },
            CFRunLoop::run_current,
        )
        .map_err(|_| anyhow!("failed to install macOS event tap; grant Input Monitoring and Accessibility to DeskBridge"))
    }

    fn capture_events_from_cg_event(event_type: CGEventType, event: &CGEvent) -> Vec<CaptureEvent> {
        match event_type {
            CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged => mouse_move_events(event),
            CGEventType::LeftMouseDown => vec![mouse_button(Button::Left, KeyState::Pressed)],
            CGEventType::LeftMouseUp => vec![mouse_button(Button::Left, KeyState::Released)],
            CGEventType::RightMouseDown => vec![mouse_button(Button::Right, KeyState::Pressed)],
            CGEventType::RightMouseUp => vec![mouse_button(Button::Right, KeyState::Released)],
            CGEventType::OtherMouseDown => vec![mouse_button(Button::Middle, KeyState::Pressed)],
            CGEventType::OtherMouseUp => vec![mouse_button(Button::Middle, KeyState::Released)],
            CGEventType::ScrollWheel => vec![CaptureEvent::Input(InputEvent::Wheel {
                dx: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2)
                    as i32,
                dy: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1)
                    as i32,
            })],
            CGEventType::KeyDown => keyboard_event(event, KeyState::Pressed)
                .into_iter()
                .collect(),
            CGEventType::KeyUp => keyboard_event(event, KeyState::Released)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn mouse_move_events(event: &CGEvent) -> Vec<CaptureEvent> {
        let location = event.location();
        let mut events = vec![CaptureEvent::LocalPointer {
            x: location.x.max(0.0) as u32,
            y: location.y.max(0.0) as u32,
        }];

        let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as i32;
        let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as i32;
        if dx != 0 || dy != 0 {
            events.push(CaptureEvent::Input(InputEvent::MouseMove { dx, dy }));
        }

        events
    }

    fn mouse_button(button: Button, state: KeyState) -> CaptureEvent {
        CaptureEvent::Input(InputEvent::MouseButton { button, state })
    }

    fn keyboard_event(event: &CGEvent, state: KeyState) -> Option<CaptureEvent> {
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        key_name(keycode).map(|key| CaptureEvent::Input(InputEvent::Key { key, state }))
    }

    fn key_name(keycode: u16) -> Option<String> {
        let key = match keycode {
            KeyCode::ANSI_A => "a",
            KeyCode::ANSI_B => "b",
            KeyCode::ANSI_C => "c",
            KeyCode::ANSI_D => "d",
            KeyCode::ANSI_E => "e",
            KeyCode::ANSI_F => "f",
            KeyCode::ANSI_G => "g",
            KeyCode::ANSI_H => "h",
            KeyCode::ANSI_I => "i",
            KeyCode::ANSI_J => "j",
            KeyCode::ANSI_K => "k",
            KeyCode::ANSI_L => "l",
            KeyCode::ANSI_M => "m",
            KeyCode::ANSI_N => "n",
            KeyCode::ANSI_O => "o",
            KeyCode::ANSI_P => "p",
            KeyCode::ANSI_Q => "q",
            KeyCode::ANSI_R => "r",
            KeyCode::ANSI_S => "s",
            KeyCode::ANSI_T => "t",
            KeyCode::ANSI_U => "u",
            KeyCode::ANSI_V => "v",
            KeyCode::ANSI_W => "w",
            KeyCode::ANSI_X => "x",
            KeyCode::ANSI_Y => "y",
            KeyCode::ANSI_Z => "z",
            KeyCode::ANSI_0 => "0",
            KeyCode::ANSI_1 => "1",
            KeyCode::ANSI_2 => "2",
            KeyCode::ANSI_3 => "3",
            KeyCode::ANSI_4 => "4",
            KeyCode::ANSI_5 => "5",
            KeyCode::ANSI_6 => "6",
            KeyCode::ANSI_7 => "7",
            KeyCode::ANSI_8 => "8",
            KeyCode::ANSI_9 => "9",
            KeyCode::RETURN => "enter",
            KeyCode::TAB => "tab",
            KeyCode::SPACE => "space",
            KeyCode::DELETE => "backspace",
            KeyCode::ESCAPE => "escape",
            KeyCode::COMMAND => "command",
            KeyCode::SHIFT => "shift",
            KeyCode::CAPS_LOCK => "capslock",
            KeyCode::OPTION => "alt",
            KeyCode::CONTROL => "control",
            KeyCode::RIGHT_COMMAND => "command",
            KeyCode::RIGHT_SHIFT => "shift",
            KeyCode::RIGHT_OPTION => "alt",
            KeyCode::RIGHT_CONTROL => "control",
            KeyCode::LEFT_ARROW => "left",
            KeyCode::RIGHT_ARROW => "right",
            KeyCode::DOWN_ARROW => "down",
            KeyCode::UP_ARROW => "up",
            _ => return None,
        };

        Some(key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_channel_transports_events() {
        let (tx, mut rx) = channel();
        tx.send(CaptureEvent::Input(InputEvent::MouseMove { dx: 2, dy: 3 }))
            .unwrap();
        assert_eq!(
            rx.try_recv().unwrap(),
            CaptureEvent::Input(InputEvent::MouseMove { dx: 2, dy: 3 })
        );
    }
}
