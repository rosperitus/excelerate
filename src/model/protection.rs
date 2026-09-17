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

    /// What Excel itself spins SHA-512 when it sets a password.
    pub const EXCEL_SPIN_COUNT: u32 = 100_000;

    /// The hash Excel writes when a password is set: SHA-512 over a fresh
    /// sixteen-byte salt, spun [`Self::EXCEL_SPIN_COUNT`] times.
    ///
    /// `None` where the platform has no source of randomness to salt with;
    /// [`Self::iso`] takes a salt from the caller instead.
    #[must_use]
    pub fn new(password: &str) -> Option<Self> {
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).ok()?;
        Some(Self::iso(password, &salt, Self::EXCEL_SPIN_COUNT))
    }

    /// The SHA-512 hash of `password` over `salt`, as ECMA-376 defines it: the
    /// salt and the password in UTF-16LE hashed once, then the hash and the
    /// iteration number hashed `spin_count` times more.
    #[must_use]
    pub fn iso(password: &str, salt: &[u8], spin_count: u32) -> Self {
        Self::Iso {
            algorithm: "SHA-512".to_owned(),
            hash: base64(&spin::<sha2::Sha512>(password, salt, spin_count)),
            salt: base64(salt),
            spin_count,
        }
    }

    /// The 16-bit verifier of the old method, which older Excel and many
    /// other programs still write.
    #[must_use]
    pub fn legacy(password: &str) -> Self {
        Self::Legacy(format!("{:04X}", legacy_verifier(password)))
    }

    /// Whether `password` is the one this hash was made from.
    ///
    /// The ISO form knows SHA-512, SHA-384, SHA-256 and SHA-1; a hash under
    /// any other algorithm, or with a salt that is not base64, never matches.
    #[must_use]
    pub fn verify(&self, password: &str) -> bool {
        match self {
            Self::Legacy(verifier) => {
                u16::from_str_radix(verifier, 16).is_ok_and(|v| v == legacy_verifier(password))
            }
            Self::Iso {
                algorithm,
                hash,
                salt,
                spin_count,
            } => {
                let Some(salt) = unbase64(salt) else {
                    return false;
                };
                let got = match algorithm.to_ascii_uppercase().as_str() {
                    "SHA-512" => spin::<sha2::Sha512>(password, &salt, *spin_count),
                    "SHA-384" => spin::<sha2::Sha384>(password, &salt, *spin_count),
                    "SHA-256" => spin::<sha2::Sha256>(password, &salt, *spin_count),
                    "SHA-1" => spin::<sha1::Sha1>(password, &salt, *spin_count),
                    _ => return false,
                };
                base64(&got) == *hash
            }
        }
    }

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

/// The iterated salted hash of ECMA-376 Part 1, 18.2.28.
fn spin<D: sha2::Digest>(password: &str, salt: &[u8], spin_count: u32) -> Vec<u8> {
    let mut first = D::new();
    first.update(salt);
    for unit in password.encode_utf16() {
        first.update(unit.to_le_bytes());
    }
    let mut hash = first.finalize().to_vec();
    for i in 0..spin_count {
        let mut next = D::new();
        next.update(&hash);
        next.update(i.to_le_bytes());
        hash = next.finalize().to_vec();
    }
    hash
}

/// The verifier of the old method (ECMA-376 Part 4, 3.3.1.81): each character
/// rotated in from the last, then the length and a constant.
///
/// Excel reads the password as single bytes of the ANSI code page; the low
/// byte of each UTF-16 unit is that byte for everything Latin-1 spells.
fn legacy_verifier(password: &str) -> u16 {
    let bytes: Vec<u16> = password.encode_utf16().map(|u| u & 0xFF).collect();
    let mut hash: u16 = 0;
    for &byte in bytes.iter().rev() {
        hash = ((hash >> 14) & 1) | ((hash << 1) & 0x7FFF);
        hash ^= byte;
    }
    hash = ((hash >> 14) & 1) | ((hash << 1) & 0x7FFF);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the old method hashes the length modulo 2^16"
    )]
    let length = bytes.len() as u16;
    hash ^ length ^ 0xCE4B
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, which is how the file spells hash and salt.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(BASE64[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The reverse; `None` for anything that is not base64.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut n, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let digit = BASE64.iter().position(|&d| d == c)?;
        n = (n << 6) | u32::try_from(digit).ok()?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((n >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
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
    use super::{PasswordAttrs, PasswordHash, SheetProtection, base64, unbase64};

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

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\x00\x10"] {
            assert_eq!(unbase64(&base64(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fo"), "Zm8=");
    }

    #[test]
    fn a_hash_verifies_its_own_password_only() {
        let hash = PasswordHash::iso("secret", b"0123456789abcdef", 1000);
        assert!(hash.verify("secret"));
        assert!(!hash.verify("Secret"));
        let hash = PasswordHash::legacy("secret");
        assert!(hash.verify("secret"));
        assert!(!hash.verify("other"));
    }
}
