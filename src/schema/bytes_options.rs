use serde::{Deserialize, Serialize};
use std::ops::BitOr;

use super::flags::{FastFlag, IndexedFlag, NormedFlag, SchemaFlagList, StoredFlag};
/// Define how an a bytes field should be handled by tantivy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytesOptions {
    indexed: bool,
    normed: bool,
    fast: bool,
    stored: bool,
}

impl BytesOptions {
    /// Returns true iff the value is indexed.
    pub fn is_indexed(&self) -> bool {
        self.indexed
    }

    /// Returns true iff the value is normed.
    pub fn is_normed(&self) -> bool {
        self.normed
    }

    /// Returns true iff the value is a fast field.
    pub fn is_fast(&self) -> bool {
        self.fast
    }

    /// Returns true iff the value is stored.
    pub fn is_stored(&self) -> bool {
        self.stored
    }

    /// Set the field as indexed.
    ///
    /// Setting an integer as indexed will generate
    /// a posting list for each value taken by the integer.
    pub fn set_indexed(mut self) -> BytesOptions {
        self.indexed = true;
        self
    }

    /// Set the field as normed.
    ///
    /// Setting an integer as normed will generate
    /// the fieldnorm data for it.
    pub fn set_normed(mut self) -> BytesOptions {
        self.normed = true;
        self
    }

    /// Set the field as a single-valued fast field.
    ///
    /// Fast fields are designed for random access.
    /// Access time are similar to a random lookup in an array.
    /// If more than one value is associated to a fast field, only the last one is
    /// kept.
    pub fn set_fast(mut self) -> BytesOptions {
        self.fast = true;
        self
    }

    /// Set the field as stored.
    ///
    /// Only the fields that are set as *stored* are
    /// persisted into the Tantivy's store.
    pub fn set_stored(mut self) -> BytesOptions {
        self.stored = true;
        self
    }
}

impl Default for BytesOptions {
    fn default() -> BytesOptions {
        BytesOptions {
            indexed: false,
            normed: false,
            fast: false,
            stored: false,
        }
    }
}

impl<T: Into<BytesOptions>> BitOr<T> for BytesOptions {
    type Output = BytesOptions;

    fn bitor(self, other: T) -> BytesOptions {
        let other = other.into();
        BytesOptions {
            indexed: self.indexed | other.indexed,
            normed: self.normed | other.normed,
            stored: self.stored | other.stored,
            fast: self.fast | other.fast,
        }
    }
}

impl From<()> for BytesOptions {
    fn from(_: ()) -> Self {
        Self::default()
    }
}

impl From<FastFlag> for BytesOptions {
    fn from(_: FastFlag) -> Self {
        BytesOptions {
            indexed: false,
            normed: false,
            stored: false,
            fast: true,
        }
    }
}

impl From<StoredFlag> for BytesOptions {
    fn from(_: StoredFlag) -> Self {
        BytesOptions {
            indexed: false,
            normed: false,
            stored: true,
            fast: false,
        }
    }
}

impl From<IndexedFlag> for BytesOptions {
    fn from(_: IndexedFlag) -> Self {
        BytesOptions {
            indexed: true,
            normed: false,
            stored: false,
            fast: false,
        }
    }
}

impl From<NormedFlag> for BytesOptions {
    fn from(_: NormedFlag) -> Self {
        BytesOptions {
            indexed: false,
            normed: true,
            stored: false,
            fast: false,
        }
    }
}

impl<Head, Tail> From<SchemaFlagList<Head, Tail>> for BytesOptions
where
    Head: Clone,
    Tail: Clone,
    Self: BitOr<Output = Self> + From<Head> + From<Tail>,
{
    fn from(head_tail: SchemaFlagList<Head, Tail>) -> Self {
        Self::from(head_tail.head) | Self::from(head_tail.tail)
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::{BytesOptions, FAST, INDEXED, NORMED, STORED};

    #[test]
    fn test_bytes_option_fast_flag() {
        assert_eq!(BytesOptions::default().set_fast(), FAST.into());
        assert_eq!(BytesOptions::default().set_indexed(), INDEXED.into());
        assert_eq!(BytesOptions::default().set_normed(), NORMED.into());
        assert_eq!(BytesOptions::default().set_stored(), STORED.into());
    }
    #[test]
    fn test_bytes_option_fast_flag_composition() {
        assert_eq!(
            BytesOptions::default().set_fast().set_stored(),
            (FAST | STORED).into()
        );
        assert_eq!(
            BytesOptions::default().set_indexed().set_fast(),
            (INDEXED | FAST).into()
        );
        assert_eq!(
            BytesOptions::default().set_stored().set_indexed(),
            (STORED | INDEXED).into()
        );
    }

    #[test]
    fn test_bytes_option_fast_() {
        assert!(!BytesOptions::default().is_stored());
        assert!(!BytesOptions::default().is_fast());
        assert!(!BytesOptions::default().is_indexed());
        assert!(!BytesOptions::default().is_normed());
        assert!(BytesOptions::default().set_stored().is_stored());
        assert!(BytesOptions::default().set_fast().is_fast());
        assert!(BytesOptions::default().set_indexed().is_indexed());
        assert!(BytesOptions::default().set_normed().is_normed());
    }
}
