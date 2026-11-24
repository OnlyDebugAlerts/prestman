use super::*;

impl Workspace {
    pub(super) fn import_curl(
        &mut self,
        id: usize,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.request_index(id) else {
            return;
        };
        let mut imported = match crate::curl_import::parse(command) {
            Ok(request) => request,
            Err(error) => {
                self.save_error = Some(format!("Could not import cURL. {error:#}"));
                cx.notify();
                return;
            }
        };
        if self.requests[index].response.is_sending() {
            self.save_error =
                Some("Wait for the current request to finish before importing cURL.".into());
            cx.notify();
            return;
        }
        // Replace HTTP fields while retaining the request's identity, folder and scripts.
        let entry = &mut self.requests[index];
        imported.name = entry.draft.name.clone();
        imported.pre_request_script = entry.draft.pre_request_script.clone();
        imported.post_response_script = entry.draft.post_response_script.clone();
        entry.replace_draft(imported);
        entry.response = RequestResponse::Idle;
        entry.script_report = Report::default();
        self.save_error = None;
        self.variable_hover = None;
        if self.selected == index {
            self.request_pane = if !entry.draft.body.is_empty() {
                RequestPane::Body
            } else if !entry.draft.query.is_empty() {
                RequestPane::Pairs(PairKind::Query)
            } else if !entry.draft.headers.is_empty() {
                RequestPane::Pairs(PairKind::Headers)
            } else {
                RequestPane::Body
            };
            self.response_pane = Pane::Body;
            self.load_editor(window, cx);
            let length = self.url.read(cx).value().len();
            self.url
                .update(cx, |url, cx| url.set_selected_range(length..length, cx));
        }
        cx.notify();
    }
}
