use crate::{BaseDocument, Document, DocumentConfig, DocumentMutator, PlainDocument};
use blitz_traits::node_id::NodeId;

pub trait HtmlParserProvider {
    fn parse_inner_html<'m, 'doc>(
        &self,
        mutr: &'m mut DocumentMutator<'doc>,
        element_id: NodeId,
        html: &str,
    );

    /// Parse a full HTML document (e.g. the contents of an `<iframe>`).
    ///
    /// The default implementation ignores the HTML and returns an empty document.
    fn parse_document(&self, html: &str, config: DocumentConfig) -> Box<dyn Document> {
        let _ = html;
        Box::new(PlainDocument(BaseDocument::new(config)))
    }

    /// pyro: parse a standalone SVG document as XML (an SVG image with a `<foreignObject>`, which
    /// is rendered as a document: `BaseDocument::svg_image_documents`). `None`: not supported.
    fn parse_svg_document(&self, svg: &str, config: DocumentConfig) -> Option<Box<dyn Document>> {
        let _ = (svg, config);
        None
    }
}

pub struct DummyHtmlParserProvider;
impl HtmlParserProvider for DummyHtmlParserProvider {
    fn parse_inner_html<'m, 'doc>(
        &self,
        mutr: &'m mut DocumentMutator<'doc>,
        element_id: NodeId,
        html: &str,
    ) {
        let _ = mutr;
        let _ = element_id;
        let _ = html;
        // Do nothing for now
        //
        // TODO: do something:
        // - Print warning?
        // - Parse HTML as plain text?
    }
}
