//! Writing form controls the sheet's VML does not hold yet.
//!
//! A control read from xlsx lives in parts that travel as bytes. One read
//! from xls, or made in code, has none, so it gets what Excel 2010 writes for
//! a new control: a shape in the sheet's VML (all Excel 2007 reads), a
//! `ctrlProp` part, and a `<control>` in the sheet behind an
//! `mc:AlternateContent` switch, which ties the two together by shape id.
//!
//! ponytail: a sheet whose VML already holds controls is left alone, so a
//! control added in code to an xlsx that had some is not written; that needs
//! matching the model against the shapes the way comment boxes are.

use super::chart::{part_text, set_part};
use super::comment::{VML_REL, VML_TYPE, column_pixels, free_name, max_shape_id, row_pixels, walk};
use super::xmlesc::escape;
use crate::coordinate::{MAX_COL, MAX_ROW};
use crate::model::chart::Anchor;
use crate::model::control::{CheckState, ControlKind, FormControl};
use crate::model::{Attachment, Spreadsheet, Worksheet};
use crate::style::{Color, Font};
use std::borrow::Cow;
use std::fmt::Write as _;

const PROPS_TYPE: &str = "application/vnd.ms-excel.controlproperties+xml";
const PROPS_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp";
const X14_NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const DRAWING_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const EMU_PER_PIXEL: i64 = 9525;

/// The shape type every control refers to, declared once per part.
const CONTROL_SHAPETYPE: &str = concat!(
    r#"<v:shapetype id="_x0000_t201" coordsize="21600,21600" o:spt="201" path="m,l,21600r21600,l21600,xe">"#,
    r#"<v:stroke joinstyle="miter"/><v:path shadowok="f" o:extrusionok="f" strokeok="f" fillok="f" o:connecttype="rect"/>"#,
    r#"<o:lock v:ext="edit" shapetype="t"/></v:shapetype>"#,
);

/// The workbook with the controls of every sheet that needs them written into
/// its parts, and for each sheet the `<controls>` element its part takes
/// (empty for most).
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> (Cow<'_, Spreadsheet>, Vec<String>) {
    let dirty: Vec<usize> = book
        .sheets()
        .iter()
        .enumerate()
        .filter(|(_, sheet)| needs_writing(&book, sheet))
        .map(|(i, _)| i)
        .collect();
    let mut elements = vec![String::new(); book.sheets().len()];
    if dirty.is_empty() {
        return (book, elements);
    }
    let mut book = book.into_owned();
    for index in dirty {
        if let Some(slot) = elements.get_mut(index) {
            *slot = apply_sheet(&mut book, index);
        }
    }
    (Cow::Owned(book), elements)
}

/// The sheet's VML part, if it has one.
fn vml_of(sheet: &Worksheet) -> Option<&str> {
    sheet
        .attachments
        .iter()
        .find(|a| a.role() == "vmlDrawing")
        .map(|a| a.target.as_str())
}

fn needs_writing(book: &Spreadsheet, sheet: &Worksheet) -> bool {
    if sheet.controls.is_empty() {
        return false;
    }
    vml_of(sheet)
        .and_then(|path| part_text(book, path))
        .is_none_or(|xml| crate::reader::vml::controls(xml).is_empty())
}

/// Writes one sheet's controls into its parts and returns its `<controls>`.
fn apply_sheet(book: &mut Spreadsheet, index: usize) -> String {
    let Some(sheet) = book.sheet(index) else {
        return String::new();
    };
    let (path, mut xml) = match vml_of(sheet) {
        Some(path) => match part_text(book, path) {
            Some(xml) => (path.to_owned(), xml.to_owned()),
            None => return String::new(),
        },
        None => (
            free_name(book),
            // The id block decides which shape ids the part may use; one per
            // sheet keeps two new parts from sharing ids.
            format!(
                concat!(
                    r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">"#,
                    r#"<o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="{}"/></o:shapelayout></xml>"#,
                ),
                index + 1
            ),
        ),
    };
    let first_id = max_shape_id(&xml).map_or(1024 * (index + 1) + 1, |id| id + 1);
    let mut shapes = String::new();
    if !xml.contains("_x0000_t201") {
        shapes.push_str(CONTROL_SHAPETYPE);
    }
    let mut placed = Vec::new();
    for (id, control) in (first_id..).zip(&sheet.controls) {
        let corners = pixel_anchor(sheet, control.anchor);
        shapes.push_str(&vml_shape(id, control, corners));
        placed.push((id, corners));
    }
    if let Some(end) = xml.rfind("</xml>") {
        xml.insert_str(end, &shapes);
    }
    let had_vml = vml_of(sheet).is_some();
    let controls = sheet.controls.clone();

    let mut targets = Vec::new();
    for control in &controls {
        let part = super::image::free_path(book, "xl/ctrlProps/ctrlProp", "xml");
        set_part(book, &part, Some(PROPS_TYPE), ctrl_prop(control));
        targets.push(part);
    }
    set_part(book, &path, Some(VML_TYPE), xml);
    let Some(sheet) = book.sheet_mut(index) else {
        return String::new();
    };
    if !had_vml {
        sheet.attachments.push(Attachment {
            kind: VML_REL.to_owned(),
            target: path,
        });
    }
    // Relationship ids follow the external hyperlinks, in attachment order.
    let first_rel = super::xlsx::external_links(sheet).len() + 1 + sheet.attachments.len();
    for target in targets {
        sheet.attachments.push(Attachment {
            kind: PROPS_REL.to_owned(),
            target,
        });
    }
    controls_element(&controls, &placed, first_rel)
}

/// The sheet's `<controls>`: each control by its shape id and the
/// relationship of its `ctrlProp` part, numbered from `first_rel`.
fn controls_element(
    controls: &[FormControl],
    placed: &[(usize, [u32; 8])],
    first_rel: usize,
) -> String {
    let mut out = format!(
        r#"<mc:AlternateContent xmlns:mc="{MC_NS}"><mc:Choice xmlns:x14="{X14_NS}" Requires="x14"><controls xmlns:xdr="{DRAWING_NS}">"#
    );
    for (i, (control, &(id, corners))) in controls.iter().zip(placed).enumerate() {
        let (_, label) = control.kind.names();
        // Excel numbers new controls by kind: "Check Box 1", "Check Box 2".
        let count = controls[..=i]
            .iter()
            .filter(|c| c.kind == control.kind)
            .count();
        let macro_attr = control
            .macro_name
            .as_deref()
            .map_or(String::new(), |m| format!(r#" macro="{}""#, escape(m)));
        let marker = |tag: &str, [col, dx, row, dy]: [u32; 4]| {
            format!(
                "<{tag}><xdr:col>{col}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>{}</xdr:rowOff></{tag}>",
                i64::from(dx) * EMU_PER_PIXEL,
                i64::from(dy) * EMU_PER_PIXEL
            )
        };
        let [c1, dx1, r1, dy1, c2, dx2, r2, dy2] = corners;
        let _ = write!(
            out,
            concat!(
                r#"<mc:AlternateContent xmlns:mc="{mc}"><mc:Choice xmlns:x14="{x14}" Requires="x14">"#,
                r#"<control shapeId="{id}" r:id="rId{rel}" name="{name} {count}">"#,
                r#"<controlPr defaultSize="0" autoFill="0" autoLine="0" autoPict="0"{macro_attr}>"#,
                r#"<anchor moveWithCells="1">{from}{to}</anchor></controlPr></control>"#,
                "</mc:Choice></mc:AlternateContent>"
            ),
            mc = MC_NS,
            x14 = X14_NS,
            id = id,
            rel = first_rel + i,
            name = label,
            count = count,
            macro_attr = macro_attr,
            from = marker("from", [c1, dx1, r1, dy1]),
            to = marker("to", [c2, dx2, r2, dy2]),
        );
    }
    out.push_str("</controls></mc:Choice></mc:AlternateContent>");
    out
}

/// The anchor as VML states it: column, pixels into it, row, pixels into it,
/// for each corner.
fn pixel_anchor(sheet: &Worksheet, anchor: Anchor) -> [u32; 8] {
    let pixels = |emu: i64| u32::try_from(emu.max(0) / EMU_PER_PIXEL).unwrap_or(u32::MAX);
    let across =
        |col: u32, dx: u32, by: u32| walk(col, dx, by, MAX_COL, |c| column_pixels(sheet, c));
    let down = |row: u32, dy: u32, by: u32| walk(row, dy, by, MAX_ROW, |r| row_pixels(sheet, r));
    let (from, (width, height)) = match anchor {
        Anchor::TwoCell { from, to, .. } => {
            return [
                from.col.index(),
                pixels(from.col_offset),
                from.row.index(),
                pixels(from.row_offset),
                to.col.index(),
                pixels(to.col_offset),
                to.row.index(),
                pixels(to.row_offset),
            ];
        }
        Anchor::OneCell {
            from,
            width,
            height,
        } => (
            [
                from.col.index(),
                pixels(from.col_offset),
                from.row.index(),
                pixels(from.row_offset),
            ],
            (pixels(width), pixels(height)),
        ),
        Anchor::Absolute {
            x,
            y,
            width,
            height,
        } => {
            let (c, dx) = across(0, 0, pixels(x));
            let (r, dy) = down(0, 0, pixels(y));
            ([c, dx, r, dy], (pixels(width), pixels(height)))
        }
    };
    let [c1, dx1, r1, dy1] = from;
    let (c2, dx2) = across(c1, dx1, width);
    let (r2, dy2) = down(r1, dy1, height);
    [c1, dx1, r1, dy1, c2, dx2, r2, dy2]
}

/// One control as a VML shape.
fn vml_shape(id: usize, control: &FormControl, corners: [u32; 8]) -> String {
    let kind = control.kind;
    let look = match kind {
        ControlKind::Button => {
            r#" o:button="t" fillcolor="buttonFace [67]" strokecolor="windowText [64]" o:insetmode="auto"><v:fill color2="buttonFace [67]" o:detectmouseclick="t"/>"#
        }
        _ => {
            r#" filled="f" fillcolor="window [65]" stroked="f" strokecolor="windowText [64]" o:insetmode="auto"><v:path shadowok="t" strokeok="t" fillok="t"/>"#
        }
    };
    let mut s = format!(
        r##"<v:shape id="_x0000_s{id}" type="#_x0000_t201" style="position:absolute;z-index:{z};mso-wrap-style:tight"{look}<o:lock v:ext="edit" rotation="t"/>"##,
        z = id % 1024,
    );
    if let Some(text) = &control.text {
        let align = if kind == ControlKind::Button {
            "center"
        } else {
            "left"
        };
        let _ = write!(
            s,
            r#"<v:textbox style="mso-direction-alt:auto" o:singleclick="f"><div style="text-align:{align}">{}</div></v:textbox>"#,
            font_markup(control.font.as_ref(), text)
        );
    }
    let anchor: Vec<String> = corners.iter().map(u32::to_string).collect();
    let _ = write!(
        s,
        concat!(
            r#"<x:ClientData ObjectType="{kind}"><x:Anchor>{anchor}</x:Anchor>"#,
            "<x:PrintObject>False</x:PrintObject><x:AutoFill>False</x:AutoFill>"
        ),
        kind = kind.vml_name(),
        anchor = anchor.join(", "),
    );
    if kind == ControlKind::Button {
        s.push_str("<x:TextHAlign>Center</x:TextHAlign>");
    }
    if matches!(
        kind,
        ControlKind::Button | ControlKind::CheckBox | ControlKind::OptionButton
    ) {
        s.push_str("<x:TextVAlign>Center</x:TextVAlign>");
    }
    let mut element = |tag: &str, value: &str| {
        let _ = write!(s, "<x:{tag}>{}</x:{tag}>", escape(value));
    };
    if let Some(state) = control.checked.filter(|&c| c != CheckState::Unchecked) {
        element(
            "Checked",
            if state == CheckState::Checked {
                "1"
            } else {
                "2"
            },
        );
    }
    if let Some(n) = control.scroll {
        for (tag, value) in [
            ("Val", n.value),
            ("Min", n.min),
            ("Max", n.max),
            ("Inc", n.step),
            ("Page", n.page),
        ] {
            element(tag, &value.to_string());
        }
    }
    if let Some(link) = &control.linked_cell {
        element("FmlaLink", link);
    }
    if let Some(range) = &control.input_range {
        element("FmlaRange", range);
    }
    if let Some(name) = &control.macro_name {
        element("FmlaMacro", name);
    }
    if kind == ControlKind::ComboBox {
        element("DropLines", "8");
    }
    if matches!(kind, ControlKind::CheckBox | ControlKind::OptionButton) {
        s.push_str("<x:NoThreeD/>");
    }
    s.push_str("</x:ClientData></v:shape>");
    s
}

/// The caption in a `<font>` saying what the model's font says.
fn font_markup(font: Option<&Font>, text: &str) -> String {
    let text = escape(text).replace('\n', "<br/>");
    let Some(font) = font else {
        return text;
    };
    let mut inner = text;
    if font.italic {
        inner = format!("<i>{inner}</i>");
    }
    if font.bold {
        inner = format!("<b>{inner}</b>");
    }
    let color = match font.color {
        Color::Argb(argb) => format!(r##" color="#{:06X}""##, argb & 0x00FF_FFFF),
        _ => String::new(),
    };
    format!(
        r#"<font face="{}" size="{}"{color}>{inner}</font>"#,
        escape(&font.name),
        font.size / 5
    )
}

/// A `ctrlProp` part: the properties Excel 2010 reads in place of the VML.
fn ctrl_prop(control: &FormControl) -> String {
    let (kind, _) = control.kind.names();
    let mut s = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<formControlPr xmlns="{X14_NS}" objectType="{kind}""#,
        "\n"
    );
    let mut attr = |name: &str, value: &str| {
        let _ = write!(s, r#" {name}="{}""#, escape(value));
    };
    match control.checked {
        Some(CheckState::Checked) => attr("checked", "Checked"),
        Some(CheckState::Mixed) => attr("checked", "Mixed"),
        _ => {}
    }
    if control.kind == ControlKind::ComboBox {
        attr("dropLines", "8");
        attr("dropStyle", "combo");
    }
    if let Some(link) = &control.linked_cell {
        attr("fmlaLink", link);
    }
    if let Some(range) = &control.input_range {
        attr("fmlaRange", range);
    }
    if let Some(n) = control.scroll {
        for (name, value) in [
            ("inc", n.step),
            ("max", n.max),
            ("min", n.min),
            ("page", n.page),
            ("val", n.value),
        ] {
            attr(name, &value.to_string());
        }
    }
    if control.text.is_some() {
        attr("lockText", "1");
    }
    if matches!(
        control.kind,
        ControlKind::CheckBox | ControlKind::OptionButton
    ) {
        attr("noThreeD", "1");
    }
    s.push_str("/>");
    s
}
