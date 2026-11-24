use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Visualization {
    pub html: String,
    pub data: Value,
}

pub fn allow_navigation(url: &str) -> bool {
    matches!(url.split('#').next(), Some("about:blank" | "about:srcdoc"))
}

fn script_string(value: &str) -> Result<String> {
    Ok(serde_json::to_string(value)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

pub fn document(visualization: &Visualization) -> Result<String> {
    let data = script_string(&serde_json::to_string(&visualization.data)?)?;
    let child = format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>body{{margin:16px;font:13px system-ui,sans-serif;color:#202124;background:white}}</style>
<script>
(() => {{
    const data = JSON.parse({data});
    window.pm = Object.freeze({{getData(callback) {{
        if (typeof callback !== 'function') throw new TypeError('pm.getData expects a callback');
        queueMicrotask(() => callback(null, data));
    }}}});
    function showError(message) {{
        const show = () => {{
            const error = document.createElement('pre');
            error.style.cssText = 'white-space:pre-wrap;padding:12px;background:#fff0f0;color:#a00;border:1px solid #e99';
            error.textContent = 'Visualizer: ' + message;
            document.body.prepend(error);
        }};
        if (document.body) show(); else document.addEventListener('DOMContentLoaded', show, {{once:true}});
    }}
    window.addEventListener('error', event => {{
        if (event.message) showError(event.message);
        else if (event.target && event.target.src) showError('Could not load ' + event.target.src);
    }}, true);
    window.addEventListener('unhandledrejection', event => showError(String(event.reason)));
}})();
</script></head><body>{}</body></html>"#,
        visualization.html
    );
    let srcdoc = script_string(&child)?;
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>html,body,iframe{{width:100%;height:100%;margin:0;border:0;display:block;background:white;overflow:hidden}}</style>
</head><body><iframe id="visualizer" title="Visualizer" sandbox="allow-scripts"></iframe>
<script>document.getElementById('visualizer').srcdoc = {srcdoc};</script></body></html>"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn template_scripts_and_data_stay_inside_the_sandbox() -> Result<()> {
        let page = document(&Visualization {
            html: "<h1>Hello</h1><script>pm.getData((error, data) => console.log(data));</script>"
                .into(),
            data: json!({"value": "</script><script>parent.stolen=true</script>\u{2028}"}),
        })?;
        assert!(page.contains("sandbox=\"allow-scripts\""));
        assert!(!page.contains("allow-same-origin"));
        assert!(!page.contains("<script>parent.stolen"));
        assert_eq!(page.matches("<script>").count(), 1);
        let encoded = page
            .split(".srcdoc = ")
            .nth(1)
            .unwrap()
            .split(";</script>")
            .next()
            .unwrap();
        let child: String = serde_json::from_str(encoded)?;
        assert!(child.contains("pm.getData"));
        assert!(child.contains("<h1>Hello</h1><script>"));
        assert!(!child.contains("<script>parent.stolen"));
        Ok(())
    }

    #[test]
    fn navigation_rejects_remote_and_filesystem_documents() {
        assert!(allow_navigation("about:blank"));
        assert!(allow_navigation("about:srcdoc#section"));
        for url in [
            "file:///etc/passwd",
            "https://example.test",
            "javascript:alert(1)",
            "data:text/html,test",
            "about:blank.evil",
        ] {
            assert!(!allow_navigation(url), "{url}");
        }
    }
}
