//! SVGTests list reflection. A value supplied through the list API is one
//! item even when it contains the separator; a subsequent attribute write
//! parses the serialized string again. All wrappers share this DOM state.

use crate::{Dom, DomResult, LocalName, NodeId};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringListAttribute {
    RequiredExtensions,
    SystemLanguage,
}

impl StringListAttribute {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "requiredExtensions" => Some(Self::RequiredExtensions),
            "systemLanguage" => Some(Self::SystemLanguage),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::RequiredExtensions => "requiredExtensions",
            Self::SystemLanguage => "systemLanguage",
        }
    }

    fn parse(self, value: &str) -> Vec<String> {
        match self {
            Self::RequiredExtensions => value.split_ascii_whitespace().map(str::to_owned).collect(),
            Self::SystemLanguage => value.split(',').map(|s| s.trim_ascii().to_owned()).collect(),
        }
    }
}

impl Dom {
    pub fn svg_string_list(&mut self, id: NodeId, kind: StringListAttribute) -> DomResult<&[String]> {
        let key = (self.check(id)?, kind);
        if !self.svg_strings.contains_key(&key) {
            let parsed = self.get_attribute_ns(id, None, kind.name())?.map(|v| kind.parse(v)).unwrap_or_default();
            self.svg_strings.insert(key, parsed);
        }
        Ok(&self.svg_strings[&key])
    }

    pub fn set_svg_string_list(&mut self, id: NodeId, kind: StringListAttribute, items: Vec<String>) -> DomResult<()> {
        let key = (self.check(id)?, kind);
        if items.is_empty() {
            self.remove_attribute_ns(id, None, kind.name())?;
        } else {
            let separator = match kind {
                StringListAttribute::RequiredExtensions => " ",
                StringListAttribute::SystemLanguage => ",",
            };
            self.set_attribute_local(id, &LocalName::from(kind.name()), &items.join(separator))?;
        }
        self.svg_strings.insert(key, items);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

    #[test]
    fn list_items_preserve_separators_until_the_content_attribute_is_written() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "about:blank");
        let id = dom.create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "g").unwrap();
        let kind = StringListAttribute::RequiredExtensions;
        dom.set_svg_string_list(id, kind, vec!["first second".into(), "".into()]).unwrap();
        assert_eq!(dom.get_attribute(id, "requiredExtensions").unwrap(), Some("first second "));
        dom.set_attribute_ns(id, Some("urn:other"), "requiredExtensions", "unrelated").unwrap();
        assert_eq!(dom.svg_string_list(id, kind).unwrap(), ["first second", ""]);
        dom.set_attribute(id, "requiredExtensions", "first second ").unwrap();
        assert_eq!(dom.svg_string_list(id, kind).unwrap(), ["first", "second"]);
        dom.remove_attribute_ns(id, None, "requiredExtensions").unwrap();
        assert!(dom.svg_string_list(id, kind).unwrap().is_empty());
    }
}
