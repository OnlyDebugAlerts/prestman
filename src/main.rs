#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

mod app_assets;
mod cloud;
mod collection_io;
mod curl_import;
#[cfg(target_os = "macos")]
mod desktop_icon;
mod graphql;
mod grpc;
mod http;
mod openapi;
mod request;
mod scripts;
mod storage;
mod updater;
mod variables;
mod visualizer;
mod workspace;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use std::borrow::Cow;

fn main() {
    #[cfg(target_os = "windows")]
    // Before GPUI starts threads: WebView2 needs this child-surface rendering mode.
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
    }
    #[cfg(target_os = "linux")]
    let app = gpui_kit::platform::linux(WindowingModes::X11);
    #[cfg(not(target_os = "linux"))]
    let app = application();
    app.with_assets(app_assets::AppAssets).run(|cx| {
        init(cx);
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(include_bytes!("../assets/Inter.ttf"))])
            .expect("Could not load the bundled Inter font");
        workspace::set_theme(cx);
        workspace::bind_keys(cx);
        #[cfg(target_os = "macos")]
        cx.set_menus([Menu::new("Prestman").items([MenuItem::action(
            "Quit Prestman",
            workspace::QuitApplication,
        )])]);
        open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Prestman".into()),
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1220.), px(780.)),
                    cx,
                ))),
                window_min_size: Some(size(px(860.), px(560.))),
                ..TitleBar::window_options()
            },
            cx,
            |window, cx| cx.new(|cx| workspace::Workspace::new(window, cx)),
        )
        .expect("Could not open the Prestman window");

        #[cfg(target_os = "macos")]
        if let Err(error) = desktop_icon::init(cx) {
            eprintln!("Could not initialize the Prestman tray icon: {error:#}");
        }

        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
