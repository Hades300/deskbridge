use deskbridge_core::InputEvent;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    LocalPointer { x: u32, y: u32 },
    Input(InputEvent),
    ProbeLocalPointer { request_id: Uuid, x: u32, y: u32 },
    ProbeInput { request_id: Uuid, event: InputEvent },
}

pub type CaptureSender = broadcast::Sender<CaptureEvent>;
pub type CaptureReceiver = broadcast::Receiver<CaptureEvent>;

pub fn channel() -> (CaptureSender, CaptureReceiver) {
    broadcast::channel(256)
}

pub fn set_local_input_suppressed(suppressed: bool) {
    set_platform_local_input_suppressed(suppressed);
}

#[cfg(not(target_os = "windows"))]
fn set_platform_local_input_suppressed(_suppressed: bool) {}

#[cfg(target_os = "windows")]
fn set_platform_local_input_suppressed(suppressed: bool) {
    windows::set_local_input_suppressed(suppressed);
}

#[cfg(target_os = "windows")]
pub mod windows {
    use super::{CaptureEvent, CaptureSender, InputEvent};
    use anyhow::{Result, anyhow};
    use deskbridge_core::{Button, KeyState};
    use std::mem::size_of;
    use std::sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    };
    use std::{
        ptr::{null, null_mut},
        thread,
    };
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetAwarenessFromDpiAwarenessContext,
        GetDpiFromDpiAwarenessContext, GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_ADD, VK_APPS, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DECIMAL, VK_DELETE, VK_DIVIDE,
        VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9,
        VK_F10, VK_F11, VK_F12, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F20,
        VK_F21, VK_F22, VK_F23, VK_F24, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU,
        VK_LSHIFT, VK_LWIN, VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2,
        VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9,
        VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_102,
        VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PAUSE, VK_PRIOR, VK_RCONTROL,
        VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT,
        VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
    };
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
        RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetMessageW, GetSystemMetrics, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
        LLMHF_INJECTED, MONITORINFOF_PRIMARY, MSG, MSLLHOOKSTRUCT, RegisterClassW, SM_CXSCREEN,
        SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        SetProcessDPIAware, SetWindowsHookExW, ShowCursor, TranslateMessage, UnhookWindowsHookEx,
        WH_KEYBOARD_LL, WH_MOUSE_LL, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
    };

    static CAPTURE_TX: OnceLock<Mutex<CaptureSender>> = OnceLock::new();
    static CAPTURE_BOUNDS: OnceLock<ScreenBounds> = OnceLock::new();
    static DPI_AWARENESS_CONFIGURED: OnceLock<()> = OnceLock::new();
    static SUPPRESS_LOCAL_INPUT: AtomicBool = AtomicBool::new(false);
    static LOCAL_CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);
    static VERTICAL_WHEEL_REMAINDER: Mutex<i32> = Mutex::new(0);
    const WINDOWS_WHEEL_DELTA: i32 = 120;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ScreenBounds {
        origin_x: i32,
        origin_y: i32,
        width: u32,
        height: u32,
    }

    pub struct WindowsHookCapture {
        raw_input_hwnd: HWND,
        mouse_hook: HHOOK,
        keyboard_hook: HHOOK,
    }

    impl WindowsHookCapture {
        pub fn install(sender: CaptureSender) -> Result<Self> {
            configure_process_dpi_awareness();
            CAPTURE_TX
                .set(Mutex::new(sender))
                .map_err(|_| anyhow!("windows capture hook already installed"))?;
            let _ = CAPTURE_BOUNDS.set(platform_screen_bounds());

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

    pub fn set_local_input_suppressed(suppressed: bool) {
        SUPPRESS_LOCAL_INPUT.store(suppressed, Ordering::SeqCst);
        set_local_cursor_hidden(suppressed);
    }

    pub fn configure_process_dpi_awareness() {
        let _ = DPI_AWARENESS_CONFIGURED.get_or_init(|| unsafe {
            if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
                let _ = SetProcessDPIAware();
            }
        });
    }

    pub fn primary_screen_size() -> Option<(u32, u32)> {
        let bounds = platform_screen_bounds();
        Some((bounds.width, bounds.height))
    }

    pub fn screen_debug_lines() -> Vec<String> {
        configure_process_dpi_awareness();
        let primary = primary_screen_bounds();
        let virtual_bounds = virtual_screen_bounds();
        let platform = platform_screen_bounds();
        let capture = CAPTURE_BOUNDS.get().copied();
        let mut lines = vec![
            format!("windows_dpi {}", dpi_awareness_summary()),
            format!(
                "windows_screen primary={} virtual={} platform={}",
                describe_bounds(primary),
                virtual_bounds
                    .map(describe_bounds)
                    .unwrap_or_else(|| "unavailable".to_string()),
                describe_bounds(platform),
            ),
            format!(
                "windows_capture_bounds={}",
                capture
                    .map(describe_bounds)
                    .unwrap_or_else(|| "not_initialized".to_string())
            ),
            format!(
                "windows_local_input suppressed={} cursor_hidden={}",
                SUPPRESS_LOCAL_INPUT.load(Ordering::SeqCst),
                LOCAL_CURSOR_HIDDEN.load(Ordering::SeqCst)
            ),
        ];

        let monitors = monitor_bounds();
        lines.push(format!("windows_monitors={}", monitors.len()));
        for (index, monitor) in monitors.iter().enumerate() {
            lines.push(format!(
                "windows_monitor[{index}] bounds={} work={} primary={}",
                describe_bounds(monitor.bounds),
                describe_bounds(monitor.work),
                monitor.primary
            ));
        }

        lines
    }

    fn platform_screen_bounds() -> ScreenBounds {
        configure_process_dpi_awareness();
        if let Some(bounds) = virtual_screen_bounds() {
            return bounds;
        }

        primary_screen_bounds()
    }

    fn primary_screen_bounds() -> ScreenBounds {
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        ScreenBounds {
            origin_x: 0,
            origin_y: 0,
            width: width.max(1) as u32,
            height: height.max(1) as u32,
        }
    }

    fn virtual_screen_bounds() -> Option<ScreenBounds> {
        let origin_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let origin_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };

        (width > 0 && height > 0).then_some(ScreenBounds {
            origin_x,
            origin_y,
            width: width as u32,
            height: height as u32,
        })
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct MonitorBounds {
        bounds: ScreenBounds,
        work: ScreenBounds,
        primary: bool,
    }

    fn monitor_bounds() -> Vec<MonitorBounds> {
        let mut monitors = Vec::<MonitorBounds>::new();
        let ok = unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitor_proc),
                &mut monitors as *mut _ as LPARAM,
            )
        };
        if ok == 0 { Vec::new() } else { monitors }
    }

    unsafe extern "system" fn enum_monitor_proc(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows_sys::core::BOOL {
        let monitors = unsafe { &mut *(data as *mut Vec<MonitorBounds>) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(hmonitor, &mut info) } != 0 {
            monitors.push(MonitorBounds {
                bounds: bounds_from_rect(info.rcMonitor),
                work: bounds_from_rect(info.rcWork),
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        1
    }

    fn bounds_from_rect(rect: RECT) -> ScreenBounds {
        ScreenBounds {
            origin_x: rect.left,
            origin_y: rect.top,
            width: (rect.right - rect.left).max(1) as u32,
            height: (rect.bottom - rect.top).max(1) as u32,
        }
    }

    fn describe_bounds(bounds: ScreenBounds) -> String {
        let max_x = bounds.origin_x + bounds.width.saturating_sub(1) as i32;
        let max_y = bounds.origin_y + bounds.height.saturating_sub(1) as i32;
        format!(
            "origin=({}, {}) size={}x{} max=({}, {})",
            bounds.origin_x, bounds.origin_y, bounds.width, bounds.height, max_x, max_y
        )
    }

    fn dpi_awareness_summary() -> String {
        let context = unsafe { GetThreadDpiAwarenessContext() };
        let awareness = unsafe { GetAwarenessFromDpiAwarenessContext(context) };
        let dpi = unsafe { GetDpiFromDpiAwarenessContext(context) };
        format!("awareness={awareness} dpi={dpi}")
    }

    fn set_local_cursor_hidden(hidden: bool) {
        if LOCAL_CURSOR_HIDDEN.swap(hidden, Ordering::SeqCst) == hidden {
            return;
        }

        unsafe {
            if hidden {
                for _ in 0..32 {
                    if ShowCursor(0) < 0 {
                        break;
                    }
                }
            } else {
                for _ in 0..32 {
                    if ShowCursor(1) >= 0 {
                        break;
                    }
                }
            }
        }
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
                null_mut(),
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

        let mut stack_buffer = [0_u8; size_of::<RAWINPUT>()];
        let mut heap_buffer = Vec::new();
        let buffer = if size as usize <= stack_buffer.len() {
            &mut stack_buffer[..size as usize]
        } else {
            heap_buffer.resize(size as usize, 0);
            heap_buffer.as_mut_slice()
        };
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

        let raw = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<RAWINPUT>()) };
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
        let mut suppress = false;
        if code == HC_ACTION as i32 {
            for event in unsafe { mouse_events_from_hook(wparam, lparam) } {
                send(event);
            }
            suppress = unsafe { should_suppress_mouse(lparam) };
        }

        if suppress {
            1
        } else {
            unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
        }
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let mut suppress = false;
        if code == HC_ACTION as i32 {
            let event = unsafe { keyboard_event_from_hook(wparam, lparam) };
            if let Some(event) = event {
                send(event);
            }
            suppress = unsafe { should_suppress_keyboard(lparam) };
        }

        if suppress {
            1
        } else {
            unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
        }
    }

    unsafe fn should_suppress_mouse(lparam: LPARAM) -> bool {
        if !SUPPRESS_LOCAL_INPUT.load(Ordering::SeqCst) {
            return false;
        }
        let hook = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        hook.flags & LLMHF_INJECTED == 0
    }

    unsafe fn should_suppress_keyboard(lparam: LPARAM) -> bool {
        if !SUPPRESS_LOCAL_INPUT.load(Ordering::SeqCst) {
            return false;
        }
        let hook = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        hook.flags & LLKHF_INJECTED == 0
    }

    unsafe fn mouse_events_from_hook(wparam: WPARAM, lparam: LPARAM) -> Vec<CaptureEvent> {
        let hook = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };

        match wparam as u32 {
            WM_MOUSEMOVE => {
                let bounds = *CAPTURE_BOUNDS.get_or_init(platform_screen_bounds);
                let (x, y) = normalize_point(hook.pt.x, hook.pt.y, bounds);
                vec![CaptureEvent::LocalPointer { x, y }]
            }
            WM_LBUTTONDOWN => vec![mouse_button(Button::Left, KeyState::Pressed)],
            WM_LBUTTONUP => vec![mouse_button(Button::Left, KeyState::Released)],
            WM_RBUTTONDOWN => vec![mouse_button(Button::Right, KeyState::Pressed)],
            WM_RBUTTONUP => vec![mouse_button(Button::Right, KeyState::Released)],
            WM_MBUTTONDOWN => vec![mouse_button(Button::Middle, KeyState::Pressed)],
            WM_MBUTTONUP => vec![mouse_button(Button::Middle, KeyState::Released)],
            WM_MOUSEWHEEL => {
                let delta = ((hook.mouseData >> 16) as i16) as i32;
                let dy = vertical_wheel_notches(delta);
                if dy == 0 {
                    Vec::new()
                } else {
                    vec![CaptureEvent::Input(InputEvent::Wheel { dx: 0, dy })]
                }
            }
            _ => Vec::new(),
        }
    }

    fn vertical_wheel_notches(delta: i32) -> i32 {
        let mut remainder = VERTICAL_WHEEL_REMAINDER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wheel_notches_from_delta(&mut *remainder, delta)
    }

    fn wheel_notches_from_delta(remainder: &mut i32, delta: i32) -> i32 {
        *remainder = remainder.saturating_add(delta);
        let notches = *remainder / WINDOWS_WHEEL_DELTA;
        *remainder %= WINDOWS_WHEEL_DELTA;
        notches
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

    fn normalize_point(x: i32, y: i32, bounds: ScreenBounds) -> (u32, u32) {
        let max_x = bounds.width.saturating_sub(1) as i32;
        let max_y = bounds.height.saturating_sub(1) as i32;
        (
            (x - bounds.origin_x).clamp(0, max_x) as u32,
            (y - bounds.origin_y).clamp(0, max_y) as u32,
        )
    }

    fn key_name(vk_code: u32) -> Option<String> {
        let vk_code = vk_code as u16;
        let key = match vk_code {
            VK_BACK => "backspace",
            VK_CAPITAL => "capslock",
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => "control",
            VK_DELETE => "delete",
            VK_DOWN => "down",
            VK_END => "end",
            VK_ESCAPE => "escape",
            VK_HOME => "home",
            VK_INSERT => "insert",
            VK_LEFT => "left",
            VK_LWIN | VK_RWIN => "meta",
            VK_MENU | VK_LMENU | VK_RMENU => "alt",
            VK_NEXT => "pagedown",
            VK_PAUSE => "pause",
            VK_PRIOR => "pageup",
            VK_RETURN => "enter",
            VK_RIGHT => "right",
            VK_SCROLL => "scrolllock",
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => "shift",
            VK_SNAPSHOT => "printscreen",
            VK_SPACE => "space",
            VK_TAB => "tab",
            VK_UP => "up",
            VK_APPS => "apps",
            VK_F1 => "f1",
            VK_F2 => "f2",
            VK_F3 => "f3",
            VK_F4 => "f4",
            VK_F5 => "f5",
            VK_F6 => "f6",
            VK_F7 => "f7",
            VK_F8 => "f8",
            VK_F9 => "f9",
            VK_F10 => "f10",
            VK_F11 => "f11",
            VK_F12 => "f12",
            VK_F13 => "f13",
            VK_F14 => "f14",
            VK_F15 => "f15",
            VK_F16 => "f16",
            VK_F17 => "f17",
            VK_F18 => "f18",
            VK_F19 => "f19",
            VK_F20 => "f20",
            VK_F21 => "f21",
            VK_F22 => "f22",
            VK_F23 => "f23",
            VK_F24 => "f24",
            VK_NUMPAD0 => "numpad0",
            VK_NUMPAD1 => "numpad1",
            VK_NUMPAD2 => "numpad2",
            VK_NUMPAD3 => "numpad3",
            VK_NUMPAD4 => "numpad4",
            VK_NUMPAD5 => "numpad5",
            VK_NUMPAD6 => "numpad6",
            VK_NUMPAD7 => "numpad7",
            VK_NUMPAD8 => "numpad8",
            VK_NUMPAD9 => "numpad9",
            VK_ADD => "numpad_add",
            VK_DECIMAL => "numpad_decimal",
            VK_DIVIDE => "numpad_divide",
            VK_MULTIPLY => "numpad_multiply",
            VK_SUBTRACT => "numpad_subtract",
            VK_OEM_1 => "semicolon",
            VK_OEM_2 => "slash",
            VK_OEM_3 => "grave",
            VK_OEM_4 => "left_bracket",
            VK_OEM_5 => "backslash",
            VK_OEM_6 => "right_bracket",
            VK_OEM_7 => "quote",
            VK_OEM_102 => "intl_backslash",
            VK_OEM_COMMA => "comma",
            VK_OEM_MINUS => "minus",
            VK_OEM_PERIOD => "period",
            VK_OEM_PLUS => "equal",
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn normalizes_virtual_desktop_coordinates() {
            let bounds = ScreenBounds {
                origin_x: -1920,
                origin_y: -100,
                width: 3840,
                height: 1180,
            };

            assert_eq!(normalize_point(-1920, -100, bounds), (0, 0));
            assert_eq!(normalize_point(-1, 540, bounds), (1919, 640));
            assert_eq!(normalize_point(4000, 2000, bounds), (3839, 1179));
        }

        #[test]
        fn describes_screen_bounds_for_debugging() {
            let bounds = ScreenBounds {
                origin_x: 10,
                origin_y: 20,
                width: 1920,
                height: 1080,
            };

            assert_eq!(
                describe_bounds(bounds),
                "origin=(10, 20) size=1920x1080 max=(1929, 1099)"
            );
        }

        #[test]
        fn normalizes_windows_wheel_delta_to_logical_notches() {
            let mut remainder = 0;
            assert_eq!(wheel_notches_from_delta(&mut remainder, 120), 1);
            assert_eq!(remainder, 0);

            assert_eq!(wheel_notches_from_delta(&mut remainder, -240), -2);
            assert_eq!(remainder, 0);
        }

        #[test]
        fn accumulates_partial_windows_wheel_delta() {
            let mut remainder = 0;
            assert_eq!(wheel_notches_from_delta(&mut remainder, 30), 0);
            assert_eq!(remainder, 30);
            assert_eq!(wheel_notches_from_delta(&mut remainder, 30), 0);
            assert_eq!(wheel_notches_from_delta(&mut remainder, 60), 1);
            assert_eq!(remainder, 0);

            assert_eq!(wheel_notches_from_delta(&mut remainder, -60), 0);
            assert_eq!(remainder, -60);
            assert_eq!(wheel_notches_from_delta(&mut remainder, -60), -1);
            assert_eq!(remainder, 0);
        }

        #[test]
        fn maps_extended_windows_keys() {
            assert_eq!(key_name(VK_LSHIFT as u32).as_deref(), Some("shift"));
            assert_eq!(key_name(VK_RSHIFT as u32).as_deref(), Some("shift"));
            assert_eq!(key_name(VK_LWIN as u32).as_deref(), Some("meta"));
            assert_eq!(key_name(VK_OEM_PLUS as u32).as_deref(), Some("equal"));
            assert_eq!(key_name(VK_ADD as u32).as_deref(), Some("numpad_add"));
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
