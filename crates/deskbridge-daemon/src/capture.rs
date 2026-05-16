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
    use std::sync::{Mutex, OnceLock};
    use std::{ptr::null_mut, thread};
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT,
        VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG,
        MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CYSCREEN, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
        WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    static CAPTURE_TX: OnceLock<Mutex<CaptureSender>> = OnceLock::new();
    static LAST_MOUSE_POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();

    pub struct WindowsHookCapture {
        mouse_hook: HHOOK,
        keyboard_hook: HHOOK,
    }

    impl WindowsHookCapture {
        pub fn install(sender: CaptureSender) -> Result<Self> {
            CAPTURE_TX
                .set(Mutex::new(sender))
                .map_err(|_| anyhow!("windows capture hook already installed"))?;

            let mouse_hook =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), null_mut(), 0) };
            if mouse_hook.is_null() {
                return Err(anyhow!("failed to install low-level mouse hook"));
            }

            let keyboard_hook =
                unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), null_mut(), 0) };
            if keyboard_hook.is_null() {
                unsafe {
                    UnhookWindowsHookEx(mouse_hook);
                }
                return Err(anyhow!("failed to install low-level keyboard hook"));
            }

            Ok(Self {
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
                let mut events = vec![CaptureEvent::LocalPointer {
                    x: x.max(0) as u32,
                    y: y.max(0) as u32,
                }];

                let last = LAST_MOUSE_POS.get_or_init(|| Mutex::new(None));
                if let Ok(mut last) = last.lock() {
                    if let Some((last_x, last_y)) = *last {
                        let dx = x.saturating_sub(last_x);
                        let dy = y.saturating_sub(last_y);
                        if dx != 0 || dy != 0 {
                            events.push(CaptureEvent::Input(InputEvent::MouseMove { dx, dy }));
                        }
                    }
                    *last = Some((x, y));
                }

                events
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
