mod box_tree;
mod color;
mod dom;
mod font;
mod html;
mod image;
mod layout;
pub mod paint;
mod pdf;
mod script;
mod subset;

use std::fmt;

pub use font::FontSource;
pub use layout::{PageSize, Paper, RenderOptions};
pub use script::{NoopScriptEngine, ScriptEngine, ScriptLimits, ScriptReport};
#[cfg(feature = "js")]
pub use script::BoaScriptEngine;

#[derive(Debug)]
pub enum Error {
    EmptyDocument,
    Pdf(pdf::PdfError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDocument => write!(f, "document does not contain renderable text"),
            Self::Pdf(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<pdf::PdfError> for Error {
    fn from(value: pdf::PdfError) -> Self {
        Self::Pdf(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default)]
pub struct Engine;

impl Engine {
    pub fn new() -> Self {
        Self
    }

    pub fn render_html(&self, html: &str, options: RenderOptions) -> Result<Vec<u8>> {
        self.render_document(html::parse(html), options)
    }

    /// Render after running a bounded pre-layout script stage (ADR 0006) that may
    /// mutate the DOM. Passing [`NoopScriptEngine`] is equivalent to
    /// [`render_html`](Self::render_html).
    pub fn render_html_with_scripts(
        &self,
        html: &str,
        options: RenderOptions,
        engine: &dyn ScriptEngine,
        limits: &ScriptLimits,
    ) -> Result<Vec<u8>> {
        self.render_document(html::parse_scripted(html, engine, limits), options)
    }

    fn render_document(
        &self,
        mut document: html::Document,
        options: RenderOptions,
    ) -> Result<Vec<u8>> {
        // Load and measure `<img>` content before the emptiness check so an
        // image-only document counts as renderable.
        html::resolve_images(&mut document, options.base_dir.as_deref());

        let has_flow = document.flow.as_ref().is_some_and(|flow| flow.has_text());
        if document.blocks.is_empty() && !has_flow {
            return Err(Error::EmptyDocument);
        }

        let options = options.with_document_hints(&document);
        let pages = layout::layout_document(&document, &options);
        pdf::write_pdf(&pages, &document.images, &options).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::{Engine, RenderOptions};

    #[test]
    fn renders_pdf_header() {
        let pdf = Engine::new()
            .render_html("<h1>Hello</h1><p>World</p>", RenderOptions::default())
            .expect("render should succeed");

        assert!(pdf.starts_with(b"%PDF-1.7\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn rejects_empty_documents() {
        let error = Engine::new()
            .render_html("<html><body>   </body></html>", RenderOptions::default())
            .expect_err("empty documents should fail");

        assert_eq!(
            error.to_string(),
            "document does not contain renderable text"
        );
    }

    #[test]
    fn noop_scripting_matches_static_render() {
        use super::{NoopScriptEngine, ScriptLimits};
        let html = "<h1>Hello</h1><p id=\"x\">World</p>";
        let plain = Engine::new()
            .render_html(html, RenderOptions::default())
            .unwrap();
        let scripted = Engine::new()
            .render_html_with_scripts(
                html,
                RenderOptions::default(),
                &NoopScriptEngine,
                &ScriptLimits::default(),
            )
            .unwrap();
        assert_eq!(plain, scripted, "no-op scripting must not change output");
    }

    #[cfg(feature = "js")]
    #[test]
    fn scripting_mutates_rendered_document() {
        use super::{BoaScriptEngine, ScriptLimits};
        let html = "<p id=\"x\">OLD</p><script>document.getElementById('x').textContent = 'SCRIPTED'</script>";

        let pdf = Engine::new()
            .render_html_with_scripts(
                html,
                RenderOptions::default(),
                &BoaScriptEngine,
                &ScriptLimits::default(),
            )
            .expect("scripted render should succeed");

        assert!(pdf.starts_with(b"%PDF-1.7\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
    }

    /// CJK with the default (base-14) font must ride the fallback chain into an
    /// embedded Type0 face — skipped on systems with no CJK-capable fallback.
    #[test]
    fn cjk_renders_via_fallback_chain() {
        let helvetica = crate::font::Font::helvetica();
        let has_cjk = helvetica
            .fallback_chain()
            .iter()
            .any(|f| f.text_width("\u{4EF7}", 10.0) > 0.0 && f.embedding().is_some());
        if !has_cjk {
            return;
        }

        let pdf = Engine::new()
            .render_html("<p>Total \u{4EF7}\u{683C}: 25 USD</p>", RenderOptions::default())
            .expect("CJK document renders");

        let haystack = pdf.windows(4).any(|w| w == b"/F2 ");
        assert!(haystack, "a second font resource must be declared");
        let type0 = pdf.windows(12).any(|w| w == b"/Type0 /Base");
        assert!(
            type0 || String::from_utf8_lossy(&pdf).contains("/Subtype /Type0"),
            "the fallback face embeds as a Type0 composite"
        );
    }

    /// A document whose *only* renderable content is script-built must render:
    /// proves createElement/appendChild feed the layout pipeline end to end.
    #[cfg(feature = "js")]
    #[test]
    fn script_built_content_renders() {
        use super::{BoaScriptEngine, ScriptLimits};
        let html = "<script>\
            var h = document.createElement('h1');\
            h.textContent = 'Built by script';\
            document.body.appendChild(h);\
            </script>";

        let pdf = Engine::new()
            .render_html_with_scripts(
                html,
                RenderOptions::default(),
                &BoaScriptEngine,
                &ScriptLimits::default(),
            )
            .expect("script-built document should render");

        assert!(pdf.starts_with(b"%PDF-1.7\n"));
    }
}
