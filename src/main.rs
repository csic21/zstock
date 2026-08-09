pub mod controller;
pub mod domain;
pub mod features;
pub mod infrastructure;
pub mod services;

mod app;
mod chart;
mod data;
mod model;
mod notifications;
mod storage;
mod update;

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod mac_gesture;
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod mac_status_bar;

fn main() {
    app::run();
}
