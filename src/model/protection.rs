//! What a sheet or a workbook forbids.
//!
//!
//! None of this is encryption: the file stays readable, and a reader that
//! chooses to ignore the flags can write every cell. It is a record of what
//! the user asked Excel to refuse, and losing it on a rewrite silently unlocks
//! the sheet.

use crate::coordinate::Range;

/// A password as a file stores it - a hash, never the text that was typed.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordHash {
    /// `password="CC1A"`: the 16-bit verifier of the old method, in hex.
    ///
    /// Two different passwords collide in it routinely - it never was a
    /// security measure.
    Legacy(String),
    /// The ISO method: a named hash iterated `spin_count` times over a salt.
    Iso {
        /// `algorithmName`, spelled as Excel spells it: `SHA-512`, `MD5`.
        algorithm: String,
        /// `hashValue`, base64.
        hash: String,
        /// `saltValue`, base64.
        salt: String,
        /// `spinCount`: how many times the hash was re-applied.
        spin_count: u32,
    },
}

/// Which attributes hold the password of one element.
///
/// `<sheetProtection>` and `<protectedRange>` name theirs `password`, the
/// workbook prefixes both of its two with `workbook` and `revisions`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PasswordAttrs {
    /// Attribute holding the legacy verifier.
    pub legacy: &'static str,
    /// Attribute holding the algorithm name.
    pub algorithm: &'static str,
    /// Attribute holding the hash.
    pub hash: &'static str,
    /// Attribute holding the salt.
    pub salt: &'static str,
    /// Attribute holding the iteration count.
    pub spin_count: &'static str,
}

impl PasswordAttrs {
    /// The unprefixed names, used by `<sheetProtection>` and
    /// `<protectedRange>`.
    pub(crate) const PLAIN: Self = Self {
        legacy: "password",
        algorithm: "algorithmName",
        hash: "hashValue",
        salt: "saltValue",
        spin_count: "spinCount",
    };

    /// The names `<workbookProtection>` uses for the workbook password.
    pub(crate) const WORKBOOK: Self = Self {
        legacy: "workbookPassword",
        algorithm: "workbookAlgorithmName",
        hash: "workbookHashValue",
        salt: "workbookSaltValue",
        spin_count: "workbookSpinCount",
    };

    /// The names `<workbookProtection>` uses for the revisions password.
    pub(crate) const REVISIONS: Self = Self {
        legacy: "revisionsPassword",
        algorithm: "revisionsAlgorithmName",
        hash: "revisionsHashValue",
        salt: "revisionsSaltValue",
        spin_count: "revisionsSpinCount",
    };
}

impl PasswordHash {
    /// Default `spinCount` when the attribute is absent, as
    /// `Protection::$spinCount` has it.
    pub const DEFAULT_SPIN_COUNT: u32 = 10_000;

    /// Builds the hash from the attributes of an element, given how that
    /// element spells them.
    ///
    /// The ISO form wins when an algorithm is named: Excel writes one form or
    /// the other, and a file carrying both means the newer one.
    pub(crate) fn from_attrs(
        attrs: PasswordAttrs,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Option<Self> {
        let algorithm = lookup(attrs.algorithm).filter(|v| !v.is_empty());
        if let Some(algorithm) = algorithm {
            return Some(Self::Iso {
                algorithm,
                hash: lookup(attrs.hash).unwrap_or_default(),
                salt: lookup(attrs.salt).unwrap_or_default(),
                spin_count: lookup(attrs.spin_count)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(Self::DEFAULT_SPIN_COUNT),
            });
        }
        lookup(attrs.legacy)
            .filter(|v| !v.is_empty())
            .map(Self::Legacy)
    }

    /// The attributes to write, in the order the schema lists them.
    #[cfg(feature = "write")]
    pub(crate) fn to_attrs(&self, attrs: PasswordAttrs) -> Vec<(&'static str, String)> {
        match self {
            Self::Legacy(verifier) => vec![(attrs.legacy, verifier.clone())],
            Self::Iso {
                algorithm,
                hash,
                salt,
                spin_count,
            } => vec![
                (attrs.algorithm, algorithm.clone()),
                (attrs.hash, hash.clone()),
                (attrs.salt, salt.clone()),
                (attrs.spin_count, spin_count.to_string()),
            ],
        }
    }
}

/// What a protected sheet refuses.
///
/// Every flag is a three-state: `Some(true)` and `Some(false)` are written
/// out, `None` leaves the attribute off and lets Excel apply its own default,
/// which differs from flag to flag. Collapsing that to a plain `bool` would
/// invent attributes the file never had.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SheetProtection {
    /// Whether protection is on at all. Everything else is moot while this is
    /// off or absent.
    pub sheet: Option<bool>,
    /// Whether drawings and comments are protected.
    pub objects: Option<bool>,
    /// Whether scenarios are protected.
    pub scenarios: Option<bool>,
    /// Whether cells may be reformatted.
    pub format_cells: Option<bool>,
    /// Whether columns may be resized or hidden.
    pub format_columns: Option<bool>,
    /// Whether rows may be resized or hidden.
    pub format_rows: Option<bool>,
    /// Whether columns may be inserted.
    pub insert_columns: Option<bool>,
    /// Whether rows may be inserted.
    pub insert_rows: Option<bool>,
    /// Whether hyperlinks may be added.
    pub insert_hyperlinks: Option<bool>,
    /// Whether columns may be deleted.
    pub delete_columns: Option<bool>,
    /// Whether rows may be deleted.
    pub delete_rows: Option<bool>,
    /// Whether the sheet may be sorted.
    pub sort: Option<bool>,
    /// Whether the auto filter may be used.
    pub auto_filter: Option<bool>,
    /// Whether pivot tables may be changed.
    pub pivot_tables: Option<bool>,
    /// Whether locked cells may be selected.
    pub select_locked_cells: Option<bool>,
    /// Whether unlocked cells may be selected.
    pub select_unlocked_cells: Option<bool>,
    /// The password that lifts the protection, as the file stored it.
    pub password: Option<PasswordHash>,
}

impl SheetProtection {
    /// Whether the element would say nothing and so is not written at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Every flag as (attribute name, value), in the order the schema fixes.
    ///
    /// The reader and the writer share this table rather than each spelling
    /// sixteen attribute names out; a flag added in one place then cannot be
    /// forgotten in the other.
    #[must_use]
    #[cfg(feature = "write")]
    pub(crate) fn flags(&self) -> [(&'static str, Option<bool>); 16] {
        [
            ("sheet", self.sheet),
            ("objects", self.objects),
            ("scenarios", self.scenarios),
            ("formatCells", self.format_cells),
            ("formatColumns", self.format_columns),
            ("formatRows", self.format_rows),
            ("insertColumns", self.insert_columns),
            ("insertRows", self.insert_rows),
            ("insertHyperlinks", self.insert_hyperlinks),
            ("deleteColumns", self.delete_columns),
            ("deleteRows", self.delete_rows),
            ("sort", self.sort),
            ("autoFilter", self.auto_filter),
            ("pivotTables", self.pivot_tables),
            ("selectLockedCells", self.select_locked_cells),
            ("selectUnlockedCells", self.select_unlocked_cells),
        ]
    }

    /// The same table, by mutable reference, for the reader to fill.
    pub(crate) fn slots(&mut self) -> [(&'static str, &mut Option<bool>); 16] {
        [
            ("sheet", &mut self.sheet),
            ("objects", &mut self.objects),
            ("scenarios", &mut self.scenarios),
            ("formatCells", &mut self.format_cells),
            ("formatColumns", &mut self.format_columns),
            ("formatRows", &mut self.format_rows),
            ("insertColumns", &mut self.insert_columns),
            ("insertRows", &mut self.insert_rows),
            ("insertHyperlinks", &mut self.insert_hyperlinks),
            ("deleteColumns", &mut self.delete_columns),
            ("deleteRows", &mut self.delete_rows),
            ("sort", &mut self.sort),
            ("autoFilter", &mut self.auto_filter),
            ("pivotTables", &mut self.pivot_tables),
            ("selectLockedCells", &mut self.select_locked_cells),
            ("selectUnlockedCells", &mut self.select_unlocked_cells),
        ]
    }
}

/// A range that stays editable while the sheet around it is protected, or one
/// with a password of its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtectedRange {
    /// The name Excel shows for the range.
    pub name: String,
    /// The cells it covers.
    pub sqref: Vec<Range>,
    /// Its own password, when it has one.
    pub password: Option<PasswordHash>,
    /// The Windows security descriptor naming who may edit it, kept as
    /// written: it is an SDDL string, not a spreadsheet concept.
    pub security_descriptor: String,
}

/// What a protected workbook refuses.
///
/// This is about the workbook's shape, not its contents: whether sheets may be
/// added, renamed or reordered, and whether the revision log may be edited.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkbookProtection {
    /// Whether the revision log is locked.
    pub lock_revision: Option<bool>,
    /// Whether the set of sheets is locked.
    pub lock_structure: Option<bool>,
    /// Whether the window layout is locked.
    pub lock_windows: Option<bool>,
    /// Password over the workbook's structure.
    pub workbook_password: Option<PasswordHash>,
    /// Password over the revision log.
    pub revisions_password: Option<PasswordHash>,
}

impl WorkbookProtection {
    /// Whether the element would say nothing and so is not written at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The three locks as (attribute name, value), in schema order.
    #[must_use]
    #[cfg(feature = "write")]
    pub(crate) fn locks(&self) -> [(&'static str, Option<bool>); 3] {
        [
            ("lockRevision", self.lock_revision),
            ("lockStructure", self.lock_structure),
            ("lockWindows", self.lock_windows),
        ]
    }

    /// The same three, by mutable reference, for the reader to fill.
    pub(crate) fn lock_slots(&mut self) -> [(&'static str, &mut Option<bool>); 3] {
        [
            ("lockRevision", &mut self.lock_revision),
            ("lockStructure", &mut self.lock_structure),
            ("lockWindows", &mut self.lock_windows),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{PasswordAttrs, PasswordHash, SheetProtection};

    /// Reads from a fixed list of attributes, the way the readers do.
    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn an_algorithm_name_selects_the_iso_form() {
        let attrs = [
            ("algorithmName", "SHA-512"),
            ("hashValue", "aGFzaA=="),
            ("saltValue", "c2FsdA=="),
            ("spinCount", "100000"),
            // Both forms present: the newer one is what the file means.
            ("password", "CC1A"),
        ];
        assert_eq!(
            PasswordHash::from_attrs(PasswordAttrs::PLAIN, lookup(&attrs)),
            Some(PasswordHash::Iso {
                algorithm: "SHA-512".into(),
                hash: "aGFzaA==".into(),
                salt: "c2FsdA==".into(),
                spin_count: 100_000,
            })
        );
    }

    #[test]
    fn a_missing_spin_count_falls_back_to_the_default() {
        let attrs = [("algorithmName", "SHA-512"), ("hashValue", "aGFzaA==")];
        assert_eq!(
            PasswordHash::from_attrs(PasswordAttrs::PLAIN, lookup(&attrs)),
            Some(PasswordHash::Iso {
                algorithm: "SHA-512".into(),
                hash: "aGFzaA==".into(),
                salt: String::new(),
                spin_count: PasswordHash::DEFAULT_SPIN_COUNT,
            })
        );
    }

    #[test]
    fn without_an_algorithm_the_legacy_verifier_is_the_password() {
        let attrs = [("password", "CC1A")];
        assert_eq!(
            PasswordHash::from_attrs(PasswordAttrs::PLAIN, lookup(&attrs)),
            Some(PasswordHash::Legacy("CC1A".into()))
        );
        assert_eq!(
            PasswordHash::from_attrs(PasswordAttrs::PLAIN, lookup(&[])),
            None
        );
        // An empty attribute is not a password: Excel writes the attribute
        // only when the string is non-empty.
        assert_eq!(
            PasswordHash::from_attrs(PasswordAttrs::PLAIN, lookup(&[("password", "")])),
            None
        );
    }

    #[test]
    fn the_workbook_spells_its_two_passwords_apart() {
        let attrs = [
            ("workbookPassword", "CC1A"),
            ("revisionsAlgorithmName", "SHA-1"),
            ("revisionsHashValue", "aA=="),
        ];
        assert_eq!(
            PasswordHash::from_attrs(PasswordAttrs::WORKBOOK, lookup(&attrs)),
            Some(PasswordHash::Legacy("CC1A".into()))
        );
        assert!(matches!(
            PasswordHash::from_attrs(PasswordAttrs::REVISIONS, lookup(&attrs)),
            Some(PasswordHash::Iso { .. })
        ));
    }

    #[test]
    #[cfg(feature = "write")]
    fn the_flag_tables_agree_on_names_and_order() {
        let mut protection = SheetProtection::default();
        let names: Vec<&str> = protection.flags().iter().map(|(n, _)| *n).collect();
        let slots: Vec<&str> = protection.slots().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, slots);
    }
}
