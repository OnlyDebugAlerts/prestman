//! AppKit integration uses the GPUI main-thread event loop.
use anyhow::{Context as _, Result};
use gpui_kit::{App, Global};
use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{NSApplication, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem};
use objc2_foundation::{NSData, NSObject, NSObjectProtocol, NSSize, NSString};

define_class!(
    // NSObject has no subclassing requirements; AppKit actions run on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrestmanTrayActions"]
    struct TrayActions;

    unsafe impl NSObjectProtocol for TrayActions {}

    impl TrayActions {
        #[unsafe(method(showPrestman:))]
        fn show(&self, _sender: &NSObject) {
            let app = NSApplication::sharedApplication(self.mtm());
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            // Restore the existing GPUI window, including when it is minimized.
            let windows = app.windows();
            for index in 0..windows.count() {
                let window = windows.objectAtIndex(index);
                window.deminiaturize(None);
                window.makeKeyAndOrderFront(None);
            }
        }
    }
);

struct DesktopIcon {
    item: Retained<NSStatusItem>,
    _actions: Retained<TrayActions>,
}

impl Global for DesktopIcon {}

impl Drop for DesktopIcon {
    fn drop(&mut self) {
        // GPUI globals are destroyed on the main thread, while AppKit is still alive.
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

pub fn init(cx: &mut App) -> Result<()> {
    let mtm = MainThreadMarker::new().context("Tray initialization requires the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    let data = NSData::with_bytes(include_bytes!("../assets/prestman-logo.png"));
    let logo = NSImage::initWithData(NSImage::alloc(), &data).context("Invalid app logo PNG")?;
    let tray_data = NSData::with_bytes(include_bytes!("../assets/prestman-tray.png"));
    let tray_logo =
        NSImage::initWithData(NSImage::alloc(), &tray_data).context("Invalid tray logo PNG")?;

    // AppKit retains assigned images and menus. The global retains the action target
    // because NSMenuItem's target property is weak.
    unsafe {
        app.setApplicationIconImage(Some(&logo));
        tray_logo.setSize(NSSize::new(22., 22.));
        let actions = mtm.alloc::<TrayActions>().set_ivars(());
        let actions: Retained<TrayActions> = msg_send![super(actions), init];
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(30.);
        let button = item
            .button(mtm)
            .context("Could not create the tray button")?;
        button.setImage(Some(&tray_logo));
        button.setToolTip(Some(&NSString::from_str("Prestman")));

        let menu = NSMenu::new(mtm);
        let show = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc::<NSMenuItem>(),
            &NSString::from_str("Show Prestman"),
            Some(sel!(showPrestman:)),
            &NSString::new(),
        );
        show.setTarget(Some(&actions));
        menu.addItem(&show);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc::<NSMenuItem>(),
            &NSString::from_str("Quit Prestman"),
            Some(sel!(terminate:)),
            &NSString::new(),
        );
        quit.setTarget(Some(&app));
        menu.addItem(&quit);
        item.setMenu(Some(&menu));
        cx.set_global(DesktopIcon {
            item,
            _actions: actions,
        });
    }
    Ok(())
}
