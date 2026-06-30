mod box_tree;
mod color;
mod dom;
mod font;
mod html;
mod layout;
pub mod paint;
mod pdf;
mod subset;

use std::fmt;

pub use font::FontSource;
pub use layout::{PageSize, RenderOptions};

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
        let document = html::parse(html);
        let has_flow = document.flow.as_ref().is_some_and(|flow| flow.has_text());
        if document.blocks.is_empty() && !has_flow {
            return Err(Error::EmptyDocument);
        }

        let options = options.with_document_hints(&document);
        let pages = layout::layout_document(&document, &options);
        pdf::write_pdf(&pages, &options).map_err(Into::into)
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
}
