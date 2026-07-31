mod app;
mod chart;
mod data;
mod model;
mod storage;

#[cfg(target_os = "macos")]
mod mac_gesture;

fn main() {
    app::run();
}
