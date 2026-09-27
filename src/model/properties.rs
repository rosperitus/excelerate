//! Document properties: what Excel shows under File > Info, and the fields a
//! user adds under Advanced Properties > Custom.
//!
//! xlsx keeps them in three package parts: `docProps/core.xml` (Dublin Core:
//! title, author, dates), `docProps/app.xml` (the application's own: company,
//! manager, and the sheet list Excel regenerates on every save) and
//! `docProps/custom.xml`. ODS keeps the same facts in `meta.xml`, xls in the
//! `SummaryInformation` and `DocumentSummaryInformation` streams.

/// The properties of a workbook.
///
/// Every field is `Option`: a file that says nothing about its subject is not
/// a file whose subject is the empty string, and writing an empty element
/// would put a statement in it nobody made.
///
/// Dates are text in ISO 8601 (`2026-09-27T12:00:00Z`), as xlsx and ODS store
/// them; xls stores a `FILETIME`, which the reader turns into the same form.
/// Nothing updates `modified` on its own: the caller decides whether a
/// rewrite is a modification.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct DocumentProperties {
    /// `dc:title`.
    pub title: Option<String>,
    /// `dc:subject`.
    pub subject: Option<String>,
    /// `dc:creator`: the author.
    pub creator: Option<String>,
    /// `cp:keywords`, as one string; Excel separates them with `;` or `,`.
    pub keywords: Option<String>,
    /// `dc:description`: what Excel calls Comments.
    pub description: Option<String>,
    /// `cp:lastModifiedBy`.
    pub last_modified_by: Option<String>,
    /// `cp:category`.
    pub category: Option<String>,
    /// `cp:contentStatus`, such as `Draft` or `Final`.
    pub content_status: Option<String>,
    /// `dc:language`.
    pub language: Option<String>,
    /// `dc:identifier`.
    pub identifier: Option<String>,
    /// `cp:revision`.
    pub revision: Option<String>,
    /// `cp:version`.
    pub version: Option<String>,
    /// `dcterms:created`.
    pub created: Option<String>,
    /// `dcterms:modified`.
    pub modified: Option<String>,
    /// `cp:lastPrinted`.
    pub last_printed: Option<String>,
    /// `Company` in `app.xml`.
    pub company: Option<String>,
    /// `Manager` in `app.xml`.
    pub manager: Option<String>,
    /// The user's own fields, in file order.
    pub custom: Vec<CustomProperty>,
}

impl DocumentProperties {
    /// Whether the Dublin Core part has anything to say.
    #[cfg(feature = "write")]
    pub(crate) fn has_core(&self) -> bool {
        self.core_fields().iter().any(|(_, value)| value.is_some())
    }

    /// The Dublin Core fields with the element each is written as, in the
    /// order Excel writes them.
    #[cfg(feature = "write")]
    pub(crate) fn core_fields(&self) -> [(&'static str, &Option<String>); 15] {
        [
            ("dc:title", &self.title),
            ("dc:subject", &self.subject),
            ("dc:creator", &self.creator),
            ("cp:keywords", &self.keywords),
            ("dc:description", &self.description),
            ("cp:lastModifiedBy", &self.last_modified_by),
            ("cp:lastPrinted", &self.last_printed),
            ("dcterms:created", &self.created),
            ("dcterms:modified", &self.modified),
            ("cp:category", &self.category),
            ("cp:contentStatus", &self.content_status),
            ("dc:language", &self.language),
            ("dc:identifier", &self.identifier),
            ("cp:revision", &self.revision),
            ("cp:version", &self.version),
        ]
    }

    /// The field an element of the Dublin Core part fills, by local name.
    pub(crate) fn core_field_mut(&mut self, local: &str) -> Option<&mut Option<String>> {
        Some(match local {
            "title" => &mut self.title,
            "subject" => &mut self.subject,
            "creator" => &mut self.creator,
            "keywords" => &mut self.keywords,
            "description" => &mut self.description,
            "lastModifiedBy" => &mut self.last_modified_by,
            "lastPrinted" => &mut self.last_printed,
            "created" => &mut self.created,
            "modified" => &mut self.modified,
            "category" => &mut self.category,
            "contentStatus" => &mut self.content_status,
            "language" => &mut self.language,
            "identifier" => &mut self.identifier,
            "revision" => &mut self.revision,
            "version" => &mut self.version,
            _ => return None,
        })
    }

    /// Sets a custom field, replacing one of the same name (compared without
    /// regard to case, as Excel does) or adding it at the end.
    pub fn set_custom(&mut self, name: impl Into<String>, value: impl Into<PropertyValue>) {
        let (name, value) = (name.into(), value.into());
        match self.custom.iter_mut().find(|p| same_name(&p.name, &name)) {
            Some(existing) => existing.value = value,
            None => self.custom.push(CustomProperty { name, value }),
        }
    }

    /// The value of a custom field, by name without regard to case.
    #[must_use]
    pub fn custom(&self, name: &str) -> Option<&PropertyValue> {
        self.custom
            .iter()
            .find(|p| same_name(&p.name, name))
            .map(|p| &p.value)
    }
}

/// Names compared without regard to case in any script: Excel treats
/// `Отдел` and `отдел` as one field, and ASCII folding would not.
fn same_name(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// One of the user's own fields.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomProperty {
    /// The name shown in Excel's list.
    pub name: String,
    /// The value, with its type.
    pub value: PropertyValue,
}

/// The value of a custom field. Excel offers four types: text, number, date
/// and yes/no; a number without a fraction is kept as an integer, since the
/// file tells the two apart.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PropertyValue {
    /// Text.
    Text(String),
    /// A whole number.
    Integer(i64),
    /// A number with a fraction.
    Number(f64),
    /// Yes or no.
    Bool(bool),
    /// A moment, in ISO 8601 as the other dates.
    Date(String),
}

impl From<&str> for PropertyValue {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for PropertyValue {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<i64> for PropertyValue {
    fn from(n: i64) -> Self {
        Self::Integer(n)
    }
}

impl From<f64> for PropertyValue {
    fn from(n: f64) -> Self {
        Self::Number(n)
    }
}

impl From<bool> for PropertyValue {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}
