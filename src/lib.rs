#![deny(missing_docs)]

//! A Glutin window back-end for the Piston game engine.

extern crate glutin;
extern crate gl;
extern crate input;
extern crate window;
extern crate shader_version;

use glutin::GlContext;

use std::collections::VecDeque;

// External crates.
use input::{
    keyboard,
    ButtonArgs,
    ButtonState,
    CloseArgs,
    MouseButton,
    Button,
    Input,
};
use window::{
    BuildFromWindowSettings,
    OpenGLWindow,
    Window,
    AdvancedWindow,
    ProcAddress,
    WindowSettings,
    Size,
    Position,
};
use glutin::{ Api, GlRequest };
use std::time::Duration;
use std::thread;

pub use shader_version::OpenGL;

/// Contains stuff for game window.
pub struct GlutinWindow {
    /// The window.
    pub window: glutin::GlWindow,
    // The back-end does not remember the title.
    title: String,
    exit_on_esc: bool,
    should_close: bool,
    // Used to fake capturing of cursor,
    // to get relative mouse events.
    is_capturing_cursor: bool,
    // Stores the last known cursor position.
    last_cursor_pos: Option<[f64; 2]>,
    // Stores relative coordinates to emit on next poll.
    mouse_relative: Option<(f64, f64)>,
    // Used to emit cursor event after enter/leave.
    cursor_pos: Option<[f64; 2]>,
    // Polls events from window.
    events_loop: glutin::EventsLoop,
    // Stores list of events ready for processing.
    events: VecDeque<glutin::Event>,
}

fn window_builder_from_settings(settings: &WindowSettings) -> glutin::WindowBuilder {
    let size = settings.get_size();
    let mut builder = glutin::WindowBuilder::new()
        .with_dimensions(size.width, size.height)
        .with_decorations(settings.get_decorated())
        .with_multitouch()
        .with_title(settings.get_title());
    if settings.get_fullscreen() {
        let events_loop = glutin::EventsLoop::new();
        builder = builder.with_fullscreen(Some(events_loop.get_primary_monitor()));
    }
    builder
}

fn context_builder_from_settings(settings: &WindowSettings) -> glutin::ContextBuilder {
    let opengl = settings.get_maybe_opengl().unwrap_or(OpenGL::V3_2);
    let (major, minor) = opengl.get_major_minor();
    let mut builder = glutin::ContextBuilder::new()
        .with_gl(GlRequest::Specific(Api::OpenGl, (major as u8, minor as u8)))
        .with_srgb(settings.get_srgb());
    let samples = settings.get_samples();
    if settings.get_vsync() {
        builder = builder.with_vsync(true);
    }
    if samples != 0 {
        builder = builder.with_multisampling(samples as u16);
    }
    builder}

impl GlutinWindow {

    /// Creates a new game window for Glutin.
    pub fn new(settings: &WindowSettings) -> Result<Self, String> {
        use std::error::Error;
        use glutin::ContextError;

        let events_loop = glutin::EventsLoop::new();
        let title = settings.get_title();
        let exit_on_esc = settings.get_exit_on_esc();
        let window = glutin::GlWindow::new(
            window_builder_from_settings(&settings),
            context_builder_from_settings(&settings),
            &events_loop
        );
        let window = match window {
                Ok(window) => window,
                Err(_) => {
                    try!(
                        glutin::GlWindow::new(
                            window_builder_from_settings(&settings),
                            context_builder_from_settings(&settings.clone().samples(0)),
                            &events_loop
                        ).map_err(|e| format!("{}", e))
                    )
                }
            };
        unsafe { try!(window.make_current().map_err(|e|
                // This can be simplified in next version of Glutin.
                match e {
                    ContextError::IoError(ref err) => {
                        String::from(err.description())
                    }
                    ContextError::ContextLost => {
                        String::from("Context lost")
                    }
                }
            )); }

        // Load the OpenGL function pointers.
        gl::load_with(|s| window.get_proc_address(s) as *const _);

        Ok(GlutinWindow {
            window: window,
            title: title,
            exit_on_esc: exit_on_esc,
            should_close: false,
            cursor_pos: None,
            is_capturing_cursor: false,
            last_cursor_pos: None,
            mouse_relative: None,
            events_loop: events_loop,
            events: VecDeque::new(),
        })
    }

    fn wait_event(&mut self) -> Input {
        // First check for and handle any pending events.
        if let Some(event) = self.poll_event() {
            return event;
        }
        loop {
            {
                let ref mut events = self.events;
                self.events_loop.run_forever(|ev| {
                    events.push_back(ev);
                    glutin::ControlFlow::Break
                });
            }

            if let Some(event) = self.poll_event() {
                return event;
            }
        }
    }

    fn wait_event_timeout(&mut self, timeout: Duration) -> Option<Input> {
        // First check for and handle any pending events.
        if let Some(event) = self.poll_event() {
            return Some(event);
        }
        // Schedule wake up when time is out.
        let events_loop_proxy = self.events_loop.create_proxy();
        thread::spawn(move || {
            thread::sleep(timeout);
            // Wakeup can fail only if the event loop went away.
            events_loop_proxy.wakeup().ok();
        });
        {
            let ref mut events = self.events;
            self.events_loop.run_forever(|ev| {
                events.push_back(ev);
                glutin::ControlFlow::Break
            });
        }

        self.poll_event()
    }

    fn poll_event(&mut self) -> Option<Input> {
        use glutin::Event as E;
        use glutin::WindowEvent as WE;

        // Loop to skip unknown events.
        loop {
            let event = self.pre_pop_front_event();
            if event.is_some() {return event;}

            if self.events.len() == 0 {
                let ref mut events = self.events;
                self.events_loop.poll_events(|ev| events.push_back(ev));
            }
            let mut ev = self.events.pop_front();

            if self.is_capturing_cursor &&
               self.last_cursor_pos.is_none() {
                if let Some(E::WindowEvent {
                    event: WE::CursorMoved{ position: (x, y), ..}, ..
                }) = ev {
                    // Ignore this event since mouse positions
                    // should not be emitted when capturing cursor.
                    self.last_cursor_pos = Some([x, y]);

                    if self.events.len() == 0 {
                        let ref mut events = self.events;
                        self.events_loop.poll_events(|ev| events.push_back(ev));
                    }
                    ev = self.events.pop_front();
                }
            }

            let mut unknown = false;
            let event = self.handle_event(ev, &mut unknown);
            if unknown {continue};
            return event;
        }
    }

    // These events are emitted before popping a new event from the queue.
    // This is because Piston handles some events separately.
    fn pre_pop_front_event(&mut self) -> Option<Input> {
        use input::{ Input, Motion };

        // Check for a pending mouse cursor move event.
        if let Some(pos) = self.cursor_pos {
            self.cursor_pos = None;
            return Some(Input::Move(Motion::MouseCursor(pos[0], pos[1])));
        }

        // Check for a pending relative mouse move event.
        if let Some((x, y)) = self.mouse_relative {
            self.mouse_relative = None;
            return Some(Input::Move(Motion::MouseRelative(x, y)));
        }

        None
    }

    /// Convert an incoming Glutin event to Piston input.
    /// Update cursor state if necessary.
    ///
    /// The `unknown` flag is set to `true` when the event is not recognized.
    /// This is used to poll another event to make the event loop logic sound.
    /// When `unknown` is `true`, the return value is `None`.
    fn handle_event(&mut self, ev: Option<glutin::Event>, unknown: &mut bool) -> Option<Input> {
        use glutin::Event as E;
        use glutin::WindowEvent as WE;
        use glutin::MouseScrollDelta;
        use input::{ Key, Input, Motion };

        match ev {
            None => {
                if self.is_capturing_cursor {
                    self.fake_capture();
                }
                None
            }
            Some(E::WindowEvent {
                event: WE::Resized(w, h), ..
            }) => {
                let dpi_factor = self.window.hidpi_factor();
                let w = (w as f32 / dpi_factor) as u32;
                let h = (h as f32 / dpi_factor) as u32;
                Some(Input::Resize(w, h))
            },
            Some(E::WindowEvent {
                event: WE::ReceivedCharacter(ch), ..
            }) => {
                let string = match ch {
                    // Ignore control characters and return ascii for Text event (like sdl2).
                    '\u{7f}' | // Delete
                    '\u{1b}' | // Escape
                    '\u{8}'  | // Backspace
                    '\r' | '\n' | '\t' => "".to_string(),
                    _ => ch.to_string()
                };
                Some(Input::Text(string))
            },
            Some(E::WindowEvent {
                event: WE::Focused(focused), ..
            }) =>
                Some(Input::Focus(focused)),
            Some(E::WindowEvent {
                event: WE::KeyboardInput{
                    input: glutin::KeyboardInput{
                        state: glutin::ElementState::Pressed,
                        virtual_keycode: Some(key), scancode, ..
                    }, ..
                }, ..
            }) => {
                let piston_key = map_key(key);
                if let (true, Key::Escape) = (self.exit_on_esc, piston_key) {
                    self.should_close = true;
                }
                Some(Input::Button(ButtonArgs {
                    state: ButtonState::Press,
                    button: Button::Keyboard(piston_key),
                    scancode: Some(scancode as i32),
                }))
            },
            Some(E::WindowEvent {
                 event: WE::KeyboardInput{
                     input: glutin::KeyboardInput{
                         state: glutin::ElementState::Released,
                         virtual_keycode: Some(key), scancode, ..
                     }, ..
                 }, ..
             }) =>
                Some(Input::Button(ButtonArgs {
                    state: ButtonState::Release,
                    button: Button::Keyboard(map_key(key)),
                    scancode: Some(scancode as i32),
                })),
            Some(E::WindowEvent {
                event: WE::Touch(glutin::Touch { phase, location, id, .. }), ..
            }) => {
                use glutin::TouchPhase;
                use input::{Touch, TouchArgs};

                Some(Input::Move(Motion::Touch(TouchArgs::new(
                    0, id as i64, [location.0, location.1], 1.0, match phase {
                        TouchPhase::Started => Touch::Start,
                        TouchPhase::Moved => Touch::Move,
                        TouchPhase::Ended => Touch::End,
                        TouchPhase::Cancelled => Touch::Cancel
                    }
                ))))
            }
            Some(E::WindowEvent {
                event: WE::CursorMoved{position: (x, y), ..}, ..
            }) => {
                if let Some(pos) = self.last_cursor_pos {
                    let dx = x - pos[0];
                    let dy = y - pos[1];
                    if self.is_capturing_cursor {
                        self.last_cursor_pos = Some([x, y]);
                        self.fake_capture();
                        // Skip normal mouse movement and emit relative motion only.
                        return Some(Input::Move(Motion::MouseRelative(dx as f64, dy as f64)));
                    }
                    // Send relative mouse movement next time.
                    self.mouse_relative = Some((dx as f64, dy as f64));
                }

                self.last_cursor_pos = Some([x, y]);
                let f = self.window.hidpi_factor();
                let x = x as f64 / f as f64;
                let y = y as f64 / f as f64;
                Some(Input::Move(Motion::MouseCursor(x, y)))
            }
            Some(E::WindowEvent {
                event: WE::CursorEntered{..}, ..
            }) => Some(Input::Cursor(true)),
            Some(E::WindowEvent {
                event: WE::CursorLeft{..}, ..
            }) => Some(Input::Cursor(false)),
            Some(E::WindowEvent {
                event: WE::MouseWheel{delta: MouseScrollDelta::PixelDelta(x, y), ..}, ..
            }) => Some(Input::Move(Motion::MouseScroll(x as f64, y as f64))),
            Some(E::WindowEvent {
                event: WE::MouseWheel{delta: MouseScrollDelta::LineDelta(x, y), ..}, ..
            }) => Some(Input::Move(Motion::MouseScroll(x as f64, y as f64))),
            Some(E::WindowEvent {
                event: WE::MouseInput{state: glutin::ElementState::Pressed, button, ..}, ..
            }) => Some(Input::Button(ButtonArgs {
                state: ButtonState::Press,
                button: Button::Mouse(map_mouse(button)),
                scancode: None,
            })),
            Some(E::WindowEvent {
                event: WE::MouseInput{state: glutin::ElementState::Released, button, ..}, ..
            }) => Some(Input::Button(ButtonArgs {
                state: ButtonState::Release,
                button: Button::Mouse(map_mouse(button)),
                scancode: None,
            })),
            Some(E::WindowEvent { event: WE::Closed, .. }) => {
                self.should_close = true;
                Some(Input::Close(CloseArgs))
            }
            _ => {
                *unknown = true;
                None
            }
        }
    }

    fn fake_capture(&mut self) {
        if let Some(pos) = self.last_cursor_pos {
            // Fake capturing of cursor.
            let size = self.size();
            let cx = (size.width / 2) as f64;
            let cy = (size.height / 2) as f64;
            let dx = cx - pos[0];
            let dy = cy - pos[1];
            if dx != 0.0 || dy != 0.0 {
                if let Ok(_) = self.window.set_cursor_position(cx as i32, cy as i32) {
                    self.last_cursor_pos = Some([cx, cy]);
                }
            }
        }
    }
}

impl Window for GlutinWindow {
    fn size(&self) -> Size {
        let (w, h) = self.window.get_inner_size().unwrap_or((0, 0));
        let hidpi = self.window.hidpi_factor();
        ((w as f32 / hidpi) as u32, (h as f32 / hidpi) as u32).into()
    }
    fn draw_size(&self) -> Size {
        self.window.get_inner_size().unwrap_or((0, 0)).into()
    }
    fn should_close(&self) -> bool { self.should_close }
    fn set_should_close(&mut self, value: bool) { self.should_close = value; }
    fn swap_buffers(&mut self) { let _ = self.window.swap_buffers(); }
    fn wait_event(&mut self) -> Input { self.wait_event() }
    fn wait_event_timeout(&mut self, timeout: Duration) -> Option<Input> { self.wait_event_timeout(timeout) }
    fn poll_event(&mut self) -> Option<Input> { self.poll_event() }
}

impl BuildFromWindowSettings for GlutinWindow {
    fn build_from_window_settings(settings: &WindowSettings)
    -> Result<Self, String> {
        GlutinWindow::new(settings)
    }
}

impl AdvancedWindow for GlutinWindow {
    fn get_title(&self) -> String { self.title.clone() }
    fn set_title(&mut self, value: String) {
        self.title = value;
        self.window.set_title(&self.title);
    }
    fn get_exit_on_esc(&self) -> bool { self.exit_on_esc }
    fn set_exit_on_esc(&mut self, value: bool) { self.exit_on_esc = value; }
    fn set_capture_cursor(&mut self, value: bool) {
        use glutin::CursorState;

        // Normally we would call `.set_cursor_state(CursorState::Grab)`
        // but since relative mouse events does not work,
        // the capturing of cursor is faked by hiding the cursor
        // and setting the position to the center of window.
        self.is_capturing_cursor = value;
        if value {
            let _ = self.window.set_cursor_state(CursorState::Hide);
        } else {
            let _ = self.window.set_cursor_state(CursorState::Normal);
        }
        if value {
            self.fake_capture();
        }
    }
    fn show(&mut self) { self.window.show(); }
    fn hide(&mut self) { self.window.hide(); }
    fn get_position(&self) -> Option<Position> {
        self.window.get_position().map(|(x, y)|
            Position { x: x, y: y })
    }
    fn set_position<P: Into<Position>>(&mut self, pos: P) {
        let pos: Position = pos.into();
        self.window.set_position(pos.x, pos.y);
    }
    fn set_size<S: Into<Size>>(&mut self, size: S) {
        let size: Size = size.into();
        let hidpi = self.window.hidpi_factor();
        self.window.set_inner_size(
            (size.width as f32 / hidpi) as u32,
            (size.height as f32 / hidpi) as u32
        );
    }
}

impl OpenGLWindow for GlutinWindow {
    fn get_proc_address(&mut self, proc_name: &str) -> ProcAddress {
        self.window.get_proc_address(proc_name) as *const _
    }

    fn is_current(&self) -> bool {
        self.window.is_current()
    }

    fn make_current(&mut self) {
        unsafe {
            self.window.make_current().unwrap()
        }
    }
}

/// Maps Glutin's key to Piston's key.
pub fn map_key(keycode: glutin::VirtualKeyCode) -> keyboard::Key {
    use input::keyboard::Key;
    use glutin::VirtualKeyCode as K;

    match keycode {
        K::Key0 => Key::D0,
        K::Key1 => Key::D1,
        K::Key2 => Key::D2,
        K::Key3 => Key::D3,
        K::Key4 => Key::D4,
        K::Key5 => Key::D5,
        K::Key6 => Key::D6,
        K::Key7 => Key::D7,
        K::Key8 => Key::D8,
        K::Key9 => Key::D9,
        K::A => Key::A,
        K::B => Key::B,
        K::C => Key::C,
        K::D => Key::D,
        K::E => Key::E,
        K::F => Key::F,
        K::G => Key::G,
        K::H => Key::H,
        K::I => Key::I,
        K::J => Key::J,
        K::K => Key::K,
        K::L => Key::L,
        K::M => Key::M,
        K::N => Key::N,
        K::O => Key::O,
        K::P => Key::P,
        K::Q => Key::Q,
        K::R => Key::R,
        K::S => Key::S,
        K::T => Key::T,
        K::U => Key::U,
        K::V => Key::V,
        K::W => Key::W,
        K::X => Key::X,
        K::Y => Key::Y,
        K::Z => Key::Z,
        K::Apostrophe => Key::Unknown,
        K::Backslash => Key::Backslash,
        K::Back => Key::Backspace,
        // K::CapsLock => Key::CapsLock,
        K::Delete => Key::Delete,
        K::Comma => Key::Comma,
        K::Down => Key::Down,
        K::End => Key::End,
        K::Return => Key::Return,
        K::Equals => Key::Equals,
        K::Escape => Key::Escape,
        K::F1 => Key::F1,
        K::F2 => Key::F2,
        K::F3 => Key::F3,
        K::F4 => Key::F4,
        K::F5 => Key::F5,
        K::F6 => Key::F6,
        K::F7 => Key::F7,
        K::F8 => Key::F8,
        K::F9 => Key::F9,
        K::F10 => Key::F10,
        K::F11 => Key::F11,
        K::F12 => Key::F12,
        K::F13 => Key::F13,
        K::F14 => Key::F14,
        K::F15 => Key::F15,
        // K::F16 => Key::F16,
        // K::F17 => Key::F17,
        // K::F18 => Key::F18,
        // K::F19 => Key::F19,
        // K::F20 => Key::F20,
        // K::F21 => Key::F21,
        // K::F22 => Key::F22,
        // K::F23 => Key::F23,
        // K::F24 => Key::F24,
        // Possibly next code.
        // K::F25 => Key::Unknown,
        K::Numpad0 => Key::NumPad0,
        K::Numpad1 => Key::NumPad1,
        K::Numpad2 => Key::NumPad2,
        K::Numpad3 => Key::NumPad3,
        K::Numpad4 => Key::NumPad4,
        K::Numpad5 => Key::NumPad5,
        K::Numpad6 => Key::NumPad6,
        K::Numpad7 => Key::NumPad7,
        K::Numpad8 => Key::NumPad8,
        K::Numpad9 => Key::NumPad9,
        K::NumpadComma => Key::NumPadDecimal,
        K::Divide => Key::NumPadDivide,
        K::Multiply => Key::NumPadMultiply,
        K::Subtract => Key::NumPadMinus,
        K::Add => Key::NumPadPlus,
        K::NumpadEnter => Key::NumPadEnter,
        K::NumpadEquals => Key::NumPadEquals,
        K::LShift => Key::LShift,
        K::LControl => Key::LCtrl,
        K::LAlt => Key::LAlt,
        K::LMenu => Key::LGui,
        K::RShift => Key::RShift,
        K::RControl => Key::RCtrl,
        K::RAlt => Key::RAlt,
        K::RMenu => Key::RGui,
        // Map to backslash?
        // K::GraveAccent => Key::Unknown,
        K::Home => Key::Home,
        K::Insert => Key::Insert,
        K::Left => Key::Left,
        K::LBracket => Key::LeftBracket,
        // K::Menu => Key::Menu,
        K::Minus => Key::Minus,
        K::Numlock => Key::NumLockClear,
        K::PageDown => Key::PageDown,
        K::PageUp => Key::PageUp,
        K::Pause => Key::Pause,
        K::Period => Key::Period,
        // K::PrintScreen => Key::PrintScreen,
        K::Right => Key::Right,
        K::RBracket => Key::RightBracket,
        // K::ScrollLock => Key::ScrollLock,
        K::Semicolon => Key::Semicolon,
        K::Slash => Key::Slash,
        K::Space => Key::Space,
        K::Tab => Key::Tab,
        K::Up => Key::Up,
        // K::World1 => Key::Unknown,
        // K::World2 => Key::Unknown,
        _ => Key::Unknown,
    }
}

/// Maps Glutin's mouse button to Piston's mouse button.
pub fn map_mouse(mouse_button: glutin::MouseButton) -> MouseButton {
    use glutin::MouseButton as M;

    match mouse_button {
        M::Left => MouseButton::Left,
        M::Right => MouseButton::Right,
        M::Middle => MouseButton::Middle,
        M::Other(0) => MouseButton::X1,
        M::Other(1) => MouseButton::X2,
        M::Other(2) => MouseButton::Button6,
        M::Other(3) => MouseButton::Button7,
        M::Other(4) => MouseButton::Button8,
        _ => MouseButton::Unknown
    }
}
