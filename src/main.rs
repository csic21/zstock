mod app;
mod chart;
mod data;
mod model;
mod notifications;
mod storage;
mod update;

#[cfg(target_os = "macos")]
mod mac_gesture;
#[cfg(target_os = "macos")]
mod mac_status_bar;

fn main() {
    app::run();
}
