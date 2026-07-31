//! macOS trackpad pinch (magnify) → channel, since GPUI 0.2 does not forward NSEventTypeMagnify.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;

use block::ConcreteBlock;
use cocoa::appkit::{NSEvent, NSEventMask, NSEventType};
use cocoa::base::{id, nil};
use objc::{class, msg_send, sel, sel_impl};

static PINCH_TX: OnceLock<Sender<f32>> = OnceLock::new();

/// Install a process-local magnify monitor. Call once from the main/UI thread.
/// Returns a receiver of relative magnification deltas (positive ≈ zoom in / fingers apart).
pub fn install_pinch_receiver() -> Receiver<f32> {
    let (tx, rx) = mpsc::channel::<f32>();
    let _ = PINCH_TX.set(tx);
    install_monitor();
    rx
}

fn install_monitor() {
    unsafe {
        let mask = NSEventMask::from_type(NSEventType::NSEventTypeMagnify);
        let block = ConcreteBlock::new(move |event: id| -> id {
            if event == nil {
                return event;
            }
            // Relative scale change for this sample of the gesture.
            let mag: f64 = NSEvent::magnification(event);
            if mag.abs() > 1e-6 {
                if let Some(tx) = PINCH_TX.get() {
                    let _ = tx.send(mag as f32);
                }
            }
            event
        });
        let block = block.copy();
        let _: id = msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: mask
            handler: &*block
        ];
        // Keep block alive for the lifetime of the process.
        std::mem::forget(block);
    }
}
