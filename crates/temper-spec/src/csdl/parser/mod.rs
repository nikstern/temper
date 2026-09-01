mod elements;
mod schema;
mod xml;

use quick_xml::Reader;
use quick_xml::events::Event;

use super::types::*;
use schema::parse_schema;
use xml::{attr_str, local_name};

#[derive(Debug, thiserror::Error)]
pub enum CsdlParseError {
    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),
    /// An XML attribute could not be tokenized.
    #[error("XML attribute parse error: {0}")]
    Attribute(#[from] quick_xml::events::attributes::AttrError),
    /// A present XML attribute contained invalid entity syntax.
    #[error("invalid value for attribute '{attr}' on element '{element}': {source}")]
    InvalidAttributeValue {
        element: String,
        attr: String,
        source: quick_xml::Error,
    },
    #[error("missing required attribute '{attr}' on element '{element}'")]
    MissingAttribute { element: String, attr: String },
    #[error("unexpected element: {0}")]
    UnexpectedElement(String),
    #[error("invalid CSDL: {0}")]
    Invalid(String),
}

/// Parse a CSDL XML document from a string.
pub fn parse_csdl(xml: &str) -> Result<CsdlDocument, CsdlParseError> {
    parse_csdl_with_compatibility(xml, false)
}

/// Parse CSDL with the frozen v1 bundle parser semantics.
pub(crate) fn parse_csdl_frozen_v1(xml: &str) -> Result<CsdlDocument, CsdlParseError> {
    let mut document = parse_csdl_with_compatibility(xml, true)?;
    for schema in &mut document.schemas {
        for entity in &mut schema.entity_types {
            freeze_empty_collections(&mut entity.annotations);
        }
        for action in &mut schema.actions {
            freeze_empty_collections(&mut action.annotations);
        }
        for function in &mut schema.functions {
            freeze_empty_collections(&mut function.annotations);
        }
    }
    Ok(document)
}

fn freeze_empty_collections(annotations: &mut [Annotation]) {
    for annotation in annotations {
        if matches!(&annotation.value, AnnotationValue::Collection(items) if items.is_empty()) {
            annotation.value = AnnotationValue::String(String::new());
        }
    }
}

fn parse_csdl_with_compatibility(
    xml: &str,
    frozen_v1: bool,
) -> Result<CsdlDocument, CsdlParseError> {
    let mut reader = Reader::from_str(xml);
    let mut doc = CsdlDocument {
        version: String::new(),
        schemas: Vec::new(),
    };

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match local_name(e).as_str() {
                "Edmx" => doc.version = attr_str(e, "Version")?.unwrap_or_default(),
                "Schema" => doc.schemas.push(parse_schema(&mut reader, e, frozen_v1)?),
                _ => {}
            },
            Ok(Event::Empty(ref e)) => {
                if local_name(e) == "Edmx" {
                    doc.version = attr_str(e, "Version")?.unwrap_or_default();
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(CsdlParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }
    Ok(doc)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
