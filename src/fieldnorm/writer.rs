use crate::{indexer::doc_id_mapping::DocIdMapping, DocId};

use super::fieldnorm_to_id;
use super::FieldNormsSerializer;
use crate::schema::Field;
use crate::schema::Schema;
use std::{io, iter};

/// The `FieldNormsWriter` is in charge of tracking the fieldnorm byte
/// of each document for each field with field norms.
///
/// `FieldNormsWriter` stores a Vec<u8> for each tracked field, using a
/// byte per document per field.
pub struct FieldNormsWriter {
    fields: Vec<Field>,
    fieldnorms_buffer: Vec<Vec<u8>>,
}

impl FieldNormsWriter {
    /// Returns the fields that should have field norms computed
    /// according to the given schema.
    pub(crate) fn fields_with_fieldnorm(schema: &Schema) -> Vec<Field> {
        schema
            .fields()
            .filter_map(|(field, field_entry)| {
                if field_entry.is_indexed() && field_entry.is_normed() {
                    Some(field)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }

    /// Initialize with state for tracking the field norm fields
    /// specified in the schema.
    pub fn for_schema(schema: &Schema) -> FieldNormsWriter {
        let fields = FieldNormsWriter::fields_with_fieldnorm(schema);
        let max_field = fields
            .iter()
            .map(Field::field_id)
            .max()
            .map(|max_field_id| max_field_id as usize + 1)
            .unwrap_or(0);
        FieldNormsWriter {
            fields,
            fieldnorms_buffer: iter::repeat_with(Vec::new)
                .take(max_field)
                .collect::<Vec<_>>(),
        }
    }

    /// The memory used inclusive childs
    pub fn mem_usage(&self) -> usize {
        self.fieldnorms_buffer
            .iter()
            .map(|buf| buf.capacity())
            .sum()
    }
    /// Ensure that all documents in 0..max_doc have a byte associated with them
    /// in each of the fieldnorm vectors.
    ///
    /// Will extend with 0-bytes for documents that have not been seen.
    pub fn fill_up_to_max_doc(&mut self, max_doc: DocId) {
        for field in self.fields.iter() {
            self.fieldnorms_buffer[field.field_id() as usize].resize(max_doc as usize, 0u8);
        }
    }

    /// Set the fieldnorm byte for the given document for the given field.
    ///
    /// Will internally convert the u32 `fieldnorm` value to the appropriate byte
    /// to approximate the field norm in less space.
    ///
    /// * doc       - the document id
    /// * field     - the field being set
    /// * fieldnorm - the number of terms present in document `doc` in field `field`
    pub fn record(&mut self, doc: DocId, field: Field, fieldnorm: u32) {
        let fieldnorm_buffer: &mut Vec<u8> = &mut self.fieldnorms_buffer[field.field_id() as usize];
        assert!(
            fieldnorm_buffer.len() <= doc as usize,
            "Cannot register a given fieldnorm twice"
        );
        // we fill intermediary `DocId` as  having a fieldnorm of 0.
        fieldnorm_buffer.resize(doc as usize + 1, 0u8);
        fieldnorm_buffer[doc as usize] = fieldnorm_to_id(fieldnorm);
    }

    /// Serialize the seen fieldnorm values to the serializer for all fields.
    pub fn serialize(
        &self,
        mut fieldnorms_serializer: FieldNormsSerializer,
        doc_id_map: Option<&DocIdMapping>,
    ) -> io::Result<()> {
        for &field in self.fields.iter() {
            let fieldnorm_values: &[u8] = &self.fieldnorms_buffer[field.field_id() as usize][..];
            if let Some(doc_id_map) = doc_id_map {
                let mut mapped_fieldnorm_values = vec![];
                mapped_fieldnorm_values.resize(fieldnorm_values.len(), 0u8);
                for (new_doc_id, old_doc_id) in doc_id_map.iter_old_doc_ids().enumerate() {
                    mapped_fieldnorm_values[new_doc_id] = fieldnorm_values[old_doc_id as usize];
                }
                fieldnorms_serializer.serialize_field(field, &mapped_fieldnorm_values)?;
            } else {
                fieldnorms_serializer.serialize_field(field, fieldnorm_values)?;
            }
        }
        fieldnorms_serializer.close()?;
        Ok(())
    }
}
