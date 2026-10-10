//! Form controls: the buttons, check boxes, drop-downs and the rest Excel
//! draws over the cells (Developer > Insert > Form Controls).
//!
//! Read from xls (`OBJ` records) and from xlsx (the VML part behind
//! `<legacyDrawing>`). Written to xlsx when the sheet's VML holds no controls
//! yet - a workbook read from xls, or controls made in code: a VML shape, a
//! `ctrlProp` part and the sheet's `<controls>`, the way Excel writes them.
//! An xlsx read keeps its VML and `ctrlProp` parts and they go back as bytes.
//! The xls writer writes no drawing at all, so controls are dropped there.
//! `ActiveX` controls are not modelled.

use super::chart::Anchor;
use crate::style::Font;

/// What sort of form control it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// A push button, usually running a macro.
    Button,
    /// A check box.
    CheckBox,
    /// An option (radio) button.
    OptionButton,
    /// A drop-down list.
    ComboBox,
    /// A list box.
    ListBox,
    /// A spin button.
    Spinner,
    /// A scroll bar.
    ScrollBar,
    /// A frame grouping option buttons.
    GroupBox,
    /// A text label.
    Label,
}

impl ControlKind {
    /// The kind VML names in `<x:ClientData ObjectType>`.
    pub(crate) fn from_vml(name: &str) -> Option<Self> {
        Some(match name {
            "Button" => Self::Button,
            "Checkbox" => Self::CheckBox,
            "Radio" => Self::OptionButton,
            "Drop" => Self::ComboBox,
            "List" => Self::ListBox,
            "Spin" => Self::Spinner,
            "Scroll" => Self::ScrollBar,
            "GBox" => Self::GroupBox,
            "Label" => Self::Label,
            _ => return None,
        })
    }

    /// The name VML gives the kind in `<x:ClientData ObjectType>`.
    #[cfg(feature = "write")]
    pub(crate) const fn vml_name(self) -> &'static str {
        match self {
            Self::Button => "Button",
            Self::CheckBox => "Checkbox",
            Self::OptionButton => "Radio",
            Self::ComboBox => "Drop",
            Self::ListBox => "List",
            Self::Spinner => "Spin",
            Self::ScrollBar => "Scroll",
            Self::GroupBox => "GBox",
            Self::Label => "Label",
        }
    }

    /// The name a `ctrlProp` part gives the kind in `objectType`, and the
    /// one Excel numbers new controls with.
    #[cfg(feature = "write")]
    pub(crate) const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Button => ("Button", "Button"),
            Self::CheckBox => ("CheckBox", "Check Box"),
            Self::OptionButton => ("Radio", "Option Button"),
            Self::ComboBox => ("Drop", "Drop Down"),
            Self::ListBox => ("List", "List Box"),
            Self::Spinner => ("Spin", "Spinner"),
            Self::ScrollBar => ("Scroll", "Scroll Bar"),
            Self::GroupBox => ("GBox", "Group Box"),
            Self::Label => ("Label", "Label"),
        }
    }

    /// The kind BIFF names in `ftCmo.ot`.
    pub(crate) fn from_biff(ot: u16) -> Option<Self> {
        Some(match ot {
            0x06 => Self::Label,
            0x07 => Self::Button,
            0x0B => Self::CheckBox,
            0x0C => Self::OptionButton,
            0x10 => Self::Spinner,
            0x11 => Self::ScrollBar,
            0x12 => Self::ListBox,
            0x13 => Self::GroupBox,
            0x14 => Self::ComboBox,
            _ => return None,
        })
    }
}

/// The state of a check box or an option button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// Clear.
    Unchecked,
    /// Ticked.
    Checked,
    /// Neither: the grey square.
    Mixed,
}

impl CheckState {
    /// From the number both formats store: 0, 1, 2.
    pub(crate) fn from_number(value: u32) -> Self {
        match value {
            0 => Self::Unchecked,
            1 => Self::Checked,
            _ => Self::Mixed,
        }
    }
}

/// The numbers of a spinner or a scroll bar (and the list a combo box
/// scrolls).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollValues {
    /// Current value.
    pub value: i32,
    /// Lowest value.
    pub min: i32,
    /// Highest value.
    pub max: i32,
    /// Step of one click on an arrow.
    pub step: i32,
    /// Step of one click on the track.
    pub page: i32,
}

/// One form control on a sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormControl {
    /// What it is.
    pub kind: ControlKind,
    /// Where it sits.
    pub anchor: Anchor,
    /// The caption, for the kinds that have one.
    pub text: Option<String>,
    /// The cell the control writes its state to, as a formula without `=`
    /// (`$C$15`, or a defined name).
    pub linked_cell: Option<String>,
    /// The range a list or combo box takes its items from, the same way.
    pub input_range: Option<String>,
    /// Check box and option button state.
    pub checked: Option<CheckState>,
    /// Spinner and scroll bar numbers.
    pub scroll: Option<ScrollValues>,
    /// The macro the control runs when clicked.
    pub macro_name: Option<String>,
    /// The font of the caption, as the file gives it: the `TXO` run's font
    /// in xls, `<font>` in the VML text box. `None` draws it in the
    /// workbook's default font.
    pub font: Option<Font>,
}

impl FormControl {
    /// A control with nothing but its kind and place.
    #[must_use]
    pub fn new(kind: ControlKind, anchor: Anchor) -> Self {
        Self {
            kind,
            anchor,
            text: None,
            linked_cell: None,
            input_range: None,
            checked: None,
            scroll: None,
            macro_name: None,
            font: None,
        }
    }
}
