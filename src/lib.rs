#![deny(missing_docs)]

//! A Glutin window back-end for the Piston game engine.

extern crate glutin;
extern crate gl;
extern crate glutin_winit;
extern crate input;
extern crate raw_window_handle;
extern crate window;
extern crate winit;
extern crate shader_version;
extern crate rustc_hash;

use rustc_hash::FxHashMap;

use std::collections::VecDeque;
use std::error::Error;

// External crates.
use input::{
    ButtonArgs,
    ButtonState,
    CloseArgs,
    Event,
    Key,
    Motion,
    MouseButton,
    Button,
    Input,
    ResizeArgs,
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
    Api,
    UnsupportedGraphicsApiError,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event_loop::{ActiveEventLoop, EventLoop},
    event::{DeviceId, ElementState, MouseScrollDelta, WindowEvent},
    window::WindowId,
};
use glutin::context::PossiblyCurrentGlContext;
use glutin::display::GlDisplay;
use glutin::prelude::GlSurface;
use std::time::Duration;
use std::sync::Arc;

pub use shader_version::OpenGL;


/// Settings for whether to ignore modifiers and use standard keyboard layouts instead.
///
/// This does not affect `piston::input::TextEvent`.
///
/// Piston uses the same key codes as in SDL2.
/// The problem is that without knowing the keyboard layout,
/// there is no coherent way of generating key codes.
///
/// This option choose different tradeoffs depending on need.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum KeyboardIgnoreModifiers {
    /// Keep the key codes that are affected by modifiers.
    ///
    /// This is a good default for most applications.
    /// However, make sure to display understandable information to the user.
    ///
    /// If you experience user problems among gamers,
    /// then you might consider allowing other options in your game engine.
    /// Some gamers might be used to how stuff works in other traditional game engines
    /// and struggle understanding this configuration, depending on how you use keyboard layout.
    None,
    /// Assume the user's keyboard layout is standard English ABC.
    ///
    /// In some non-English speaking countries, this might be more user friendly for some gamers.
    ///
    /// This might sound counter-intuitive at first, so here is the reason:
    ///
    /// Gamers can customize their keyboard layout without needing to understand scan codes.
    /// When gamers want physically accuracy with good default options,
    /// they can simply use standard English ABC.
    ///
    /// In other cases, this option displays understandable information for game instructions.
    /// This information makes it easier for users to correct the problem themselves.
    ///
    /// Most gaming consoles use standard controllers.
    /// Typically, the only device that might be problematic for users is the keyboard.
    /// Instead of solving this problem in your game engine, let users do it in the OS.
    ///
    /// This option gives more control to users and is also better for user data privacy.
    /// Detecting keyboard layout is usually not needed.
    /// Instead, provide options for the user where they can modify the keys.
    /// If users want to switch layout in the middle of a game, they can do it through the OS.
    AbcKeyCode,
}

/// Contains stuff for game window.
pub struct GlutinWindow {
    /// The OpenGL context.
    pub ctx: Option<glutin::context::PossiblyCurrentContext>,
    /// The window surface.
    pub surface: Option<glutin::surface::Surface<glutin::surface::WindowSurface>>,
    /// The graphics display.
    pub display: Option<glutin::display::Display>,
    /// The event loop of the window.
    ///
    /// This is optional because when pumping events using `ApplicationHandler`,
    /// the event loop can not be owned by `WinitWindow`.
    pub event_loop: Option<EventLoop<UserEvent>>,
    /// Sets keyboard layout.
    ///
    /// When set, the key codes are
    pub keyboard_ignore_modifiers: KeyboardIgnoreModifiers,
    /// The Winit window.
    ///
    /// This is optional because when creating the window,
    /// it is only accessible by `ActiveEventLoop::create_window`,
    /// which in turn requires `ApplicationHandler`.
    /// One call to `Window::pull_event` is needed to trigger
    /// Winit to call `ApplicationHandler::request_redraw`,
    /// which creates the window.
    pub window: Option<Arc<winit::window::Window>>,
    /// Keeps track of connected devices.
    pub devices: u32,
    /// Maps device id to a unique id used by Piston.
    pub device_id_map: FxHashMap<DeviceId, u32>,
    // The window settings that created the window.
    settings: WindowSettings,
    // The back-end does not remember the title.
    title: String,
    exit_on_esc: bool,
    should_close: bool,
    automatic_close: bool,
    // Used to fake capturing of cursor,
    // to get relative mouse events.
    is_capturing_cursor: bool,
    // Stores the last known cursor position.
    last_cursor_pos: Option<[f64; 2]>,
    // Stores relative coordinates to emit on next poll.
    mouse_relative: Option<(f64, f64)>,
    // Used to emit cursor event after enter/leave.
    cursor_pos: Option<[f64; 2]>,
    // Used to filter repeated key presses (does not affect text repeat).
    last_key_pressed: Option<input::Key>,
    // Stores list of events ready for processing.
    events: VecDeque<Event>,
}

fn graphics_api_from_settings(settings: &WindowSettings) -> Result<Api, Box<dyn Error>> {
    let api = settings.get_maybe_graphics_api().unwrap_or(Api::opengl(3, 2));
    if api.api != "OpenGL" {
        return Err(UnsupportedGraphicsApiError {
            found: api.api,
            expected: vec!["OpenGL".into()]
        }.into());
    };
    Ok(api)
}

fn surface_attributes_builder_from_settings(
    settings: &WindowSettings
) -> glutin::surface::SurfaceAttributesBuilder<glutin::surface::WindowSurface> {
    glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
        .with_srgb(Some(settings.get_srgb()))
}

fn config_template_builder_from_settings(
    settings: &WindowSettings
) -> glutin::config::ConfigTemplateBuilder {
    let x = glutin::config::ConfigTemplateBuilder::new()
        .with_transparency(settings.get_transparent());
    let samples = settings.get_samples();
    if samples == 0 {x} else {
        x.with_multisampling(samples)
    }
}

impl GlutinWindow {

    /// Creates a new game window for Glutin.
    pub fn new(settings: &WindowSettings) -> Result<Self, Box<dyn Error>> {
        let event_loop = winit::event_loop::EventLoop::with_user_event().build()?;
        Self::from_event_loop(settings, event_loop)
    }

    /// Creates a game window from a pre-existing Glutin event loop.
    pub fn from_event_loop(
        settings: &WindowSettings,
        event_loop: winit::event_loop::EventLoop<UserEvent>,
    ) -> Result<Self, Box<dyn Error>> {
        let title = settings.get_title();
        let exit_on_esc = settings.get_exit_on_esc();

        let mut w = GlutinWindow {
            ctx: None,
            display: None,
            surface: None,
            window: None,
            title,
            exit_on_esc,
            settings: settings.clone(),
            should_close: false,
            automatic_close: settings.get_automatic_close(),
            cursor_pos: None,
            is_capturing_cursor: false,
            last_cursor_pos: None,
            mouse_relative: None,
            last_key_pressed: None,
            event_loop: Some(event_loop),
            keyboard_ignore_modifiers: KeyboardIgnoreModifiers::None,
            events: VecDeque::new(),

            devices: 0,
            device_id_map: FxHashMap::default(),
        };
        // Causes the window to be created through `ApplicationHandler::request_redraw`.
        if let Some(e) = w.poll_event() {w.events.push_front(e)}
        Ok(w)
    }

    /// Gets a reference to the window.
    ///
    /// This is faster than [get_window], but borrows self.
    pub fn get_window_ref(&self) -> &winit::window::Window {
        self.window.as_ref().unwrap()
    }

    /// Returns a cloned smart pointer to the underlying Winit window.
    pub fn get_window(&self) -> Arc<winit::window::Window> {
        self.window.as_ref().unwrap().clone()
    }

    // These events are emitted before popping a new event from the queue.
    // This is because Piston handles some events separately.
    fn pre_pop_front_event(&mut self) -> Option<Input> {
        use input::Motion;

        // Check for a pending mouse cursor move event.
        if let Some(pos) = self.cursor_pos {
            self.cursor_pos = None;
            return Some(Input::Move(Motion::MouseCursor(pos)));
        }

        // Check for a pending relative mouse move event.
        if let Some((x, y)) = self.mouse_relative {
            self.mouse_relative = None;
            return Some(Input::Move(Motion::MouseRelative([x, y])));
        }

        None
    }

    /// Convert an incoming Winit event to Piston input.
    /// Update cursor state if necessary.
    ///
    /// The `unknown` flag is set to `true` when the event is not recognized.
    /// This is used to poll another event to make the event loop logic sound.
    /// When `unknown` is `true`, the return value is `None`.
    fn handle_event(
        &mut self,
        event: winit::event::WindowEvent,
        unknown: &mut bool,
    ) -> Option<Input> {
        use winit::keyboard::{Key, NamedKey};

        match event {
            WindowEvent::KeyboardInput { event: ref ev, .. } => {
                if self.exit_on_esc {
                    if let Key::Named(NamedKey::Escape) = ev.logical_key {
                        self.set_should_close(true);
                        return None;
                    }
                }
                if let Some(s) = &ev.text {
                    let s = s.to_string();
                    let repeat = ev.repeat;
                    if !repeat {
                        if let Some(input) = map_window_event(
                            event,
                            self.get_window_ref().scale_factor(),
                            self.keyboard_ignore_modifiers,
                            unknown,
                            &mut self.last_key_pressed,
                            &mut self.devices,
                            &mut self.device_id_map,
                        ) {
                            self.events.push_back(Event::Input(input, None));
                        }
                    }

                    return Some(Input::Text(s));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.get_window_ref().scale_factor();
                let position = position.to_logical::<f64>(scale);
                let x = f64::from(position.x);
                let y = f64::from(position.y);

                let pre_event = self.pre_pop_front_event();
                let mut input = || {
                    if let Some(pos) = self.last_cursor_pos {
                        let dx = x - pos[0];
                        let dy = y - pos[1];
                        if self.is_capturing_cursor {
                            self.last_cursor_pos = Some([x, y]);
                            self.fake_capture();
                            // Skip normal mouse movement and emit relative motion only.
                            return Some(Input::Move(Motion::MouseRelative([dx as f64, dy as f64])));
                        }
                        // Send relative mouse movement next time.
                        self.mouse_relative = Some((dx as f64, dy as f64));
                    } else if self.is_capturing_cursor {
                        // Ignore this event since mouse positions
                        // should not be emitted when capturing cursor.
                        self.last_cursor_pos = Some([x, y]);
                        return None;
                    }

                    self.last_cursor_pos = Some([x, y]);
                    return Some(Input::Move(Motion::MouseCursor([x, y])))
                };

                let input = input();
                return if pre_event.is_some() {
                    if let Some(input) = input {
                        self.events.push_back(Event::Input(input, None));
                    }
                    pre_event
                } else {input}
            }
            _ => {}
        }

        // Usual events are handled here and passed to user.
        let input = map_window_event(
            event,
            self.get_window_ref().scale_factor(),
            self.keyboard_ignore_modifiers,
            unknown,
            &mut self.last_key_pressed,
            &mut self.devices,
            &mut self.device_id_map,
        );

        let pre_event = self.pre_pop_front_event();
        if pre_event.is_some() {
            if let Some(input) = input {
                self.events.push_back(Event::Input(input, None));
            }
            pre_event
        } else {input}
    }

    fn fake_capture(&mut self) {
        if let Some(pos) = self.last_cursor_pos {
            // Fake capturing of cursor.
            let size = self.size();
            let cx = size.width / 2.0;
            let cy = size.height / 2.0;
            let dx = cx - pos[0];
            let dy = cy - pos[1];
            if dx != 0.0 || dy != 0.0 {
                let pos = winit::dpi::LogicalPosition::new(cx, cy);
                if let Ok(_) = self.get_window_ref().set_cursor_position(pos) {
                    self.last_cursor_pos = Some([cx, cy]);
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for GlutinWindow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        use glutin::display::GetGlDisplay;
        use glutin::config::GlConfig;
        use glutin::context::ContextApi;
        use glutin::context::NotCurrentGlContext;
        use raw_window_handle::HasRawWindowHandle;
        use std::num::NonZeroU32;

        let settings = &self.settings;

        let template = config_template_builder_from_settings(settings);
        let display_builder = glutin_winit::DisplayBuilder::new();
        let (_, gl_config) = display_builder
            .build(event_loop, template, |configs| {
                configs.reduce(|accum, config| {
                    let transparency_check = config.supports_transparency().unwrap_or(false)
                        & !accum.supports_transparency().unwrap_or(false);

                    if transparency_check || config.num_samples() > accum.num_samples() {
                        config
                    } else {
                        accum
                    }
                })
                .unwrap()
            }).unwrap();

        let window = event_loop.create_window(winit::window::Window::default_attributes()
            .with_inner_size(LogicalSize::<f64>::new(
                settings.get_size().width.into(),
                settings.get_size().height.into(),
            ))
            .with_title(settings.get_title())
        ).unwrap();

        let raw_window_handle = window.raw_window_handle().unwrap();
        let draw_size = window.inner_size();
        let dw = NonZeroU32::new(draw_size.width).unwrap();
        let dh = NonZeroU32::new(draw_size.height).unwrap();
        let surface_attributes = surface_attributes_builder_from_settings(settings)
            .build(raw_window_handle, dw, dh);

        let display: glutin::display::Display = gl_config.display();
        let surface = unsafe {display.create_window_surface(&gl_config, &surface_attributes).unwrap()};

        let api = graphics_api_from_settings(settings).unwrap();
        let context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(Some(glutin::context::Version::new(api.major as u8, api.minor as u8))))
            .build(Some(raw_window_handle));

        let fallback_context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(Some(raw_window_handle));

        let legacy_context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(Some(glutin::context::Version::new(2, 1))))
            .build(Some(raw_window_handle));

        let mut not_current_gl_context = Some(unsafe {
            if let Ok(x) = display.create_context(&gl_config, &context_attributes) {x}
            else if let Ok(x) = display.create_context(&gl_config, &fallback_context_attributes) {x}
            else {
                display.create_context(&gl_config, &legacy_context_attributes).unwrap()
            }
        });

        let ctx: glutin::context::PossiblyCurrentContext = not_current_gl_context.take().unwrap()
            .make_current(&surface).unwrap();

        if settings.get_vsync() {
            surface.set_swap_interval(&ctx,
                glutin::surface::SwapInterval::Wait(NonZeroU32::new(1).unwrap())).unwrap();
        }

        // Load the OpenGL function pointers.
        gl::load_with(|s| {
            use std::ffi::CString;

            let s = CString::new(s).expect("CString::new failed");
            display.get_proc_address(&s) as *const _
        });

        self.ctx = Some(ctx);
        self.surface = Some(surface);
        self.display = Some(display);
        self.window = Some(Arc::new(window));
    }

    fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            event: WindowEvent,
        ) {
            let window =  &self.get_window_ref();

            match event {
                WindowEvent::CloseRequested => {
                    if self.automatic_close {
                        self.should_close = true;
                        event_loop.exit();
                    }
                }
                WindowEvent::RedrawRequested => {
                    window.request_redraw();
                },
                event => {
                    let mut unknown = false;
                    if let Some(ev) = self.handle_event(event, &mut unknown) {
                        if !unknown {
                            self.events.push_back(Event::Input(ev, None));
                        }
                    }
                }
            }
        }
}

impl Window for GlutinWindow {
    fn size(&self) -> Size {
        let window = self.get_window_ref();
        let (w, h): (u32, u32) = window.inner_size().into();
        let hidpi = window.scale_factor();
        ((w as f64 / hidpi) as u32, (h as f64 / hidpi) as u32).into()
    }

    fn should_close(&self) -> bool { self.should_close }

    fn set_should_close(&mut self, value: bool) { self.should_close = value; }

    fn swap_buffers(&mut self) {
        if let (Some(ctx), Some(surface)) = (&self.ctx, &self.surface) {
            let _ = surface.swap_buffers(ctx);
        }
    }

    fn wait_event(&mut self) -> Event {
        use winit::platform::pump_events::EventLoopExtPumpEvents;
        use input::{IdleArgs, Loop};

        // Add all events we got to the event queue, since winit only allows us to get all pending
        //  events at once.
        if let Some(mut event_loop) = std::mem::replace(&mut self.event_loop, None) {
            let event_loop_proxy = event_loop.create_proxy();
            event_loop_proxy
                .send_event(UserEvent::WakeUp)
                .expect("Event loop is closed before property handling all events.");
            event_loop.pump_app_events(None, self);
            self.event_loop = Some(event_loop);
        }

        // Get the first event in the queue
        let event = self.events.pop_front();

        // Check if we got a close event, if we did we need to mark ourselves as should-close
        if let &Some(Event::Input(Input::Close(_), ..)) = &event {
            self.set_should_close(true);
        }

        event.unwrap_or(Event::Loop(Loop::Idle(IdleArgs {dt: 0.0})))
    }
    fn wait_event_timeout(&mut self, timeout: Duration) -> Option<Event> {
        use winit::platform::pump_events::EventLoopExtPumpEvents;

        // Add all events we got to the event queue, since winit only allows us to get all pending
        //  events at once.
        if let Some(mut event_loop) = std::mem::replace(&mut self.event_loop, None) {
            let event_loop_proxy = event_loop.create_proxy();
            event_loop_proxy
                .send_event(UserEvent::WakeUp)
                .expect("Event loop is closed before property handling all events.");
            event_loop.pump_app_events(Some(timeout), self);
            self.event_loop = Some(event_loop);
        }

        // Get the first event in the queue
        let event = self.events.pop_front();

        // Check if we got a close event, if we did we need to mark ourselves as should-close
        if let &Some(Event::Input(Input::Close(_), ..)) = &event {
            self.set_should_close(true);
        }

        event
    }
    fn poll_event(&mut self) -> Option<Event> {
        use winit::platform::pump_events::EventLoopExtPumpEvents;

        // Add all events we got to the event queue, since winit only allows us to get all pending
        //  events at once.
        if let Some(mut event_loop) = std::mem::replace(&mut self.event_loop, None) {
           let event_loop_proxy = event_loop.create_proxy();
           event_loop_proxy
               .send_event(UserEvent::WakeUp)
               .expect("Event loop is closed before property handling all events.");
           event_loop.pump_app_events(Some(Duration::ZERO), self);
           self.event_loop = Some(event_loop);
        }

        // Get the first event in the queue
        let event = self.events.pop_front();

        // Check if we got a close event, if we did we need to mark ourselves as should-close
        if let &Some(Event::Input(Input::Close(_), ..)) = &event {
           self.set_should_close(true);
        }

        event
     }

    fn draw_size(&self) -> Size {
        let size: (f64, f64) = self.get_window_ref().inner_size().into();
        size.into()
    }
}

impl BuildFromWindowSettings for GlutinWindow {
    fn build_from_window_settings(settings: &WindowSettings)
    -> Result<Self, Box<dyn Error>> {
        GlutinWindow::new(settings)
    }
}

impl AdvancedWindow for GlutinWindow {
    fn get_title(&self) -> String {
        self.title.clone()
    }

    fn set_title(&mut self, value: String) {
        self.get_window_ref().set_title(&value);
        self.title = value;
    }

    fn get_exit_on_esc(&self) -> bool {
        self.exit_on_esc
    }

    fn set_exit_on_esc(&mut self, value: bool) {
        self.exit_on_esc = value
    }

    fn set_capture_cursor(&mut self, value: bool) {
        // Normally we would call `.set_cursor_grab`
        // but since relative mouse events does not work,
        // because device deltas have unspecified coordinates,
        // the capturing of cursor is faked by hiding the cursor
        // and setting the position to the center of window.
        self.is_capturing_cursor = value;
        self.get_window_ref().set_cursor_visible(!value);
        if value {
            self.fake_capture();
        }
    }

    fn get_automatic_close(&self) -> bool {self.automatic_close}

    fn set_automatic_close(&mut self, value: bool) {self.automatic_close = value}

    fn show(&mut self) {
        self.get_window_ref().set_visible(true);
    }

    fn hide(&mut self) {
        self.get_window_ref().set_visible(false);
    }

    fn get_position(&self) -> Option<Position> {
        self.get_window_ref()
            .outer_position()
            .map(|p| Position { x: p.x, y: p.y })
            .ok()
    }

    fn set_position<P: Into<Position>>(&mut self, val: P) {
        let val = val.into();
        self.get_window_ref()
            .set_outer_position(LogicalPosition::new(val.x as f64, val.y as f64))
    }

    fn set_size<S: Into<Size>>(&mut self, size: S) {
        let size: Size = size.into();
        let w = self.get_window_ref();
        let _ = w.request_inner_size(LogicalSize::new(
            size.width as f64,
            size.height as f64,
        ));
    }
}

impl OpenGLWindow for GlutinWindow {
    fn get_proc_address(&mut self, proc_name: &str) -> ProcAddress {
        use std::ffi::CString;

        let s = CString::new(proc_name).expect("CString::new failed");
        self.display.as_ref().expect("No display").get_proc_address(&s) as *const _
    }

    fn is_current(&self) -> bool {
        if let Some(ctx) = &self.ctx {
            ctx.is_current()
        } else {false}
    }

    fn make_current(&mut self) {
        if let (Some(ctx), Some(surface)) = (&self.ctx, &self.surface) {
            let _ = ctx.make_current(surface);
        }
    }
}

fn map_key(input: &winit::event::KeyEvent, kim: KeyboardIgnoreModifiers) -> Key {
    use winit::keyboard::NamedKey::*;
    use winit::keyboard::Key::*;
    use KeyboardIgnoreModifiers as KIM;

    match input.logical_key {
        Character(ref ch) => match ch.as_str() {
            "0" | ")" if kim == KIM::AbcKeyCode => Key::D0,
            "0" => Key::D0,
            ")" => Key::RightParen,
            "1" | "!" if kim == KIM::AbcKeyCode => Key::D1,
            "1" => Key::D1,
            "!" => Key::NumPadExclam,
            "2" | "@" if kim == KIM::AbcKeyCode => Key::D2,
            "2" => Key::D2,
            "@" => Key::At,
            "3" | "#" if kim == KIM::AbcKeyCode => Key::D3,
            "3" => Key::D3,
            "#" => Key::Hash,
            "4" | "$" if kim == KIM::AbcKeyCode => Key::D4,
            "4" => Key::D4,
            "$" => Key::Dollar,
            "5" | "%" if kim == KIM::AbcKeyCode => Key::D5,
            "5" => Key::D5,
            "%" => Key::Percent,
            "6" | "^" if kim == KIM::AbcKeyCode => Key::D6,
            "6" => Key::D6,
            "^" => Key::Caret,
            "7" | "&" if kim == KIM::AbcKeyCode => Key::D7,
            "7" => Key::D7,
            "&" => Key::Ampersand,
            "8" | "*" if kim == KIM::AbcKeyCode => Key::D8,
            "8" => Key::D8,
            "*" => Key::Asterisk,
            "9" | "(" if kim == KIM::AbcKeyCode => Key::D9,
            "9" => Key::D9,
            "(" => Key::LeftParen,
            "a" | "A" => Key::A,
            "b" | "B" => Key::B,
            "c" | "C" => Key::C,
            "d" | "D" => Key::D,
            "e" | "E" => Key::E,
            "f" | "F" => Key::F,
            "g" | "G" => Key::G,
            "h" | "H" => Key::H,
            "i" | "I" => Key::I,
            "j" | "J" => Key::J,
            "k" | "K" => Key::K,
            "l" | "L" => Key::L,
            "m" | "M" => Key::M,
            "n" | "N" => Key::N,
            "o" | "O" => Key::O,
            "p" | "P" => Key::P,
            "q" | "Q" => Key::Q,
            "r" | "R" => Key::R,
            "s" | "S" => Key::S,
            "t" | "T" => Key::T,
            "u" | "U" => Key::U,
            "v" | "V" => Key::V,
            "w" | "W" => Key::W,
            "x" | "X" => Key::X,
            "y" | "Y" => Key::Y,
            "z" | "Z" => Key::Z,
            "'" | "\"" if kim == KIM::AbcKeyCode => Key::Quote,
            "'" => Key::Quote,
            "\"" => Key::Quotedbl,
            ";" | ":" if kim == KIM::AbcKeyCode => Key::Semicolon,
            ";" => Key::Semicolon,
            ":" => Key::Colon,
            "[" | "{" if kim == KIM::AbcKeyCode => Key::LeftBracket,
            "[" => Key::LeftBracket,
            "{" => Key::NumPadLeftBrace,
            "]" | "}" if kim == KIM::AbcKeyCode => Key::RightBracket,
            "]" => Key::RightBracket,
            "}" => Key::NumPadRightBrace,
            "\\" | "|" if kim == KIM::AbcKeyCode => Key::Backslash,
            "\\" => Key::Backslash,
            "|" => Key::NumPadVerticalBar,
            "," | "<" if kim == KIM::AbcKeyCode => Key::Comma,
            "," => Key::Comma,
            "<" => Key::Less,
            "." | ">" if kim == KIM::AbcKeyCode => Key::Period,
            "." => Key::Period,
            ">" => Key::Greater,
            "/" | "?" if kim == KIM::AbcKeyCode => Key::Slash,
            "/" => Key::Slash,
            "?" => Key::Question,
            "`" | "~" if kim == KIM::AbcKeyCode => Key::Backquote,
            "`" => Key::Backquote,
            // Piston v1.0 does not support `~` using modifier.
            // Use `KeyboardIgnoreModifiers::AbcKeyCode` on window to fix this issue.
            // It will be mapped to `Key::Backquote`.
            "~" => Key::Unknown,
            _ => Key::Unknown,
        }
        Named(Escape) => Key::Escape,
        Named(F1) => Key::F1,
        Named(F2) => Key::F2,
        Named(F3) => Key::F3,
        Named(F4) => Key::F4,
        Named(F5) => Key::F5,
        Named(F6) => Key::F6,
        Named(F7) => Key::F7,
        Named(F8) => Key::F8,
        Named(F9) => Key::F9,
        Named(F10) => Key::F10,
        Named(F11) => Key::F11,
        Named(F12) => Key::F12,
        Named(F13) => Key::F13,
        Named(F14) => Key::F14,
        Named(F15) => Key::F15,

        Named(Delete) => Key::Delete,

        Named(ArrowLeft) => Key::Left,
        Named(ArrowUp) => Key::Up,
        Named(ArrowRight) => Key::Right,
        Named(ArrowDown) => Key::Down,

        Named(Backspace) => Key::Backspace,
        Named(Enter) => Key::Return,
        Named(Space) => Key::Space,

        Named(Alt) => Key::LAlt,
        Named(AltGraph) => Key::RAlt,
        Named(Control) => Key::LCtrl,
        Named(Super) => Key::Menu,
        Named(Shift) => Key::LShift,

        Named(Tab) => Key::Tab,
        _ => Key::Unknown,
    }
}

fn map_keyboard_input(
    input: &winit::event::KeyEvent,
    kim: KeyboardIgnoreModifiers,
    unknown: &mut bool,
    last_key_pressed: &mut Option<Key>,
) -> Option<Input> {
    let key = map_key(input, kim);

    let state = if input.state == ElementState::Pressed {
        // Filter repeated key presses (does not affect text repeat when holding keys).
        if let Some(last_key) = &*last_key_pressed {
            if last_key == &key {
                *unknown = true;
                return None;
            }
        }
        *last_key_pressed = Some(key);

        ButtonState::Press
    } else {
        if let Some(last_key) = &*last_key_pressed {
            if last_key == &key {
                *last_key_pressed = None;
            }
        }
        ButtonState::Release
    };

    Some(Input::Button(ButtonArgs {
        state: state,
        button: Button::Keyboard(key),
        scancode: if let winit::keyboard::PhysicalKey::Code(code) = input.physical_key {
                Some(code as i32)
            } else {None},
    }))
}

/// Maps Glutin's mouse button to Piston's mouse button.
pub fn map_mouse(mouse_button: winit::event::MouseButton) -> MouseButton {
    use winit::event::MouseButton as M;

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

/// Converts a winit's [`WindowEvent`] into a piston's [`Input`].
///
/// For some events that will not be passed to the user, returns `None`.
fn map_window_event(
    window_event: WindowEvent,
    scale_factor: f64,
    kim: KeyboardIgnoreModifiers,
    unknown: &mut bool,
    last_key_pressed: &mut Option<Key>,
    devices: &mut u32,
    device_id_map: &mut FxHashMap<DeviceId, u32>,
) -> Option<Input> {
    use input::FileDrag;

    match window_event {
        WindowEvent::DroppedFile(path) =>
            Some(Input::FileDrag(FileDrag::Drop(path))),
        WindowEvent::HoveredFile(path) =>
            Some(Input::FileDrag(FileDrag::Hover(path))),
        WindowEvent::HoveredFileCancelled =>
            Some(Input::FileDrag(FileDrag::Cancel)),
        WindowEvent::Resized(size) => Some(Input::Resize(ResizeArgs {
            window_size: [size.width as f64, size.height as f64],
            draw_size: Size {
                width: size.width as f64,
                height: size.height as f64,
            }
            .into(),
        })),
        WindowEvent::CloseRequested => Some(Input::Close(CloseArgs)),
        WindowEvent::Destroyed => Some(Input::Close(CloseArgs)),
        WindowEvent::Focused(focused) => Some(Input::Focus(focused)),
        WindowEvent::KeyboardInput { ref event, .. } => {
            map_keyboard_input(event, kim, unknown, last_key_pressed)
        }
        WindowEvent::CursorMoved { position, .. } => {
            let position = position.to_logical(scale_factor);
            Some(Input::Move(Motion::MouseCursor([position.x, position.y])))
        }
        WindowEvent::CursorEntered { .. } => Some(Input::Cursor(true)),
        WindowEvent::CursorLeft { .. } => Some(Input::Cursor(false)),
        WindowEvent::MouseWheel { delta, .. } => match delta {
            MouseScrollDelta::PixelDelta(position) => {
                let position = position.to_logical(scale_factor);
                Some(Input::Move(Motion::MouseScroll([position.x, position.y])))
            }
            MouseScrollDelta::LineDelta(x, y) =>
                Some(Input::Move(Motion::MouseScroll([x as f64, y as f64]))),
        },
        WindowEvent::MouseInput { state, button, .. } => {
            let button = map_mouse(button);
            let state = match state {
                ElementState::Pressed => ButtonState::Press,
                ElementState::Released => ButtonState::Release,
            };

            Some(Input::Button(ButtonArgs {
                state,
                button: Button::Mouse(button),
                scancode: None,
            }))
        }
        WindowEvent::AxisMotion { device_id, axis, value } => {
            use input::ControllerAxisArgs;

            Some(Input::Move(Motion::ControllerAxis(ControllerAxisArgs::new(
                {
                    if let Some(id) = device_id_map.get(&device_id) {*id}
                    else {
                        let id = *devices;
                        *devices += 1;
                        device_id_map.insert(device_id, id);
                        id
                    }
                },
                axis as u8,
                value,
            ))))
        }
        WindowEvent::Touch(winit::event::Touch { phase, location, id, .. }) => {
            use winit::event::TouchPhase;
            use input::{Touch, TouchArgs};

            let location = location.to_logical::<f64>(scale_factor);

            Some(Input::Move(Motion::Touch(TouchArgs::new(
                0, id as i64, [location.x, location.y], 1.0, match phase {
                    TouchPhase::Started => Touch::Start,
                    TouchPhase::Moved => Touch::Move,
                    TouchPhase::Ended => Touch::End,
                    TouchPhase::Cancelled => Touch::Cancel
                }
            ))))
        }
        // Events not built-in by Piston v1.0.
        // It is possible to use Piston's `Event::Custom`.
        // This might be added as a library in the future to Piston's ecosystem.
        WindowEvent::TouchpadPressure { .. } |
        WindowEvent::PinchGesture { .. } |
        WindowEvent::RotationGesture { .. } |
        WindowEvent::PanGesture { .. } |
        WindowEvent::DoubleTapGesture { .. } => None,
        WindowEvent::ScaleFactorChanged { .. } => None,
        WindowEvent::ActivationTokenDone { .. } => None,
        WindowEvent::ThemeChanged(_) => None,
        WindowEvent::Ime(_) => None,
        WindowEvent::Occluded(_) => None,
        WindowEvent::RedrawRequested { .. } => None,
        WindowEvent::Moved(_) => None,
        WindowEvent::ModifiersChanged(_) => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
/// Custom events for the glutin event loop
pub enum UserEvent {
    /// Do nothing, just spin the event loop
    WakeUp,
}
