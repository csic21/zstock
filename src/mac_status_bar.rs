//! macOS menu bar (NSStatusItem) for live watchlist quotes.
//!
//! Install once on the main thread. UI updates (title / menu) must also run on
//! the main thread — GPUI's AppKit run loop satisfies that when called from
//! `cx.update` / bootstrap.
//!
//! When no symbols are pinned, the status item shows the embedded S logo
//! (template image) instead of the "ZStock · 未固定" text placeholder.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use cocoa::appkit::{
    NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSData, NSSize, NSString};
use objc::declare::ClassDecl;
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

/// Embedded menu-bar mark (template PNG, black + alpha).
const STATUS_BAR_LOGO_PNG: &[u8] =
    include_bytes!("../assets/logo/status-bar-mark.png");

/// Actions sent from menu item clicks back into the GPUI app.
#[derive(Debug, Clone)]
pub enum StatusBarAction {
    /// Switch the title to this code (must be in the pinned list).
    SelectCode(String),
    /// Bring the main window forward.
    ShowWindow,
    /// Quit the application.
    Quit,
}

/// AppKit object pointer held as `usize` so the state can be `Send`/`Sync`.
/// Only touch from the main thread.
struct StatusBarState {
    item: usize,
    target: usize,
    /// Retained `NSImage*` for the idle logo.
    logo: usize,
    last_title: String,
    showing_logo: bool,
}

impl StatusBarState {
    fn item(&self) -> id {
        self.item as id
    }

    fn target(&self) -> id {
        self.target as id
    }

    fn logo(&self) -> id {
        self.logo as id
    }
}

static INSTALLED: AtomicBool = AtomicBool::new(false);
static ACTION_TX: OnceLock<Sender<StatusBarAction>> = OnceLock::new();
static STATE: Mutex<Option<StatusBarState>> = Mutex::new(None);

const TARGET_CLASS: &str = "ZStockStatusBarTarget";

/// Install the status item and return a receiver for menu actions.
/// Safe to call once; subsequent calls return a disconnected empty receiver.
pub fn install() -> Receiver<StatusBarAction> {
    let (tx, rx) = mpsc::channel();
    if ACTION_TX.set(tx).is_err() {
        // Already installed — return a dead receiver so callers can still spawn
        // a poll loop that immediately ends.
        let (_t, r) = mpsc::channel();
        return r;
    }
    unsafe {
        register_target_class();
        let target: id = msg_send![class!(ZStockStatusBarTarget), new];
        let bar = NSStatusBar::systemStatusBar(nil);
        let item = bar.statusItemWithLength_(NSVariableStatusItemLength);
        // Keep both objects alive for process lifetime while installed.
        let _: id = msg_send![item, retain];
        let _: id = msg_send![target, retain];

        let logo = load_logo_image();
        // Start with logo (no pins yet).
        apply_logo(item, logo);

        let menu = NSMenu::new(nil);
        let _: () = msg_send![menu, setAutoenablesItems: NO];
        item.setMenu_(menu);

        *STATE.lock().unwrap() = Some(StatusBarState {
            item: item as usize,
            target: target as usize,
            logo: logo as usize,
            last_title: String::new(),
            showing_logo: true,
        });
    }
    INSTALLED.store(true, Ordering::SeqCst);
    // Start hidden; app enables via config.
    set_visible(false);
    rx
}

pub fn is_installed() -> bool {
    INSTALLED.load(Ordering::SeqCst)
}

/// Show or hide the status item without destroying it.
pub fn set_visible(visible: bool) {
    let guard = STATE.lock().unwrap();
    let Some(st) = guard.as_ref() else {
        return;
    };
    unsafe {
        if visible {
            st.item().setLength_(NSVariableStatusItemLength);
        } else {
            // Length 0 removes it from the bar visually.
            st.item().setLength_(0.0);
        }
    }
}

/// Update the button title and clear the logo (skips if unchanged).
pub fn set_title(title: &str) {
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else {
        return;
    };
    if !st.showing_logo && st.last_title == title {
        return;
    }
    st.last_title = title.to_string();
    st.showing_logo = false;
    unsafe {
        apply_title(st.item(), title);
    }
}

/// Show the app logo and clear the title text (idle / no pins).
pub fn set_logo() {
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else {
        return;
    };
    if st.showing_logo {
        return;
    }
    st.showing_logo = true;
    st.last_title.clear();
    unsafe {
        apply_logo(st.item(), st.logo());
    }
}

/// One row in the status bar dropdown.
#[derive(Debug, Clone)]
pub struct MenuEntry {
    pub code: String,
    pub label: String,
    pub active: bool,
}

/// Rebuild the dropdown: pinned quotes + Show Window + Quit.
pub fn rebuild_menu(entries: &[MenuEntry], work_mode: bool) {
    let guard = STATE.lock().unwrap();
    let Some(st) = guard.as_ref() else {
        return;
    };
    let item = st.item();
    let target = st.target();
    unsafe {
        let menu: id = item.menu();
        if menu == nil {
            return;
        }
        // Clear existing items.
        let count: isize = msg_send![menu, numberOfItems];
        for _ in 0..count {
            let _: () = msg_send![menu, removeItemAtIndex: 0isize];
        }

        if entries.is_empty() {
            let title = if work_mode {
                "No symbols pinned"
            } else {
                "未固定标的 · 在设置中选择"
            };
            let row = menu_item_label(title, sel!(noop:), target, None);
            let _: () = msg_send![row, setEnabled: NO];
            menu.addItem_(row);
        } else {
            for e in entries {
                let row = menu_item_label(&e.label, sel!(selectCode:), target, Some(&e.code));
                let state: isize = if e.active { 1 } else { 0 };
                let _: () = msg_send![row, setState: state];
                menu.addItem_(row);
            }
        }

        menu.addItem_(NSMenuItem::separatorItem(nil));

        let show_label = if work_mode {
            "Show Window"
        } else {
            "显示主窗口"
        };
        menu.addItem_(menu_item_label(
            show_label,
            sel!(showWindow:),
            target,
            None,
        ));

        let quit_label = if work_mode { "Quit" } else { "退出 ZStock" };
        menu.addItem_(menu_item_label(quit_label, sel!(quitApp:), target, None));
    }
}

/// Tear down the status item (optional; process exit also clears it).
#[allow(dead_code)]
pub fn uninstall() {
    let mut guard = STATE.lock().unwrap();
    if let Some(st) = guard.take() {
        unsafe {
            let bar = NSStatusBar::systemStatusBar(nil);
            let item = st.item();
            let target = st.target();
            let logo = st.logo();
            bar.removeStatusItem_(item);
            let _: () = msg_send![item, release];
            let _: () = msg_send![target, release];
            if logo != nil {
                let _: () = msg_send![logo, release];
            }
        }
    }
    INSTALLED.store(false, Ordering::SeqCst);
}

// —— ObjC helpers ————————————————————————————————————————————————————————

unsafe fn load_logo_image() -> id {
    unsafe {
        let data = NSData::dataWithBytes_length_(
            nil,
            STATUS_BAR_LOGO_PNG.as_ptr() as *const std::os::raw::c_void,
            STATUS_BAR_LOGO_PNG.len() as u64,
        );
        if data == nil {
            return nil;
        }
        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithData: data];
        if image == nil {
            return nil;
        }
        // Template: menu bar tints for light/dark appearance.
        let _: () = msg_send![image, setTemplate: YES];
        // Point size ~18 matches typical menu-bar glyph.
        let size = NSSize::new(18.0, 18.0);
        let _: () = msg_send![image, setSize: size];
        image
    }
}

unsafe fn apply_logo(item: id, logo: id) {
    unsafe {
        let button: id = item.button();
        if button == nil {
            set_button_title(item, "ZStock");
            return;
        }
        let empty = NSString::alloc(nil).init_str("");
        let _: () = msg_send![button, setTitle: empty];
        if logo != nil {
            let _: () = msg_send![button, setImage: logo];
            // NSImageOnly = 1
            let _: () = msg_send![button, setImagePosition: 1isize];
        } else {
            // Fallback if image failed to load.
            set_button_title(item, "ZStock");
        }
    }
}

unsafe fn apply_title(item: id, title: &str) {
    unsafe {
        let button: id = item.button();
        if button == nil {
            set_button_title(item, title);
            return;
        }
        // Clear image so multi-quote text is not squeezed beside a glyph.
        let _: () = msg_send![button, setImage: nil];
        // NSNoImage = 0
        let _: () = msg_send![button, setImagePosition: 0isize];
        let ns = NSString::alloc(nil).init_str(title);
        let _: () = msg_send![button, setTitle: ns];
    }
}

unsafe fn set_button_title(item: id, title: &str) {
    unsafe {
        let button: id = item.button();
        if button == nil {
            // Older macOS fallback.
            let ns = NSString::alloc(nil).init_str(title);
            let _: () = msg_send![item, setTitle: ns];
            return;
        }
        let ns = NSString::alloc(nil).init_str(title);
        let _: () = msg_send![button, setTitle: ns];
    }
}

unsafe fn menu_item_label(title: &str, action: Sel, target: id, code: Option<&str>) -> id {
    unsafe {
        let ns_title = NSString::alloc(nil).init_str(title);
        let ns_key = NSString::alloc(nil).init_str("");
        let item =
            NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(ns_title, action, ns_key);
        item.setTarget_(target);
        if let Some(c) = code {
            let ns_code = NSString::alloc(nil).init_str(c);
            let _: () = msg_send![item, setRepresentedObject: ns_code];
        }
        item
    }
}

unsafe fn register_target_class() {
    if objc::runtime::Class::get(TARGET_CLASS).is_some() {
        return;
    }
    unsafe {
        let superclass = class!(NSObject);
        let mut decl =
            ClassDecl::new(TARGET_CLASS, superclass).expect("declare status bar target");
        decl.add_method(
            sel!(selectCode:),
            select_code as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(showWindow:),
            show_window as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(sel!(quitApp:), quit_app as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(noop:), noop as extern "C" fn(&Object, Sel, id));
        decl.register();
    }
}

fn send_action(action: StatusBarAction) {
    if let Some(tx) = ACTION_TX.get() {
        let _ = tx.send(action);
    }
}

extern "C" fn select_code(_this: &Object, _sel: Sel, sender: id) {
    unsafe {
        if sender == nil {
            return;
        }
        let obj: id = msg_send![sender, representedObject];
        if obj == nil {
            return;
        }
        let bytes: *const std::os::raw::c_char = msg_send![obj, UTF8String];
        if bytes.is_null() {
            return;
        }
        let s = std::ffi::CStr::from_ptr(bytes)
            .to_string_lossy()
            .into_owned();
        if !s.is_empty() {
            send_action(StatusBarAction::SelectCode(s));
        }
    }
}

extern "C" fn show_window(_this: &Object, _sel: Sel, _sender: id) {
    send_action(StatusBarAction::ShowWindow);
}

extern "C" fn quit_app(_this: &Object, _sel: Sel, _sender: id) {
    send_action(StatusBarAction::Quit);
}

extern "C" fn noop(_this: &Object, _sel: Sel, _sender: id) {}
