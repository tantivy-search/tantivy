use crate::schema::FacetOptions;
use crate::schema::TextOptions;
use crate::schema::{is_valid_field_name, IntOptions};

use crate::schema::bytes_options::BytesOptions;
use crate::schema::FieldType;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A `FieldEntry` represents a field and its configuration.
/// `Schema` are a collection of `FieldEntry`
///
/// It consists of
/// - a field name
/// - a field type, itself wrapping up options describing
/// how the field should be indexed.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldEntry {
    name: String,
    field_type: FieldType,
}

impl FieldEntry {
    /// Creates a new field entry given a name and a field type
    pub fn new(field_name: String, field_type: FieldType) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type,
        }
    }

    /// Creates a new u64 field entry in the schema, given
    /// a name, and some options.
    pub fn new_text(field_name: String, text_options: TextOptions) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type: FieldType::Str(text_options),
        }
    }

    /// Creates a new u64 field entry in the schema, given
    /// a name, and some options.
    pub fn new_u64(field_name: String, field_type: IntOptions) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type: FieldType::U64(field_type),
        }
    }

    /// Creates a new i64 field entry in the schema, given
    /// a name, and some options.
    pub fn new_i64(field_name: String, field_type: IntOptions) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type: FieldType::I64(field_type),
        }
    }

    /// Creates a new f64 field entry in the schema, given
    /// a name, and some options.
    pub fn new_f64(field_name: String, field_type: IntOptions) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type: FieldType::F64(field_type),
        }
    }

    /// Creates a new date field entry in the schema, given
    /// a name, and some options.
    pub fn new_date(field_name: String, field_type: IntOptions) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type: FieldType::Date(field_type),
        }
    }

    /// Creates a field entry for a facet.
    pub fn new_facet(field_name: String, field_type: FacetOptions) -> FieldEntry {
        assert!(is_valid_field_name(&field_name));
        FieldEntry {
            name: field_name,
            field_type: FieldType::HierarchicalFacet(field_type),
        }
    }

    /// Creates a field entry for a bytes field
    pub fn new_bytes(field_name: String, bytes_type: BytesOptions) -> FieldEntry {
        FieldEntry {
            name: field_name,
            field_type: FieldType::Bytes(bytes_type),
        }
    }

    /// Returns the name of the field
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the field type
    pub fn field_type(&self) -> &FieldType {
        &self.field_type
    }

    /// Returns true iff the field is indexed
    pub fn is_indexed(&self) -> bool {
        match self.field_type {
            FieldType::Str(ref options) => options.get_indexing_options().is_some(),
            FieldType::U64(ref options)
            | FieldType::I64(ref options)
            | FieldType::F64(ref options)
            | FieldType::Date(ref options) => options.is_indexed(),
            FieldType::HierarchicalFacet(ref options) => options.is_indexed(),
            FieldType::Bytes(ref options) => options.is_indexed(),
        }
    }

    /// Returns true iff the field is normed
    pub fn is_normed(&self) -> bool {
        self.field_type.is_normed()
    }

    /// Returns true iff the field is a int (signed or unsigned) fast field
    pub fn is_fast(&self) -> bool {
        match self.field_type {
            FieldType::U64(ref options)
            | FieldType::I64(ref options)
            | FieldType::Date(ref options)
            | FieldType::F64(ref options) => options.is_fast(),
            _ => false,
        }
    }

    /// Returns true iff the field is stored
    pub fn is_stored(&self) -> bool {
        match self.field_type {
            FieldType::U64(ref options)
            | FieldType::I64(ref options)
            | FieldType::F64(ref options)
            | FieldType::Date(ref options) => options.is_stored(),
            FieldType::Str(ref options) => options.is_stored(),
            FieldType::HierarchicalFacet(ref options) => options.is_stored(),
            FieldType::Bytes(ref options) => options.is_stored(),
        }
    }
}

impl Serialize for FieldEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("field_entry", 3)?;
        s.serialize_field("name", &self.name)?;

        match self.field_type {
            FieldType::Str(ref options) => {
                s.serialize_field("type", "text")?;
                s.serialize_field("options", options)?;
            }
            FieldType::U64(ref options) => {
                s.serialize_field("type", "u64")?;
                s.serialize_field("options", options)?;
            }
            FieldType::I64(ref options) => {
                s.serialize_field("type", "i64")?;
                s.serialize_field("options", options)?;
            }
            FieldType::F64(ref options) => {
                s.serialize_field("type", "f64")?;
                s.serialize_field("options", options)?;
            }
            FieldType::Date(ref options) => {
                s.serialize_field("type", "date")?;
                s.serialize_field("options", options)?;
            }
            FieldType::HierarchicalFacet(ref options) => {
                s.serialize_field("type", "hierarchical_facet")?;
                s.serialize_field("options", options)?;
            }
            FieldType::Bytes(ref options) => {
                s.serialize_field("type", "bytes")?;
                s.serialize_field("options", options)?;
            }
        }

        s.end()
    }
}

impl<'de> Deserialize<'de> for FieldEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Name,
            Type,
            Options,
        }

        const FIELDS: &[&str] = &["name", "type", "options"];

        struct FieldEntryVisitor;

        impl<'de> Visitor<'de> for FieldEntryVisitor {
            type Value = FieldEntry;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("struct FieldEntry")
            }

            fn visit_map<V>(self, mut map: V) -> Result<FieldEntry, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut name = None;
                let mut ty = None;
                let mut field_type = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Name => {
                            if name.is_some() {
                                return Err(de::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        Field::Type => {
                            if ty.is_some() {
                                return Err(de::Error::duplicate_field("type"));
                            }
                            let type_string = map.next_value::<String>()?;
                            match type_string.as_str() {
                                "text" | "u64" | "i64" | "f64" | "date" | "bytes"
                                | "hierarchical_facet" => {
                                    // These types require additional options to create a field_type
                                }
                                _ => panic!("unhandled type"),
                            }
                            ty = Some(type_string);
                        }
                        Field::Options => match ty {
                            None => {
                                let msg = "The `type` field must be \
                                           specified before `options`";
                                return Err(de::Error::custom(msg));
                            }
                            Some(ref ty) => match ty.as_str() {
                                "text" => field_type = Some(FieldType::Str(map.next_value()?)),
                                "u64" => field_type = Some(FieldType::U64(map.next_value()?)),
                                "i64" => field_type = Some(FieldType::I64(map.next_value()?)),
                                "f64" => field_type = Some(FieldType::F64(map.next_value()?)),
                                "date" => field_type = Some(FieldType::Date(map.next_value()?)),
                                "bytes" => field_type = Some(FieldType::Bytes(map.next_value()?)),
                                "hierarchical_facet" => {
                                    field_type =
                                        Some(FieldType::HierarchicalFacet(map.next_value()?))
                                }
                                _ => {
                                    let msg = format!("Unrecognised type {}", ty);
                                    return Err(de::Error::custom(msg));
                                }
                            },
                        },
                    }
                }

                let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
                ty.ok_or_else(|| de::Error::missing_field("ty"))?;
                let field_type = field_type.ok_or_else(|| de::Error::missing_field("options"))?;

                Ok(FieldEntry { name, field_type })
            }
        }

        deserializer.deserialize_struct("field_entry", FIELDS, FieldEntryVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::TEXT;
    use serde_json;

    #[test]
    #[should_panic]
    fn test_invalid_field_name_should_panic() {
        FieldEntry::new_text("-hello".to_string(), TEXT);
    }

    #[test]
    fn test_json_serialization() {
        let field_value = FieldEntry::new_text(String::from("title"), TEXT);

        let expected = r#"{
  "name": "title",
  "type": "text",
  "options": {
    "indexing": {
      "record": "position",
      "tokenizer": "default"
    },
    "stored": false
  }
}"#;
        let field_value_json = serde_json::to_string_pretty(&field_value).unwrap();

        assert_eq!(expected, &field_value_json);

        let field_value: FieldEntry = serde_json::from_str(expected).unwrap();

        assert_eq!("title", field_value.name);

        match field_value.field_type {
            FieldType::Str(_) => {}
            _ => panic!("expected FieldType::Str"),
        }
    }
}
