//! Title, author, dates and the user's own fields.

use super::Book;
#[cfg(feature = "write")]
use super::convert::field;
use super::convert::{list, object};
use crate::model::{DocumentProperties, PropertyValue};
use wasm_bindgen::prelude::*;

/// The text fields, by the names the JS side uses.
const KEYS: [&str; 17] = [
    "title",
    "subject",
    "creator",
    "keywords",
    "description",
    "lastModifiedBy",
    "category",
    "contentStatus",
    "language",
    "identifier",
    "revision",
    "version",
    "created",
    "modified",
    "lastPrinted",
    "company",
    "manager",
];

/// The model field a JS key names.
fn text_mut<'a>(props: &'a mut DocumentProperties, key: &str) -> Option<&'a mut Option<String>> {
    Some(match key {
        "title" => &mut props.title,
        "subject" => &mut props.subject,
        "creator" => &mut props.creator,
        "keywords" => &mut props.keywords,
        "description" => &mut props.description,
        "lastModifiedBy" => &mut props.last_modified_by,
        "category" => &mut props.category,
        "contentStatus" => &mut props.content_status,
        "language" => &mut props.language,
        "identifier" => &mut props.identifier,
        "revision" => &mut props.revision,
        "version" => &mut props.version,
        "created" => &mut props.created,
        "modified" => &mut props.modified,
        "lastPrinted" => &mut props.last_printed,
        "company" => &mut props.company,
        "manager" => &mut props.manager,
        _ => return None,
    })
}

#[wasm_bindgen]
impl Book {
    /// What Excel shows under File > Info: every text field is a string or
    /// `null`, dates in ISO 8601. `custom` lists the user's own fields with
    /// their type: `text`, `integer`, `number`, `boolean` or `date`.
    #[wasm_bindgen(js_name = documentProperties, unchecked_return_type = "DocumentProperties")]
    #[must_use]
    pub fn document_properties(&self) -> JsValue {
        let mut props = self.workbook.properties.clone();
        let mut fields: Vec<(&str, JsValue)> = KEYS
            .iter()
            .map(|&key| {
                let value = text_mut(&mut props, key)
                    .and_then(|v| v.as_deref())
                    .map_or(JsValue::NULL, JsValue::from_str);
                (key, value)
            })
            .collect();
        let custom = list(&self.workbook.properties.custom, |p| {
            let (kind, value) = match &p.value {
                PropertyValue::Text(t) => ("text", JsValue::from_str(t)),
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a JS number is an f64; past 2^53 it is what JS can hold"
                )]
                PropertyValue::Integer(n) => ("integer", JsValue::from_f64(*n as f64)),
                PropertyValue::Number(n) => ("number", JsValue::from_f64(*n)),
                PropertyValue::Bool(b) => ("boolean", JsValue::from_bool(*b)),
                PropertyValue::Date(d) => ("date", JsValue::from_str(d)),
            };
            object(&[
                ("name", JsValue::from_str(&p.name)),
                ("type", JsValue::from_str(kind)),
                ("value", value),
            ])
        });
        fields.push(("custom", custom));
        object(&fields)
    }
}

#[cfg(feature = "write")]
#[wasm_bindgen]
impl Book {
    /// Changes the fields the patch names and leaves the rest: a string sets
    /// one, `null` clears it. `custom`, when given, replaces the list; a
    /// field's type is taken from `type` or else from the value (a whole
    /// number is an integer).
    #[wasm_bindgen(js_name = setDocumentProperties)]
    pub fn set_document_properties(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "DocumentPropertiesPatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let mut props = self.workbook.properties.clone();
        for key in KEYS {
            let Some(value) = field(patch, key) else {
                continue;
            };
            let text = if value.is_null() {
                None
            } else {
                Some(
                    value
                        .as_string()
                        .ok_or_else(|| JsError::new(&format!("`{key}` is a string or null")))?,
                )
            };
            if let Some(slot) = text_mut(&mut props, key) {
                *slot = text;
            }
        }
        if let Some(custom) = field(patch, "custom") {
            props.custom.clear();
            for item in js_sys::Array::from(&custom).iter() {
                let name = field(&item, "name")
                    .and_then(|n| n.as_string())
                    .ok_or_else(|| JsError::new("a custom property has a `name`"))?;
                let value = field(&item, "value")
                    .ok_or_else(|| JsError::new("a custom property has a `value`"))?;
                let kind = field(&item, "type").and_then(|t| t.as_string());
                props.set_custom(name, custom_value(kind.as_deref(), &value)?);
            }
        }
        self.workbook.properties = props;
        Ok(())
    }
}

/// A custom field's value from JS, by its declared type or else its own.
#[cfg(feature = "write")]
fn custom_value(kind: Option<&str>, value: &JsValue) -> Result<PropertyValue, JsError> {
    let wrong = || JsError::new("a custom property's value does not match its type");
    Ok(match kind {
        Some("date") => PropertyValue::Date(value.as_string().ok_or_else(wrong)?),
        Some("text") => PropertyValue::Text(value.as_string().ok_or_else(wrong)?),
        Some("boolean") => PropertyValue::Bool(value.as_bool().ok_or_else(wrong)?),
        Some("number") => PropertyValue::Number(value.as_f64().ok_or_else(wrong)?),
        Some("integer") => whole(value.as_f64().ok_or_else(wrong)?).ok_or_else(wrong)?,
        Some(other) => {
            return Err(JsError::new(&format!(
                "`{other}` is not a property type: text, integer, number, boolean or date"
            )));
        }
        None => {
            if let Some(text) = value.as_string() {
                PropertyValue::Text(text)
            } else if let Some(b) = value.as_bool() {
                PropertyValue::Bool(b)
            } else if let Some(n) = value.as_f64() {
                whole(n).unwrap_or(PropertyValue::Number(n))
            } else {
                return Err(wrong());
            }
        }
    })
}

/// A number with no fraction, inside the range an i64 holds, as an integer.
#[cfg(feature = "write")]
fn whole(n: f64) -> Option<PropertyValue> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "checked whole and within 2^63 first"
    )]
    (n.fract() == 0.0 && n.abs() < 9.2e18).then_some(PropertyValue::Integer(n as i64))
}
