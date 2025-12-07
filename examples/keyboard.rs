
extern crate glutin_window;
extern crate shader_version;
extern crate window;
extern crate piston;

use glutin_window::GlutinWindow;
use shader_version::OpenGL;
use window::WindowSettings;

use piston::*;

fn main() {
    let mut window = GlutinWindow::new(
        &WindowSettings::new("Glutin Window", (640, 480))
            .fullscreen(false)
            .vsync(true)
            .graphics_api(OpenGL::V2_1) // etc
    ).unwrap();

    let mut events = Events::new(EventSettings::new());
    while let Some(e) = events.next(&mut window) {
        if let Some(button) = e.press_args() {
            println!("{:?}", button);
        }
    }
}
