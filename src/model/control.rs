//! Form controls: the buttons, check boxes, drop-downs and the rest Excel
//! draws over the cells (Developer > Insert > Form Controls).
//!
//! Read from xls (`OBJ` records) and from xlsx (the VML part behind
//! `<legacyDrawing>`). Not written: an xlsx read keeps its VML and `ctrlProp`
//! parts and they go back as bytes; controls read from xls are dropped on
//! write. `ActiveX` controls are not modelled.

use super::chart::Anchor;

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
        }
    }
}
