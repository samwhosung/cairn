/// How a field's four bytes are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    UInt32,
    Int32,
    Float32,
    /// An offset into the string block.
    String,
}

/// A named column, or an inline array of `count` consecutive columns of one type.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    pub ty: FieldType,
    pub count: usize,
}

impl SchemaField {
    pub fn new(name: impl Into<String>, ty: FieldType) -> Self {
        Self::new_array(name, ty, 1)
    }

    pub fn new_array(name: impl Into<String>, ty: FieldType, count: usize) -> Self {
        Self {
            name: name.into(),
            ty,
            count,
        }
    }
}

/// The columns of a DBC, which the file does not describe.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub name: String,
    pub fields: Vec<SchemaField>,
    pub key_field: Option<String>,
}

impl Schema {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
            key_field: None,
        }
    }

    pub fn add_field(&mut self, field: SchemaField) {
        self.fields.push(field);
    }

    pub fn set_key_field(&mut self, name: impl Into<String>) {
        self.key_field = Some(name.into());
    }

    pub(crate) fn expanded_len(&self) -> usize {
        self.fields.iter().map(|f| f.count).sum()
    }
}
