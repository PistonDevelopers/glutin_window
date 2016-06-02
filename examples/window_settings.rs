
extern crate glutin_window;
extern crate window;

use glutin_window::GlutinWindow;
use window::WindowSettings;

fn main() {
    let _ = GlutinWindow::new(
        &WindowSettings::new("Glutin Window", (640, 480))
            .fullscreen(false)
            .vsync(true) // etc
    ).unwrap();
}
