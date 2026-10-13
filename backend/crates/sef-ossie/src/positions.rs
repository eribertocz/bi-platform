//! Where each node of a document is written: line and column by JSON
//! pointer. Most diagnostics are found after parsing (validation, CSMIR
//! conversion, the catalog) and only know the pointer of what they are
//! about; this index lets every one of them point at the text.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_saphyr::Spanned;

use crate::diagnostic::Diagnostic;

/// The 1-based line and column of every node of one document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Positions(BTreeMap<String, (u64, u64)>);

impl Positions {
    /// Indexes `text` (YAML or JSON) with the reader's own options; empty
    /// when it does not parse, so nothing is ever located wrongly.
    #[must_use]
    pub fn of(text: &str) -> Self {
        let mut index = BTreeMap::new();
        if let Ok(root) =
            serde_saphyr::from_str_with_options::<Spanned<Node>>(text, crate::load::options())
        {
            walk(&root, &mut String::new(), &mut index);
        }
        Self(index)
    }

    /// The node `pointer` names or, when the document lacks it (a missing
    /// member), its nearest ancestor that exists.
    #[must_use]
    pub fn locate(&self, pointer: &str) -> Option<(u64, u64)> {
        let mut pointer = pointer;
        loop {
            if let Some(position) = self.0.get(pointer) {
                return Some(*position);
            }
            pointer = &pointer[..pointer.rfind('/')?];
        }
    }

    /// Locates the diagnostics that do not know where they are.
    pub fn fill(&self, diagnostics: &mut [Diagnostic]) {
        for diagnostic in diagnostics.iter_mut() {
            if diagnostic.location.is_none() {
                diagnostic.location = self.locate(&diagnostic.path);
            }
        }
    }
}

/// A document reduced to its shape; every child keeps where it was written.
enum Node {
    Map(Vec<(String, Spanned<Node>)>),
    Seq(Vec<Spanned<Node>>),
    Scalar,
}

fn walk(node: &Spanned<Node>, pointer: &mut String, index: &mut BTreeMap<String, (u64, u64)>) {
    // The place it is used, also when it is an alias.
    let at = &node.referenced;
    index.insert(pointer.clone(), (at.line(), at.column()));
    let length = pointer.len();
    match &node.value {
        Node::Map(entries) => {
            for (key, value) in entries {
                pointer.push('/');
                // RFC 6901: `~` and `/` inside a member name.
                pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
                walk(value, pointer, index);
                pointer.truncate(length);
            }
        }
        Node::Seq(items) => {
            for (i, item) in items.iter().enumerate() {
                pointer.push('/');
                pointer.push_str(&i.to_string());
                walk(item, pointer, index);
                pointer.truncate(length);
            }
        }
        Node::Scalar => {}
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a YAML node")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_i128<E>(self, _: i128) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_u128<E>(self, _: u128) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_str<E>(self, _: &str) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_bytes<E>(self, _: &[u8]) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Node, D::Error> {
        Node::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Node::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries = Vec::new();
        while let Some(Key(key)) = map.next_key()? {
            entries.push((key, map.next_value()?));
        }
        Ok(Node::Map(entries))
    }
}

/// A member name: Ossie's are strings; a plain scalar key of another type
/// is named by its text, as a JSON pointer would.
struct Key(String);

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(KeyVisitor)
    }
}

struct KeyVisitor;

impl Visitor<'_> for KeyVisitor {
    type Value = Key;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a scalar member name")
    }

    fn visit_str<E>(self, value: &str) -> Result<Key, E> {
        Ok(Key(value.to_owned()))
    }

    fn visit_bool<E>(self, value: bool) -> Result<Key, E> {
        Ok(Key(value.to_string()))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Key, E> {
        Ok(Key(value.to_string()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Key, E> {
        Ok(Key(value.to_string()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Key, E> {
        Ok(Key(value.to_string()))
    }
}
