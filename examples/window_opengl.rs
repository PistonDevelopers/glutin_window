
extern crate glutin_window;
extern crate shader_version;
extern crate window;

use glutin_window::GlutinWindow;
use shader_version::OpenGL;
use window::WindowSettings;

fn main() {
    let _ = GlutinWindow::new(
        &WindowSettings::new("Glutin Window", (640, 480))
            .fullscreen(false)
            .vsync(true)
            .opengl(OpenGL::V2_1) // etc
    ).unwrap();
}
