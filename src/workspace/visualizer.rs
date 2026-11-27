use super::*;
use gpui_kit::component::WindowExt as _;
use gpui_webview::WebView;

impl Workspace {
    pub(super) fn sync_visualizer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = !self.request_tabs.is_empty()
            && self.response_pane == Pane::Visualizer
            && (!Self::compact_layout(window) || self.compact_pane == CompactPane::Response)
            && self.variable_editor.is_none()
            && self.openapi_import.is_none()
            && !self.cloud_is_open()
            && !self.comparing_cloud()
            && !window.has_active_dialog(cx)
            && matches!(self.active().response, RequestResponse::Finished { .. })
            && self.active().script_report.visualization.is_some();
        if !visible {
            self.visualizer_error = None;
            if let Some(view) = &self.visualizer {
                view.update(cx, |view, _| view.hide());
            }
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let request_id = self.active().id;
            let visualization = self.active().script_report.visualization.as_ref().unwrap();
            let document = crate::visualizer::document(visualization)?;
            if let Some(view) = &self.visualizer {
                if self
                    .visualizer_document
                    .as_ref()
                    .is_none_or(|(id, loaded)| *id != request_id || loaded != &document)
                {
                    view.read(cx).raw().load_html(&document)?;
                }
                view.read(cx).set_visible(true)?;
            } else {
                let builder = wry::WebViewBuilder::new()
                    .with_html(&document)
                    .with_incognito(true)
                    .with_navigation_handler(|url| crate::visualizer::allow_navigation(&url))
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_download_started_handler(|_, _| false);
                let view = WebView::build(builder, window, cx)?;
                self.visualizer = Some(cx.new(|_| view));
            }
            self.visualizer_document = Some((request_id, document));
            Ok(())
        })();
        self.visualizer_error = result
            .err()
            .map(|error| format!("Could not display the visualization: {error:#}"));
        if self.visualizer_error.is_some() {
            if let Some(view) = &self.visualizer {
                view.update(cx, |view, _| view.hide());
            }
        }
    }
}
