//! Reading HTML.
//!
//! A page becomes one sheet: a `<table>` is a block of cells, `<tr>` a row,
//! `<td>` a cell, `colspan`/`rowspan` a merge. Outside a table the text still
//! lands on the sheet - a `<p>` or an `<h1>` fills one cell and moves the
//! cursor down a row - which is what a reader has to do and what makes a plain
//! article readable as a column of text.
//!
//! A full HTML parser would do, but that is a dependency we do
//! not have. The scanner below is the small part of one that a spreadsheet
//! needs: tags, attributes, entities, and the implied end tags (`<tr>` closes
//! an open `<td>`) without which real-world tables collapse into one cell.
//! Everything a browser needs and a sheet does not - the element categories,
//! foster parenting, character encoding declared halfway through the file - is
//! left out.
//!
//! Not ported: images (the model has no drawings yet), comments, document
//! properties from `<meta>`, and `data-printarea`, which is a defined name on
//! the workbook rather than anything the sheet holds.

use crate::error::{Error, Result};
use crate::model::{CellValue, ColumnRun, Hyperlink, LinkTarget, Spreadsheet, Worksheet};
use crate::style::{
    Border, BorderStyle, Color, HorizontalAlign, NumberFormat, Pattern, Style, StyleTable,
    Underline, VerticalAlign,
};
use crate::{CellError, CellRef, Col, Range, Row};

/// Reads a page from a file.
///
/// # Errors
/// [`Error::Html`] if the file cannot be read. Malformed markup is not an
/// error: a browser shows what it can, and so does this.
pub fn read_html(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let bytes = std::fs::read(path).map_err(|e| Error::Html(e.to_string()))?;
    Ok(read_html_str(&super::csv::decode(&bytes)))
}

/// Reads a page already in memory.
#[must_use]
pub fn read_html_str(html: &str) -> Spreadsheet {
    let mut build = Build::new();
    build.children(&parse(html));
    build.finish()
}

// ---------------------------------------------------------------- the scanner

/// One node of the tree the scanner builds.
#[derive(Debug)]
enum Node {
    /// Character data, entities already resolved.
    Text(String),
    /// An element with its attributes and content.
    Elem(Elem),
}

/// An element. Names and attribute names are lower-cased.
#[derive(Debug, Default)]
struct Elem {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Elem {
    /// One attribute by name.
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// An attribute read as a number, ignoring any unit that follows it.
    fn number(&self, name: &str) -> Option<f64> {
        let value = self.attr(name)?;
        let digits: String = value
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        digits.parse().ok()
    }

    /// A span attribute (`colspan`, `rowspan`) as a count of at least one.
    fn span(&self, name: &str) -> Option<u32> {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        self.number(name).map(|n| (n.max(1.0) as u32).max(1))
    }
}

/// How deep the tree may nest. The walk over it recurses, and a page is free
/// to open ten thousand `<div>`s; past the cap an element keeps its attributes
/// but its content is read as the parent's.
const MAX_DEPTH: usize = 256;

/// Elements that never have content, so they are never left open.
const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// What an opening tag closes for us when the page left it open. This is the
/// slice of the HTML5 tree builder a table needs: without it `<tr><td>a<td>b`
/// nests the second cell inside the first.
fn implied_closes(name: &str) -> &'static [&'static str] {
    match name {
        "tr" => &["td", "th", "tr"],
        "td" | "th" => &["td", "th"],
        "li" => &["li"],
        "p" => &["p"],
        "tbody" | "thead" | "tfoot" => &["td", "th", "tr", "tbody", "thead", "tfoot"],
        _ => &[],
    }
}

/// Builds a tree from markup.
fn parse(html: &str) -> Vec<Node> {
    let mut root = Vec::new();
    let mut stack: Vec<Elem> = Vec::new();
    let mut rest = html;

    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut root, &mut stack, rest);
            break;
        };
        if lt > 0 {
            push_text(&mut root, &mut stack, &rest[..lt]);
        }
        let tail = &rest[lt..];

        // Comments, doctypes and processing instructions carry nothing we want.
        if let Some(body) = tail.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |end| &body[end + 3..]);
            continue;
        }
        if tail.starts_with("<!") || tail.starts_with("<?") {
            rest = tail.find('>').map_or("", |end| &tail[end + 1..]);
            continue;
        }
        if let Some(body) = tail.strip_prefix("</") {
            let end = body.find('>').unwrap_or(body.len());
            let name = body[..end].trim().to_ascii_lowercase();
            close(&mut root, &mut stack, &name);
            rest = body.get(end + 1..).unwrap_or("");
            continue;
        }

        let after = &tail[1..];
        if !after.starts_with(|c: char| c.is_ascii_alphabetic()) {
            // A stray `<` is text, the way a browser reads it.
            push_text(&mut root, &mut stack, "<");
            rest = after;
            continue;
        }
        let (elem, self_closing, tail) = open_tag(after);

        // Script and style hold text that is not markup; skipping to the end
        // tag keeps a stylesheet out of the cells.
        if elem.name == "script" || elem.name == "style" {
            let end = format!("</{}", elem.name);
            rest = match tail.to_ascii_lowercase().find(&end) {
                Some(at) => tail[at..].find('>').map_or("", |gt| &tail[at + gt + 1..]),
                None => "",
            };
            continue;
        }

        for closing in implied_closes(&elem.name) {
            if stack.iter().any(|e| &e.name == closing) {
                close(&mut root, &mut stack, closing);
            }
        }
        if self_closing || VOID.contains(&elem.name.as_str()) || stack.len() >= MAX_DEPTH {
            push_node(&mut root, &mut stack, Node::Elem(elem));
        } else {
            stack.push(elem);
        }
        rest = tail;
    }

    // Whatever the page left open is closed at the end of it.
    while let Some(elem) = stack.pop() {
        push_node(&mut root, &mut stack, Node::Elem(elem));
    }
    root
}

/// Reads a tag after its `<`, returning the element, whether it closed itself,
/// and what follows it.
fn open_tag(after: &str) -> (Elem, bool, &str) {
    let end = after
        .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        .unwrap_or(after.len());
    let mut elem = Elem {
        name: after[..end].to_ascii_lowercase(),
        ..Elem::default()
    };
    let mut rest = &after[end..];
    let mut self_closing = false;

    loop {
        rest = rest.trim_start_matches(char::is_whitespace);
        match rest.as_bytes().first() {
            None => break,
            Some(b'>') => {
                rest = &rest[1..];
                break;
            }
            Some(b'/') => {
                self_closing = true;
                rest = &rest[1..];
                continue;
            }
            Some(_) => {}
        }
        let end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '>')
            .unwrap_or(rest.len());
        let name = rest[..end].to_ascii_lowercase();
        rest = rest[end..].trim_start_matches(char::is_whitespace);
        let mut value = String::new();
        if let Some(tail) = rest.strip_prefix('=') {
            let tail = tail.trim_start_matches(char::is_whitespace);
            let (raw, tail) = if let Some(&quote @ (b'"' | b'\'')) = tail.as_bytes().first() {
                let body = &tail[1..];
                let end = body.find(char::from(quote)).unwrap_or(body.len());
                (&body[..end], body.get(end + 1..).unwrap_or(""))
            } else {
                let end = tail
                    .find(|c: char| c.is_ascii_whitespace() || c == '>')
                    .unwrap_or(tail.len());
                (&tail[..end], &tail[end..])
            };
            value = unescape(raw);
            rest = tail;
        }
        if !name.is_empty() {
            elem.attrs.push((name, value));
        }
    }
    (elem, self_closing, rest)
}

/// Closes the innermost element of a name, and everything the page left open
/// inside it.
fn close(root: &mut Vec<Node>, stack: &mut Vec<Elem>, name: &str) {
    if !stack.iter().any(|e| e.name == name) {
        // An end tag with no start tag is noise; a browser drops it too.
        return;
    }
    while let Some(elem) = stack.pop() {
        let done = elem.name == name;
        push_node(root, stack, Node::Elem(elem));
        if done {
            return;
        }
    }
}

/// Adds a node to whatever is open, or to the document.
fn push_node(root: &mut Vec<Node>, stack: &mut [Elem], node: Node) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => root.push(node),
    }
}

/// Adds character data, resolving its entities.
fn push_text(root: &mut Vec<Node>, stack: &mut [Elem], text: &str) {
    push_node(root, stack, Node::Text(unescape(text)));
}

/// Resolves character references. An unknown name is left as it was written:
/// the page means those characters, and inventing a replacement is worse than
/// showing the source.
fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let body = &rest[amp + 1..];
        let end = body.find(';').filter(|&end| end <= 8);
        if let Some((c, end)) = end.and_then(|end| entity(&body[..end]).map(|c| (c, end))) {
            out.push(c);
            rest = &body[end + 1..];
        } else {
            out.push('&');
            rest = body;
        }
    }
    out.push_str(rest);
    out
}

/// One character reference by name or number.
fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" | "#39" => return Some('\''),
        "nbsp" => return Some('\u{a0}'),
        _ => {}
    }
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code)
}

// ------------------------------------------------------------------ the walk

/// The cursor the walk carries: the state threaded through the
/// `processDomElement*` chain, which is one method per element name written as
/// a fall-through.
struct Build {
    sheet: Worksheet,
    styles: StyleTable,
    /// One-based, like the sheet. Starting at row 0 and letting `<body>`
    /// set it to 1; `DOMDocument` always supplies a `<body>`, our scanner does
    /// not, so the cursor starts on the first row instead.
    row: u32,
    col: u32,
    /// Text collected since the last flush, the content of the cell to come.
    content: String,
    table_level: u32,
    /// The column each open table starts at, `nestedColumn` in the HTML reader's own terms.
    nested_column: Vec<u32>,
    /// What a `rowspan` already covers, carried from the row above. Kept as
    /// ranges rather than as a set of addresses: a page is free to claim a span
    /// of a million rows, and the set would then hold a million entries.
    spanned: Vec<Range>,
    /// Which column the next `<col>` describes.
    current_column: u32,
}

/// How many cells one `style=` attribute may be spread over. A `rowspan` is a
/// number out of the file, and a merge of a million rows would otherwise mean a
/// million cells created to hold one background colour.
// ponytail: fixed cap; a per-cell style on a huge merge is not worth the memory.
const MAX_STYLED_SPAN: u64 = 4096;

impl Build {
    fn new() -> Self {
        Self {
            sheet: Worksheet::new("Worksheet").unwrap_or_default(),
            styles: StyleTable::default(),
            row: 1,
            col: 1,
            content: String::new(),
            table_level: 0,
            nested_column: vec![1],
            spanned: Vec::new(),
            current_column: 1,
        }
    }

    /// The sheet as a workbook of its own.
    fn finish(self) -> Spreadsheet {
        let mut book = Spreadsheet::empty();
        book.styles = self.styles;
        let _ = book.add_sheet(self.sheet);
        book
    }

    /// The cell the cursor is on, `None` once it has run past the sheet.
    fn at(&self) -> Option<CellRef> {
        Some(CellRef::new(
            Col::from_one_based(u64::from(self.col)).ok()?,
            Row::from_one_based(u64::from(self.row)).ok()?,
        ))
    }

    fn children(&mut self, nodes: &[Node]) {
        for node in nodes {
            match node {
                Node::Text(text) => {
                    let text = collapse(text);
                    // A cell holding one non-breaking space is an empty cell:
                    // that is how a page spells "nothing here".
                    if text != "\u{a0}" {
                        self.content.push_str(&text);
                    }
                }
                Node::Elem(elem) => self.element(elem),
            }
        }
    }

    /// One element.
    fn element(&mut self, e: &Elem) {
        match e.name.as_str() {
            "body" => {
                self.row = 1;
                self.col = 1;
                self.content.clear();
                self.table_level = 0;
                self.children(&e.children);
            }
            "title" => {
                self.children(&e.children);
                let title = std::mem::take(&mut self.content);
                // A title too long, or holding a character Excel bans, leaves
                // the sheet named as it was.
                let _ = self.sheet.set_title(title);
            }
            "span" | "div" | "font" | "i" | "em" | "strong" | "b" => {
                self.children(&e.children);
                self.tag_format(&e.name);
            }
            "hr" => {
                self.flush_cell(Some(e));
                self.row += 1;
                self.tag_format("hr");
                self.row += 1;
                self.line_break(e);
            }
            "br" => self.line_break(e),
            "a" => {
                if let Some(href) = e.attr("href") {
                    self.link(href);
                }
                self.children(&e.children);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ol" | "ul" | "p" => self.block(e),
            "li" => self.list_item(e),
            "table" => self.table(e),
            "col" => {
                self.style_attribute(e, None, self.current_column, None);
                self.current_column += 1;
            }
            "tr" => self.table_row(e),
            "td" | "th" => self.table_cell(e),
            // An image wants a drawing, which the model does not have yet.
            "img" => {}
            _ => self.children(&e.children),
        }
    }

    fn line_break(&mut self, e: &Elem) {
        if self.table_level > 0 {
            self.content.push('\n');
            self.restyle_current(|style| style.alignment.wrap_text = true);
        } else {
            self.flush_cell(Some(e));
            self.row += 1;
        }
    }

    fn block(&mut self, e: &Elem) {
        if self.table_level > 0 {
            if !self.content.is_empty() {
                self.content.push('\n');
            }
            self.restyle_current(|style| style.alignment.wrap_text = true);
            self.children(&e.children);
            return;
        }
        if !self.content.is_empty() {
            self.flush_cell(Some(e));
            self.row += 1;
        }
        self.children(&e.children);
        self.flush_cell(Some(e));
        self.tag_format(&e.name);
        self.row += 1;
        self.col = 1;
    }

    fn list_item(&mut self, e: &Elem) {
        if self.table_level > 0 {
            if !self.content.is_empty() {
                self.content.push('\n');
            }
            self.children(&e.children);
            return;
        }
        if !self.content.is_empty() {
            self.flush_cell(Some(e));
        }
        self.row += 1;
        self.children(&e.children);
        self.flush_cell(Some(e));
        self.col = 1;
    }

    fn table(&mut self, e: &Elem) {
        if let Some(class) = e.attr("class") {
            let classes: Vec<&str> = class.split_whitespace().collect();
            self.sheet.view.show_grid_lines = classes.contains(&"gridlines");
            self.sheet.print_options.grid_lines = classes.contains(&"gridlinesp");
        }
        if e.attr("dir") == Some("rtl") {
            self.sheet.view.right_to_left = true;
        }
        self.current_column = 1;
        self.flush_cell(Some(e));

        // setTableStartColumn: a table at the top level always starts at A.
        if self.table_level == 0 {
            self.col = 1;
        }
        self.table_level += 1;
        self.nested_column.push(self.col);
        if self.table_level > 1 && self.row > 1 {
            self.row -= 1;
        }

        self.children(&e.children);

        self.table_level = self.table_level.saturating_sub(1);
        self.col = self.nested_column.pop().unwrap_or(1);
        // The other branch increments a copy of the column and throws it
        // away, so closing a table only ever moves the cursor down a row.
        if self.table_level <= 1 {
            self.row += 1;
        }
    }

    fn table_row(&mut self, e: &Elem) {
        self.col = self
            .nested_column
            .get(self.table_level as usize)
            .copied()
            .unwrap_or(1);
        self.content.clear();
        self.children(&e.children);
        if let Some(height) = e.number("height") {
            self.set_row_height(self.row, height);
        }
        self.row += 1;
    }

    fn table_cell(&mut self, e: &Elem) {
        while let Some(at) = self.at() {
            if !self.spanned.iter().any(|range| range.contains(at)) {
                break;
            }
            self.col += 1;
        }
        self.children(&e.children);

        let merge = self.merge_range(e);
        self.style_attribute(e, merge, self.col, Some(self.row));
        self.flush_cell(Some(e));

        if let Some(colour) = e.attr("bgcolor").and_then(colour) {
            self.restyle_current(|style| {
                style.fill.pattern = Pattern::Solid;
                style.fill.foreground = colour;
            });
        }
        if let Some(width) = e.attr("width").and_then(css_width) {
            self.set_column_width(self.col, width);
        }
        if let Some(height) = e.attr("height").and_then(css_height) {
            self.set_row_height(self.row, height);
        }
        if let Some(align) = e.attr("align").map(HorizontalAlign::parse) {
            self.restyle_current(|style| style.alignment.horizontal = align);
        }
        if let Some(align) = e.attr("valign").map(vertical) {
            self.restyle_current(|style| style.alignment.vertical = align);
        }
        if let Some(code) = e.attr("data-format").map(ToOwned::to_owned) {
            self.restyle_current(|style| style.number_format = NumberFormat::Custom(code));
        }

        if let Some(range) = merge {
            if e.attr("rowspan").is_some() {
                self.spanned.push(range);
            }
            self.sheet.merges.push(range);
            self.col = range.end.col.one_based();
        }
        self.col += 1;
    }

    /// The range a cell's spans cover, `None` when it spans nothing and `None`
    /// again when the spans run off the sheet.
    fn merge_range(&self, e: &Elem) -> Option<Range> {
        let cols = e.span("colspan").unwrap_or(1);
        let rows = e.span("rowspan").unwrap_or(1);
        if cols == 1 && rows == 1 {
            return None;
        }
        let start = self.at()?;
        let end = CellRef::new(
            Col::from_one_based(u64::from(self.col) + u64::from(cols) - 1).ok()?,
            Row::from_one_based(u64::from(self.row) + u64::from(rows) - 1).ok()?,
        );
        Some(Range::new(start, end))
    }

    fn flush_cell(&mut self, e: Option<&Elem>) {
        let content = std::mem::take(&mut self.content);
        if content.trim().is_empty() {
            return;
        }
        let Some(at) = self.at() else { return };
        self.sheet.entry(at).value = value_of(&content, e);
    }

    /// The hyperlink of an `<a href>`, plus the blue underline browsers give
    /// it from its `FORMATS` table.
    fn link(&mut self, href: &str) {
        let Some(at) = self.at() else { return };
        self.sheet.hyperlinks.push(Hyperlink {
            range: Range::new(at, at),
            target: match href.strip_prefix('#') {
                Some(place) => LinkTarget::Inside(place.to_owned()),
                None => LinkTarget::Outside(href.to_owned()),
            },
            display: None,
            tooltip: None,
        });
        self.tag_format("a");
    }

    fn tag_format(&mut self, name: &str) {
        let points = match name {
            "h1" => Some(24.0),
            "h2" => Some(18.0),
            "h3" => Some(13.5),
            "h4" => Some(12.0),
            "h5" => Some(10.0),
            "h6" => Some(7.5),
            _ => None,
        };
        self.restyle_current(|style| match name {
            "b" | "strong" => style.font.bold = true,
            "i" | "em" => style.font.italic = true,
            "a" => {
                style.font.underline = Underline::Single;
                style.font.color = Color::Argb(0xFF00_00FF);
            }
            "hr" => {
                style.borders.bottom = Border {
                    style: BorderStyle::parse("thin"),
                    color: Color::Argb(0xFF00_0000),
                };
            }
            _ => {
                if let Some(points) = points {
                    style.font.bold = true;
                    style.font.set_size_points(points);
                }
            }
        });
    }

    /// `target` is the range a merged cell covers; `row` is `None` for a
    /// `<col>`, which carries a width and nothing a cell could hold.
    fn style_attribute(&mut self, e: &Elem, target: Option<Range>, col: u32, row: Option<u32>) {
        let Some(css) = e.attr("style").map(ToOwned::to_owned) else {
            return;
        };
        let range = match (target, row) {
            (Some(range), _) => Some(range),
            (None, Some(_)) => self.at().map(|at| Range::new(at, at)),
            (None, None) => None,
        };
        let mut style = range
            .and_then(|range| self.sheet.get(range.start))
            .map(|cell| cell.style)
            .and_then(|id| self.styles.get(id).cloned())
            .unwrap_or_default();

        for declaration in css.split(';') {
            let Some((name, value)) = declaration.split_once(':') else {
                continue;
            };
            let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
            match name.as_str() {
                "background" | "background-color" => {
                    if let Some(colour) = colour(value) {
                        style.fill.pattern = Pattern::Solid;
                        style.fill.foreground = colour;
                    }
                }
                "color" => {
                    if let Some(colour) = colour(value) {
                        style.font.color = colour;
                    }
                }
                "border" => set_border(&mut style, value, &["top", "right", "bottom", "left"]),
                "border-top" => set_border(&mut style, value, &["top"]),
                "border-right" => set_border(&mut style, value, &["right"]),
                "border-bottom" => set_border(&mut style, value, &["bottom"]),
                "border-left" => set_border(&mut style, value, &["left"]),
                "font-size" => {
                    if let Some((size, _)) = length(value) {
                        style.font.set_size_points(size);
                    }
                }
                "font-weight" => {
                    style.font.bold |=
                        value == "bold" || length(value).is_some_and(|(weight, _)| weight >= 500.0);
                }
                "font-style" => style.font.italic |= value == "italic",
                "font-family" => style.font.name = value.replace(['\'', '"'], ""),
                "text-decoration" => match value {
                    "underline" => style.font.underline = Underline::Single,
                    "line-through" => style.font.strike = true,
                    _ => {}
                },
                "text-align" => style.alignment.horizontal = HorizontalAlign::parse(value),
                "vertical-align" => style.alignment.vertical = vertical(value),
                "word-wrap" => style.alignment.wrap_text = value == "break-word",
                "text-indent" => {
                    if let Some(indent) = css_pixels(value) {
                        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        {
                            style.alignment.indent = (indent.max(0.0) / 9.0) as u32;
                        }
                    }
                }
                "width" => {
                    if let Some(width) = css_width(value) {
                        self.set_column_width(col, width);
                    }
                }
                "height" => {
                    if let (Some(row), Some(height)) = (row, css_height(value)) {
                        self.set_row_height(row, height);
                    }
                }
                // `direction` has no counterpart: the model keeps the reading
                // order of a sheet, not of a cell.
                _ => {}
            }
        }

        let Some(range) = range.filter(|_| row.is_some()) else {
            return;
        };
        let id = self.styles.intern(style);
        let cells = u64::from(range.width()) * u64::from(range.height());
        let range = if cells > MAX_STYLED_SPAN {
            Range::new(range.start, range.start)
        } else {
            range
        };
        for at in range.cells() {
            self.sheet.entry(at).style = id;
        }
    }

    /// Rewrites the style of the cell under the cursor.
    fn restyle_current(&mut self, apply: impl FnOnce(&mut Style)) {
        let Some(at) = self.at() else { return };
        let id = self.sheet.entry(at).style;
        let mut style = self.styles.get(id).cloned().unwrap_or_default();
        apply(&mut style);
        let id = self.styles.intern(style);
        self.sheet.entry(at).style = id;
    }

    /// Sets one column's width, in the character units xlsx counts in.
    fn set_column_width(&mut self, col: u32, width: f64) {
        let Ok(col) = Col::from_one_based(u64::from(col)) else {
            return;
        };
        if !self
            .sheet
            .columns
            .iter()
            .any(|run| run.first == col && run.last == col)
        {
            self.sheet.columns.push(ColumnRun::new(col, col));
        }
        if let Some(run) = self
            .sheet
            .columns
            .iter_mut()
            .find(|run| run.first == col && run.last == col)
        {
            run.width = Some(width);
            run.custom_width = true;
        }
    }

    /// Sets one row's height, in points.
    fn set_row_height(&mut self, row: u32, height: f64) {
        let Ok(row) = Row::from_one_based(u64::from(row)) else {
            return;
        };
        let properties = self.sheet.rows.entry(row).or_default();
        properties.height = Some(height);
        properties.custom_height = true;
    }
}

/// Collapses runs of whitespace the way a browser lays text out, so the markup's
/// own indentation does not reach the cell.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.trim_matches(|c: char| c.is_ascii_whitespace()).chars() {
        if c.is_ascii_whitespace() {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// What a cell's text means.
///
/// The `data-` attributes are how `Writer/Html` records the type it knew, so a
/// page it wrote reads back as numbers and formulas rather than as text.
/// Without them the text is read the way a CSV field is, which is the same
/// value binder a reader falls back on.
fn value_of(content: &str, e: Option<&Elem>) -> CellValue {
    let attr = |name: &str| e.and_then(|e| e.attr(name));
    if let Some(formula) = attr("data-formula") {
        return CellValue::Formula {
            formula: formula.trim_start_matches('=').to_owned(),
            cached: Some(Box::new(super::csv::value_of(content))),
        };
    }
    let text = attr("data-value").unwrap_or(content);
    match attr("data-type") {
        Some("s" | "str" | "inlineStr") => CellValue::text(text),
        Some("n") => text
            .trim()
            .parse()
            .map_or_else(|_| CellValue::text(text), CellValue::Number),
        Some("b") => CellValue::Bool(!matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false"
        )),
        Some("e") => {
            CellError::parse(text.trim()).map_or_else(|| CellValue::text(text), CellValue::Error)
        }
        _ => super::csv::value_of(text),
    }
}

/// Vertical placement. HTML's word for the middle is `middle`, Excel's is
/// `center`; the rest of the values the two spell the same.
fn vertical(value: &str) -> VerticalAlign {
    match value {
        "middle" => VerticalAlign::Center,
        other => VerticalAlign::parse(other),
    }
}

fn set_border(style: &mut Style, value: &str, sides: &[&str]) {
    let words: Vec<&str> = value.split_whitespace().collect();
    let (line, colour) = if value.trim() == "none" {
        ("none", None)
    } else if words.len() >= 3 {
        (words[1], Some(words[2]))
    } else {
        (*words.first().unwrap_or(&"none"), words.get(1).copied())
    };
    let border = Border {
        style: border_style(line),
        color: colour.and_then(self::colour).unwrap_or_default(),
    };
    for side in sides {
        match *side {
            "top" => style.borders.top = border.clone(),
            "right" => style.borders.right = border.clone(),
            "bottom" => style.borders.bottom = border.clone(),
            _ => style.borders.left = border.clone(),
        }
    }
}

fn border_style(name: &str) -> BorderStyle {
    BorderStyle::parse(match name {
        "solid" => "thin",
        "dash-dot" => "dashDot",
        "dash-dot-dot" => "dashDotDot",
        "medium-dashed" => "mediumDashed",
        "medium-dash-dot" => "mediumDashDot",
        "medium-dash-dot-dot" => "mediumDashDotDot",
        "slant-dash-dot" => "slantDashDot",
        other => other,
    })
}

/// A CSS colour: `#rgb`, `#rrggbb` or one of the names.
fn colour(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let hex: String = match hex.len() {
            // `#abc` is `#aabbcc`.
            3 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 => hex.to_owned(),
            _ => return None,
        };
        return Color::from_argb_str(&format!("FF{hex}"));
    }
    let name = value.to_ascii_lowercase();
    COLOUR_NAMES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, rgb)| Color::Argb(0xFF00_0000 | rgb))
}

/// A CSS length as its number and, when it named an absolute unit, that length
/// in pixels at the 96 dpi the format assumes.
fn length(value: &str) -> Option<(f64, Option<f64>)> {
    let value = value.trim();
    let end = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let size: f64 = value[..end].parse().ok()?;
    if !size.is_finite() {
        return None;
    }
    let unit = value[end..].trim().to_ascii_lowercase();
    let per_unit = match unit.as_str() {
        "" => return Some((size, None)),
        "px" => 1.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "in" => 96.0,
        "pt" => 96.0 / 72.0,
        "pc" => 96.0 * 12.0 / 72.0,
        _ => return None,
    };
    Some((size, Some(size * per_unit)))
}

/// A CSS length in pixels, whatever unit it was written in.
fn css_pixels(value: &str) -> Option<f64> {
    let (size, pixels) = length(value)?;
    Some(pixels.unwrap_or(size))
}

/// A CSS length as a column width. A number with no unit is already in the
/// character units xlsx counts in, which is what `Dimension::width` returns.
fn css_width(value: &str) -> Option<f64> {
    let (size, pixels) = length(value)?;
    // The inverse of `Drawing::pixelsToCellDimension` for Calibri 11, the font
    // measures with here.
    Some(pixels.map_or(size, |pixels| pixels * 9.140_625 / 64.0))
}

/// A CSS length as a row height, in points.
fn css_height(value: &str) -> Option<f64> {
    let (size, pixels) = length(value)?;
    Some(pixels.map_or(size, |pixels| pixels * 0.75))
}

/// The CSS colour names. The X11 numbered
/// variants (`antiquewhite3`) are left out: no page writes them.
const COLOUR_NAMES: [(&str, u32); 112] = [
    ("aliceblue", 0xf0_f8ff),
    ("antiquewhite", 0xfa_ebd7),
    ("aqua", 0x00_ffff),
    ("beige", 0xf5_f5dc),
    ("black", 0x00_0000),
    ("blanchedalmond", 0xff_ebcd),
    ("blue", 0x00_00ff),
    ("blueviolet", 0x8a_2be2),
    ("brown", 0xa5_2a2a),
    ("burlywood", 0xde_b887),
    ("cadetblue", 0x5f_9ea0),
    ("chocolate", 0xd2_691e),
    ("coral", 0xff_7f50),
    ("cornflowerblue", 0x64_95ed),
    ("darkgoldenrod", 0xb8_860b),
    ("darkgreen", 0x00_6400),
    ("darkkhaki", 0xbd_b76b),
    ("darkolivegreen", 0x55_6b2f),
    ("darkorange", 0xff_8c00),
    ("darkorchid", 0x99_32cc),
    ("darksalmon", 0xe9_967a),
    ("darkseagreen", 0x8f_bc8f),
    ("darkslateblue", 0x48_3d8b),
    ("darkslategray", 0x2f_4f4f),
    ("darkturquoise", 0x00_ced1),
    ("darkviolet", 0x94_00d3),
    ("dimgray", 0x69_6969),
    ("firebrick", 0xb2_2222),
    ("floralwhite", 0xff_faf0),
    ("forestgreen", 0x22_8b22),
    ("fuchsia", 0xff_00ff),
    ("gainsboro", 0xdc_dcdc),
    ("ghostwhite", 0xf8_f8ff),
    ("goldenrod", 0xda_a520),
    ("gray", 0xbe_bebe),
    ("green", 0x00_ff00),
    ("greenyellow", 0xad_ff2f),
    ("hotpink", 0xff_69b4),
    ("indianred", 0xcd_5c5c),
    ("khaki", 0xf0_e68c),
    ("lavender", 0xe6_e6fa),
    ("lawngreen", 0x7c_fc00),
    ("light", 0xee_dd82),
    ("lightblue", 0xad_d8e6),
    ("lightcoral", 0xf0_8080),
    ("lightgoldenrodyellow", 0xfa_fad2),
    ("lightgray", 0xd3_d3d3),
    ("lightpink", 0xff_b6c1),
    ("lightseagreen", 0x20_b2aa),
    ("lightskyblue", 0x87_cefa),
    ("lightslateblue", 0x84_70ff),
    ("lightslategray", 0x77_8899),
    ("lightsteelblue", 0xb0_c4de),
    ("lime", 0x00_ff00),
    ("limegreen", 0x32_cd32),
    ("linen", 0xfa_f0e6),
    ("magenta", 0xff_00ff),
    ("maroon", 0xb0_3060),
    ("medium", 0x66_cdaa),
    ("mediumaquamarine", 0x66_cdaa),
    ("mediumblue", 0x00_00cd),
    ("mediumorchid", 0xba_55d3),
    ("mediumpurple", 0x93_70db),
    ("mediumseagreen", 0x3c_b371),
    ("mediumslateblue", 0x7b_68ee),
    ("mediumspringgreen", 0x00_fa9a),
    ("mediumturquoise", 0x48_d1cc),
    ("mediumvioletred", 0xc7_1585),
    ("midnightblue", 0x19_1970),
    ("mintcream", 0xf5_fffa),
    ("moccasin", 0xff_e4b5),
    ("navy", 0x00_0080),
    ("navyblue", 0x00_0080),
    ("oldlace", 0xfd_f5e6),
    ("olive", 0x80_8000),
    ("olivedrab", 0x6b_8e23),
    ("orange", 0xff_a500),
    ("orchid", 0xda_70d6),
    ("pale", 0xdb_7093),
    ("palegoldenrod", 0xee_e8aa),
    ("palegreen", 0x98_fb98),
    ("paleturquoise", 0xaf_eeee),
    ("palevioletred", 0xdb_7093),
    ("papayawhip", 0xff_efd5),
    ("pink", 0xff_c0cb),
    ("plum", 0xdd_a0dd),
    ("powderblue", 0xb0_e0e6),
    ("purple", 0xa0_20f0),
    ("rebeccapurple", 0x66_3399),
    ("red", 0xff_0000),
    ("rosybrown", 0xbc_8f8f),
    ("royalblue", 0x41_69e1),
    ("saddlebrown", 0x8b_4513),
    ("salmon", 0xfa_8072),
    ("sandybrown", 0xf4_a460),
    ("sienna", 0xa0_522d),
    ("silver", 0xc0_c0c0),
    ("skyblue", 0x87_ceeb),
    ("slateblue", 0x6a_5acd),
    ("slategray", 0x70_8090),
    ("steelblue", 0x46_82b4),
    ("tan", 0xd2_b48c),
    ("teal", 0x00_8080),
    ("thistle", 0xd8_bfd8),
    ("turquoise", 0x40_e0d0),
    ("violet", 0xee_82ee),
    ("violetred", 0xd0_2090),
    ("wheat", 0xf5_deb3),
    ("white", 0xff_ffff),
    ("whitesmoke", 0xf5_f5f5),
    ("yellow", 0xff_ff00),
    ("yellowgreen", 0x9a_cd32),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CellValue;

    fn cells(html: &str) -> Vec<(String, CellValue)> {
        let book = read_html_str(html);
        let sheet = book.sheet(0).expect("the reader always makes one sheet");
        sheet
            .iter()
            .map(|(at, cell)| (at.to_string(), cell.value.clone()))
            .collect()
    }

    #[test]
    fn a_table_becomes_a_grid() {
        let cells = cells(
            "<html><body><table>\n\
             <tr><td>Name</td><td>Count</td></tr>\n\
             <tr><td>Widget</td><td>12</td></tr>\n\
             </table></body></html>",
        );
        assert_eq!(
            cells,
            vec![
                ("A1".to_owned(), CellValue::text("Name")),
                ("B1".to_owned(), CellValue::text("Count")),
                ("A2".to_owned(), CellValue::text("Widget")),
                ("B2".to_owned(), CellValue::Number(12.0)),
            ]
        );
    }

    #[test]
    fn cells_left_open_still_end_where_the_next_one_starts() {
        // What a hand-written page looks like, and what `DOMDocument` fixes up.
        let cells = cells("<table><tr><td>a<td>b<tr><td>c<td>d</table>");
        assert_eq!(
            cells.iter().map(|(at, _)| at.as_str()).collect::<Vec<_>>(),
            ["A1", "B1", "A2", "B2"]
        );
        assert_eq!(cells[3].1, CellValue::text("d"));
    }

    #[test]
    fn spans_merge_and_push_the_cursor_along() {
        let book = read_html_str(
            "<table>\
             <tr><td rowspan=\"2\">a</td><td colspan=\"2\">b</td></tr>\
             <tr><td>c</td><td>d</td></tr>\
             </table>",
        );
        let sheet = book.sheet(0).expect("one sheet");
        assert_eq!(
            sheet
                .merges
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["A1:A2", "B1:C1"]
        );
        // The second row starts under the rowspan, so `c` lands in B2, not A2.
        let values: Vec<String> = sheet.iter().map(|(at, _)| at.to_string()).collect();
        assert!(values.contains(&"B2".to_owned()), "{values:?}");
        assert!(values.contains(&"C2".to_owned()), "{values:?}");
    }

    #[test]
    fn entities_and_layout_whitespace_are_resolved() {
        let cells = cells("<table><tr><td>  a &amp;\n  b&#66;  </td><td>&nbsp;</td></tr></table>");
        assert_eq!(cells, vec![("A1".to_owned(), CellValue::text("a & bB"))]);
    }

    #[test]
    fn a_line_break_inside_a_cell_stays_in_the_cell() {
        let cells = cells("<table><tr><td>a<br>b</td></tr></table>");
        assert_eq!(cells, vec![("A1".to_owned(), CellValue::text("a\nb"))]);
    }

    #[test]
    fn a_script_is_not_content() {
        let cells = cells("<table><tr><td><script>var a = '<b>';</script>x</td></tr></table>");
        assert_eq!(cells, vec![("A1".to_owned(), CellValue::text("x"))]);
    }

    #[test]
    fn inline_style_reaches_the_cell() {
        let book = read_html_str(
            "<table><tr><td style=\"background-color:#ff0000;font-weight:bold;text-align:center\">\
             x</td></tr></table>",
        );
        let sheet = book.sheet(0).expect("one sheet");
        let cell = sheet
            .get(CellRef::parse("A1").expect("A1 parses"))
            .expect("the cell was written");
        let style = book
            .styles
            .get(cell.style)
            .expect("its style is in the table");
        assert_eq!(style.fill.foreground, Color::Argb(0xffff_0000));
        assert!(style.font.bold);
        assert_eq!(style.alignment.horizontal, HorizontalAlign::Center);
    }

    #[test]
    fn a_link_becomes_a_hyperlink() {
        let book = read_html_str(
            "<table><tr><td><a href=\"https://example.com\">go</a></td></tr></table>",
        );
        let sheet = book.sheet(0).expect("one sheet");
        assert_eq!(sheet.hyperlinks.len(), 1);
        assert_eq!(
            sheet.hyperlinks[0].target,
            LinkTarget::Outside("https://example.com".to_owned())
        );
    }

    #[test]
    fn the_page_title_names_the_sheet() {
        let book = read_html_str("<html><head><title>Sales</title></head><body></body></html>");
        assert_eq!(book.sheet(0).expect("one sheet").title(), "Sales");
    }

    #[test]
    fn text_outside_a_table_fills_a_column() {
        let cells = cells("<body><h1>Title</h1><p>One</p><p>Two</p></body>");
        assert_eq!(
            cells,
            vec![
                ("A1".to_owned(), CellValue::text("Title")),
                ("A2".to_owned(), CellValue::text("One")),
                ("A3".to_owned(), CellValue::text("Two")),
            ]
        );
    }

    #[test]
    fn the_writers_own_types_survive_the_page() {
        let cells = cells(
            "<table><tr>\
             <td data-type=\"n\" data-value=\"1234.5\">1 234,50</td>\
             <td data-type=\"b\">TRUE</td>\
             <td data-formula=\"=A1*2\">2469</td>\
             </tr></table>",
        );
        assert_eq!(cells[0].1, CellValue::Number(1234.5));
        assert_eq!(cells[1].1, CellValue::Bool(true));
        assert_eq!(
            cells[2].1,
            CellValue::Formula {
                formula: "A1*2".to_owned(),
                cached: Some(Box::new(CellValue::Number(2469.0))),
            }
        );
    }

    #[test]
    fn widths_and_heights_come_through_in_the_units_the_sheet_keeps() {
        let book = read_html_str(
            "<table><tr height=\"20\"><td style=\"width:64px;height:15pt\">x</td></tr></table>",
        );
        let sheet = book.sheet(0).expect("one sheet");
        let width = sheet
            .column_width(Col::from_one_based(1).expect("column A"))
            .expect("the width was set");
        assert!((width - 9.140_625).abs() < 1e-6, "{width}");
        let height = sheet
            .row_height(Row::from_one_based(1).expect("row 1"))
            .expect("the height was set");
        // The `height` attribute of the row is applied last and wins.
        assert!((height - 20.0).abs() < 1e-6, "{height}");
    }
}
