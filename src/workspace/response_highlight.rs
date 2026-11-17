use std::{ops::Range, sync::Arc};

use gpui_kit::{
    Context, HighlightStyle, SharedString, Window,
    component::{
        highlighter::SyntaxHighlighter,
        input::{
            EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope,
        },
    },
};

type StyleRun = (Range<usize>, HighlightStyle);

#[derive(Clone)]
pub(super) struct ResponseHighlight {
    runs: Arc<[StyleRun]>,
    folds: Arc<[FoldRange]>,
}

impl ResponseHighlight {
    pub(super) fn json(body: &str, theme: &dyn HighlightStyleResolver) -> Self {
        let text = Rope::from(body);
        let mut highlighter = SyntaxHighlighter::new("json");
        highlighter.update(None, &text, None);

        let mut folds = Vec::new();
        if let Some(tree) = highlighter.tree() {
            let root = tree.root_node();
            let mut pending = root.named_children(&mut root.walk()).collect::<Vec<_>>();
            while let Some(node) = pending.pop() {
                let start = node.start_position().row;
                let end = node.end_position().row;
                if end.saturating_sub(start) >= 2 {
                    folds.push(FoldRange::new(start, end));
                    pending.extend(node.named_children(&mut node.walk()));
                }
            }
            folds.sort_unstable_by_key(|fold| (fold.start_line, std::cmp::Reverse(fold.end_line)));
            folds.dedup_by_key(|fold| fold.start_line);
        }

        Self {
            runs: highlighter.styles(&(0..text.len()), theme).into(),
            folds: folds.into(),
        }
    }
}

impl InputHighlighter for ResponseHighlight {
    fn language(&self) -> SharedString {
        "json".into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        _: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        // The read-only editor receives a new snapshot whenever its text changes.
    }

    fn styles(&self, range: &Range<usize>, _: &dyn HighlightStyleResolver) -> Vec<StyleRun> {
        let first = self.runs.partition_point(|(run, _)| run.end <= range.start);
        self.runs[first..]
            .iter()
            .take_while(|(run, _)| run.start < range.end)
            .map(|(run, style)| (run.start.max(range.start)..run.end.min(range.end), *style))
            .filter(|(run, _)| !run.is_empty())
            .collect()
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        self.folds.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::highlighter::HighlightTheme;

    #[test]
    fn cached_highlighting_matches_the_editor_near_the_end_of_a_large_json_response() {
        let row = format!("  {{\"key\": \"{}\", \"number\": 42}},\n", "x".repeat(256));
        let body = format!("[\n{}  {{\"последний\": true}}\n]", row.repeat(5000));
        let theme = HighlightTheme::default_dark();
        let cached = ResponseHighlight::json(&body, &*theme);
        let mut original = SyntaxHighlighter::new("json");
        original.update(None, &Rope::from(body.as_str()), None);

        let viewport = body.rfind("  {").unwrap()..body.len();
        let actual = cached.styles(&viewport, &*theme);
        assert!(actual.iter().any(|(_, style)| style.color.is_some()));
        assert_eq!(actual, original.styles(&viewport, &*theme));
        assert_eq!(cached.folds[0], FoldRange::new(0, 5002));

        let viewports: Vec<_> = (0..100)
            .map(|index| {
                let start = 2 + index * 47 * row.len();
                start..start + 40 * row.len()
            })
            .collect();
        let started = std::time::Instant::now();
        let original_styles: Vec<_> = viewports
            .iter()
            .map(|range| original.styles(range, &*theme))
            .collect();
        let original_time = started.elapsed();
        let started = std::time::Instant::now();
        let cached_styles: Vec<_> = viewports
            .iter()
            .map(|range| cached.styles(range, &*theme))
            .collect();
        let cached_time = started.elapsed();
        assert_eq!(cached_styles, original_styles);
        eprintln!("100 windows of 40 rows: Tree-sitter {original_time:?}, cached {cached_time:?}");
    }
}
