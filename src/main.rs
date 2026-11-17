#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

mod http;
mod request;
mod storage;
mod workspace;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use std::borrow::Cow;

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(include_bytes!("../assets/Inter.ttf"))])
            .expect("Could not load the bundled Inter font");
        workspace::set_theme(cx);
        workspace::bind_keys(cx);
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

        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
