//! macOS menu bar (NSStatusItem) for live watchlist quotes.
//!
//! Install once on the main thread. UI updates (title / menu) must also run on
//! the main thread — GPUI's AppKit run loop satisfies that when called from
//! `cx.update` / bootstrap.
//!
//! When no symbols are pinned, the status item shows the embedded S logo
//! (template image) instead of the "ZStock · 未固定" text placeholder.
//!
//! ## Menu update policy
//! Quote ticks refresh labels every second. Rebuilding an open `NSMenu`
//! (remove/add items) freezes AppKit tracking and can deadlock if close
//! callbacks re-enter our mutex. While the dropdown is open we only stash a
//! pending snapshot; structure-stable refreshes update titles in place.

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

/// Snapshot applied after the dropdown closes (or immediately if closed).
#[derive(Clone)]
struct PendingMenu {
    entries: Vec<MenuEntry>,
    work_mode: bool,
    /// Full fingerprint (labels + selection + work_mode).
    sig: String,
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
    /// Last applied visibility (avoid setLength thrash every quote tick).
    last_visible: Option<bool>,
    /// Fingerprint of last applied menu (labels + active flags + work_mode).
    last_menu_sig: String,
    /// Codes + work_mode + active — used to decide full rebuild vs in-place.
    last_structure_sig: String,
    /// True between `menuWillOpen` and `menuDidClose`.
    menu_open: bool,
    /// Latest rebuild requested while the menu was open.
    pending_menu: Option<PendingMenu>,
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
        // Track open/close so quote ticks never tear down a live menu.
        let _: () = msg_send![menu, setDelegate: target];
        item.setMenu_(menu);

        *STATE.lock().unwrap() = Some(StatusBarState {
            item: item as usize,
            target: target as usize,
            logo: logo as usize,
            last_title: String::new(),
            showing_logo: true,
            last_visible: None,
            last_menu_sig: String::new(),
            last_structure_sig: String::new(),
            menu_open: false,
            pending_menu: None,
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
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else {
        return;
    };
    if st.last_visible == Some(visible) {
        return;
    }
    st.last_visible = Some(visible);
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
    let clear_image = st.showing_logo;
    st.last_title = title.to_string();
    st.showing_logo = false;
    unsafe {
        apply_title(st.item(), title, clear_image);
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

fn menu_sig(entries: &[MenuEntry], work_mode: bool) -> String {
    let mut s = String::with_capacity(entries.len() * 48 + 8);
    s.push(if work_mode { 'W' } else { 'N' });
    for e in entries {
        s.push('|');
        s.push_str(&e.code);
        s.push(':');
        s.push_str(&e.label);
        s.push(if e.active { '*' } else { '.' });
    }
    s
}

/// Codes / selection / work_mode — independent of live price text.
fn structure_sig(entries: &[MenuEntry], work_mode: bool) -> String {
    let mut s = String::with_capacity(entries.len() * 16 + 4);
    s.push(if work_mode { 'W' } else { 'N' });
    for e in entries {
        s.push('|');
        s.push_str(&e.code);
        s.push(if e.active { '*' } else { '.' });
    }
    s
}

/// Rebuild the dropdown: pinned quotes + Show Window + Quit.
///
/// - Skips work when the full signature is unchanged.
/// - While the menu is open, never mutates items (defer to close).
/// - When only prices change, updates titles/checkmarks in place.
pub fn rebuild_menu(entries: &[MenuEntry], work_mode: bool) {
    let sig = menu_sig(entries, work_mode);
    let struct_sig = structure_sig(entries, work_mode);

    // Snapshot + decide under the lock; AppKit work runs unlocked so menu
    // open/close delegates cannot deadlock on `STATE`.
    let plan = {
        let mut guard = STATE.lock().unwrap();
        let Some(st) = guard.as_mut() else {
            return;
        };
        if st.last_menu_sig == sig {
            return;
        }
        if st.menu_open {
            st.pending_menu = Some(PendingMenu {
                entries: entries.to_vec(),
                work_mode,
                sig,
            });
            return;
        }
        let in_place = !st.last_structure_sig.is_empty() && st.last_structure_sig == struct_sig;
        st.last_menu_sig = sig;
        st.last_structure_sig = struct_sig;
        st.pending_menu = None;
        Some((
            st.item() as usize,
            st.target() as usize,
            in_place,
            entries.to_vec(),
            work_mode,
        ))
    };

    let Some((item, target, in_place, entries, work_mode)) = plan else {
        return;
    };
    unsafe {
        if in_place {
            update_menu_in_place(item as id, &entries, work_mode);
        } else {
            rebuild_menu_items(item as id, target as id, &entries, work_mode);
        }
    }
}

fn apply_pending_menu(pending: PendingMenu) {
    let plan = {
        let mut guard = STATE.lock().unwrap();
        let Some(st) = guard.as_mut() else {
            return;
        };
        if st.menu_open {
            // Nested open — keep pending for the next close.
            st.pending_menu = Some(pending);
            return;
        }
        if st.last_menu_sig == pending.sig {
            return;
        }
        let struct_sig = structure_sig(&pending.entries, pending.work_mode);
        let in_place = !st.last_structure_sig.is_empty() && st.last_structure_sig == struct_sig;
        st.last_menu_sig = pending.sig;
        st.last_structure_sig = struct_sig;
        st.pending_menu = None;
        Some((
            st.item() as usize,
            st.target() as usize,
            in_place,
            pending.entries,
            pending.work_mode,
        ))
    };
    let Some((item, target, in_place, entries, work_mode)) = plan else {
        return;
    };
    unsafe {
        if in_place {
            update_menu_in_place(item as id, &entries, work_mode);
        } else {
            rebuild_menu_items(item as id, target as id, &entries, work_mode);
        }
    }
}

unsafe fn rebuild_menu_items(item: id, target: id, entries: &[MenuEntry], work_mode: bool) {
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

/// Update prices/checkmarks without destroying menu items (structure unchanged).
unsafe fn update_menu_in_place(item: id, entries: &[MenuEntry], work_mode: bool) {
    unsafe {
        let menu: id = item.menu();
        if menu == nil {
            return;
        }
        let count: isize = msg_send![menu, numberOfItems];
        if entries.is_empty() {
            // Placeholder row only.
            if count >= 1 {
                let row: id = msg_send![menu, itemAtIndex: 0isize];
                if row != nil {
                    let title = if work_mode {
                        "No symbols pinned"
                    } else {
                        "未固定标的 · 在设置中选择"
                    };
                    set_item_title(row, title);
                }
            }
            return;
        }
        // Expect: N symbols + separator + Show + Quit.
        if count < entries.len() as isize {
            return;
        }
        for (i, e) in entries.iter().enumerate() {
            let row: id = msg_send![menu, itemAtIndex: i as isize];
            if row == nil {
                continue;
            }
            set_item_title(row, &e.label);
            let state: isize = if e.active { 1 } else { 0 };
            let _: () = msg_send![row, setState: state];
        }
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
            // Drop delegate before release so AppKit does not call into freed target.
            let menu: id = item.menu();
            if menu != nil {
                let _: () = msg_send![menu, setDelegate: nil];
            }
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

/// `alloc` + `init` returns a retained object; callers own it under MRC.
unsafe fn nsstring(s: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(s) }
}

/// Balance a retained ObjC object created via `alloc` / `new` / `copy`.
unsafe fn release_obj(obj: id) {
    unsafe {
        if obj != nil {
            let _: () = msg_send![obj, release];
        }
    }
}

/// `setTitle:` copies/retains; release our temporary string so quote ticks
/// (once per second for hours) do not leak NSString instances into the process.
unsafe fn set_item_title(item: id, title: &str) {
    unsafe {
        if item == nil {
            return;
        }
        let ns = nsstring(title);
        let _: () = msg_send![item, setTitle: ns];
        release_obj(ns);
    }
}

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
        set_item_title(button, "");
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

unsafe fn apply_title(item: id, title: &str, clear_image: bool) {
    unsafe {
        let button: id = item.button();
        if button == nil {
            set_button_title(item, title);
            return;
        }
        if clear_image {
            // Leaving a template glyph beside multi-quote text squeezes the title.
            let _: () = msg_send![button, setImage: nil];
            // NSNoImage = 0
            let _: () = msg_send![button, setImagePosition: 0isize];
        }
        set_item_title(button, title);
        // Re-assert variable length so the bar remeasures after title changes
        // (esp. when switching logo ↔ multi-quote text of different widths).
        let _: () = msg_send![item, setLength: NSVariableStatusItemLength];
        let _: () = msg_send![button, setNeedsDisplay: YES];
    }
}

unsafe fn set_button_title(item: id, title: &str) {
    unsafe {
        let button: id = item.button();
        if button == nil {
            // Older macOS fallback.
            set_item_title(item, title);
            return;
        }
        set_item_title(button, title);
    }
}

unsafe fn menu_item_label(title: &str, action: Sel, target: id, code: Option<&str>) -> id {
    unsafe {
        let ns_title = nsstring(title);
        let ns_key = nsstring("");
        // initWithTitle: copies title/key; release our temporaries.
        let item =
            NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(ns_title, action, ns_key);
        release_obj(ns_title);
        release_obj(ns_key);
        item.setTarget_(target);
        if let Some(c) = code {
            let ns_code = nsstring(c);
            let _: () = msg_send![item, setRepresentedObject: ns_code];
            release_obj(ns_code);
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
        // NSMenuDelegate — keep quote-driven rebuilds off the open menu.
        decl.add_method(
            sel!(menuWillOpen:),
            menu_will_open as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(menuDidClose:),
            menu_did_close as extern "C" fn(&Object, Sel, id),
        );
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
    // Terminate immediately on the AppKit menu-action thread. Going through the
    // 50ms GPUI poll + async `cx.quit()` made "退出" feel dead, especially when
    // the main thread was busy applying quote ticks.
    send_action(StatusBarAction::Quit);
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        if app != nil {
            let _: () = msg_send![app, terminate: nil];
        }
    }
}

extern "C" fn noop(_this: &Object, _sel: Sel, _sender: id) {}

extern "C" fn menu_will_open(_this: &Object, _sel: Sel, _menu: id) {
    let mut guard = STATE.lock().unwrap();
    if let Some(st) = guard.as_mut() {
        st.menu_open = true;
    }
}

extern "C" fn menu_did_close(_this: &Object, _sel: Sel, _menu: id) {
    let pending = {
        let mut guard = STATE.lock().unwrap();
        let Some(st) = guard.as_mut() else {
            return;
        };
        st.menu_open = false;
        st.pending_menu.take()
    };
    if let Some(pending) = pending {
        apply_pending_menu(pending);
    }
}
