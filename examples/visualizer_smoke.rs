//! Run with `cargo run --example visualizer_smoke`. Opens a temporary native
//! window, checks the real web engine, and exits without opening user storage.
#[path = "../src/visualizer.rs"]
mod visualizer;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window,
};
use gpui_webview::WebView;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct SmokeView {
    page: Entity<WebView>,
}

impl Render for SmokeView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::div().size_full().child(self.page.clone())
    }
}

fn main() {
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
    }
    #[cfg(target_os = "linux")]
    let app = gpui_kit::platform::linux(gpui_kit::WindowingModes::X11);
    #[cfg(not(target_os = "linux"))]
    let app = gpui_kit::application();
    let passed = Arc::new(AtomicBool::new(false));
    let result = passed.clone();
    app.run(move |cx| {
        gpui_kit::init(cx);
        let visualization = visualizer::Visualization {
            html: r#"<style>#title {color:rgb(20, 40, 60)}</style>
<h1 id="title">Native Visualizer</h1><canvas id="chart"></canvas>
<script>
pm.getData((error, data) => {
    const canvas = document.getElementById('chart');
    const context = canvas.getContext('2d');
    context.fillStyle = 'red'; context.fillRect(0, 0, 2, 2);
    let isolated = false;
    try { void parent.document.body; } catch (_) { isolated = true; }
    parent.postMessage({
        ok: !error && data.name === 'Привет' && isolated &&
            getComputedStyle(document.getElementById('title')).color === 'rgb(20, 40, 60)' &&
            context.getImageData(0, 0, 1, 1).data[0] === 255,
        data, isolated, title: document.getElementById('title').textContent
    }, '*');
});
</script>"#
                .into(),
            data: serde_json::json!({"name":"Привет", "escaped":"</script><p>data</p>"}),
        };
        let mut document = visualizer::document(&visualization).unwrap();
        document = document.replace(
            "</body></html>",
            r#"<script>
window.addEventListener('message', event => {
    if (event.source === document.getElementById('visualizer').contentWindow)
        window.ipc.postMessage(JSON.stringify(event.data));
});
</script></body></html>"#,
        );
        let (tx, rx) = mpsc::channel();
        gpui_kit::open_window(
            gpui_kit::WindowOptions {
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("Prestman Visualizer smoke test".into()),
                    ..Default::default()
                }),
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                    gpui_kit::Bounds::centered(
                        None,
                        gpui_kit::size(gpui_kit::px(600.), gpui_kit::px(400.)),
                        cx,
                    ),
                )),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let builder = wry::WebViewBuilder::new()
                    .with_html(document)
                    .with_incognito(true)
                    .with_navigation_handler(|url| visualizer::allow_navigation(&url))
                    .with_ipc_handler(move |message| {
                        let _ = tx.send(message.body().clone());
                    });
                let page = WebView::build(builder, window, cx)
                    .expect("Could not create native Visualizer");
                let page = cx.new(|_| page);
                cx.new(|_| SmokeView { page })
            },
        )
        .expect("Could not open smoke-test window");
        let background = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Ok(message) = rx.try_recv() {
                    let report: serde_json::Value = serde_json::from_str(&message).unwrap();
                    println!("Native Visualizer: {report}");
                    result.store(report["ok"] == true, Ordering::SeqCst);
                    cx.update(|cx| cx.quit());
                    break;
                }
                if Instant::now() > deadline {
                    eprintln!("Native Visualizer did not produce a result within 15 seconds");
                    cx.update(|cx| cx.quit());
                    break;
                }
                background.timer(Duration::from_millis(100)).await;
            }
        })
        .detach();
        cx.activate(true);
    });
    assert!(
        passed.load(Ordering::SeqCst),
        "Native Visualizer smoke test failed"
    );
}
