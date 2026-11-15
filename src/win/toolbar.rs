//! The toolbar: flat buttons with a symbol and a label, drawn by hand (the
//! common-controls toolbar is not wrapped by winsafe). It lays itself out
//! from measured label widths and reports which button a point is on.

use topos::session::Session;
use topos::theme::Palette;
use winsafe::{self as w, co};

use super::gdi::{self, Fonts};

/// A toolbar command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Rescan the target; while a scan runs, this button stops it instead.
    Reload,
    ZoomFull,
    ZoomIn,
    ZoomOut,
    FreeSpace,
    Open,
    Delete,
    Setup,
    /// The Help menu: Using Topos and About Topos.
    Help,
}

/// Buttons in order; `None` is a separator.
const LAYOUT: [Option<Tool>; 12] = [
    Some(Tool::Reload),
    None,
    Some(Tool::ZoomFull),
    Some(Tool::ZoomIn),
    Some(Tool::ZoomOut),
    Some(Tool::FreeSpace),
    None,
    Some(Tool::Open),
    Some(Tool::Delete),
    None,
    Some(Tool::Setup),
    Some(Tool::Help),
];

/// How a button looks right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    /// Segoe MDL2 Assets code point.
    pub glyph: char,
    pub label: &'static str,
    pub enabled: bool,
    /// Drawn pressed in (a toggle that is on).
    pub checked: bool,
}

/// A button's face for the session's current state.
#[must_use]
pub fn face(tool: Tool, session: &Session) -> Face {
    let scanned = session.scanned().is_some();
    let selected = session.selected();
    let face = |glyph, label, enabled| Face {
        glyph,
        label,
        enabled,
        checked: false,
    };
    match tool {
        Tool::Reload if session.is_scanning() => face('\u{E71A}', "Stop", true),
        Tool::Reload => face('\u{E72C}', "Reload", session.target().is_some()),
        Tool::ZoomFull => face(
            '\u{E80F}',
            "Zoom Full",
            scanned && !session.zoom().is_empty(),
        ),
        Tool::ZoomIn => face('\u{E8A3}', "Zoom In", session.can_zoom_in()),
        Tool::ZoomOut => face('\u{E71F}', "Zoom Out", !session.zoom().is_empty()),
        Tool::FreeSpace => Face {
            checked: session.settings.show_free_space,
            // Only a whole drive has free space to show.
            ..face('\u{EDA2}', "Free Space", session.target_is_drive())
        },
        Tool::Open => face('\u{E8E5}', "Run or Open", selected.is_some()),
        Tool::Delete => face('\u{E74D}', "Delete", session.can_delete()),
        Tool::Setup => face('\u{E713}', "Setup", true),
        Tool::Help => face('\u{E897}', "Help", true),
    }
}

/// A placed button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub tool: Tool,
    pub rect: w::RECT,
}

/// Mouse state of the toolbar.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    pub hover: Option<Tool>,
    pub pressed: Option<Tool>,
}

/// Fixed metrics at a DPI scale.
struct Metrics {
    margin: i32,
    pad: i32,
    icon: i32,
    gap: i32,
    separator: i32,
}

impl Metrics {
    fn new(dpi: u32) -> Self {
        let px = |v: i32| gdi::mul_div(v, dpi, 96);
        Self {
            margin: px(3),
            pad: px(7),
            icon: px(16),
            gap: px(5),
            separator: px(11),
        }
    }
}

/// Place every button left to right in a toolbar `height` pixels high,
/// using `label_width` to measure each label.
#[must_use]
pub fn place(
    height: i32,
    dpi: u32,
    session: &Session,
    label_width: impl Fn(&str) -> i32,
) -> Vec<Slot> {
    let m = Metrics::new(dpi);
    let mut x = 0;
    let mut slots = Vec::new();
    for entry in LAYOUT {
        let Some(tool) = entry else {
            x += m.separator;
            continue;
        };
        // Reload and Stop share a slot; size it for the wider label.
        let label = match tool {
            Tool::Reload => label_width("Reload").max(label_width("Stop")),
            _ => label_width(face(tool, session).label),
        };
        let width = m.pad + m.icon + m.gap + label + m.pad;
        slots.push(Slot {
            tool,
            rect: w::RECT {
                left: x,
                top: m.margin,
                right: x + width,
                bottom: height - m.margin,
            },
        });
        x += width + m.margin;
    }
    slots
}

/// The button under a point.
#[must_use]
pub fn hit(slots: &[Slot], x: i32, y: i32) -> Option<Tool> {
    slots
        .iter()
        .find(|slot| {
            x >= slot.rect.left && x < slot.rect.right && y >= slot.rect.top && y < slot.rect.bottom
        })
        .map(|slot| slot.tool)
}

/// Paint the toolbar into `hdc` (already filled with the face colour).
pub fn paint(
    hdc: &w::HDC,
    fonts: &Fonts,
    dpi: u32,
    slots: &[Slot],
    session: &Session,
    pointer: Pointer,
    palette: &Palette,
) {
    let m = Metrics::new(dpi);
    let text_color = gdi::colorref(palette.text);
    let gray = gdi::colorref(palette.disabled);
    let _ = hdc.SetBkMode(co::BKMODE::TRANSPARENT);
    let mut previous_right: Option<i32> = None;
    for slot in slots {
        // A gap wider than the usual margin is a separator: draw its line.
        if let Some(right) = previous_right
            && slot.rect.left - right > m.margin + m.margin
        {
            let x = right + (slot.rect.left - right) / 2;
            gdi::fill(
                hdc,
                w::RECT {
                    left: x,
                    top: slot.rect.top + m.margin,
                    right: x + 1,
                    bottom: slot.rect.bottom - m.margin,
                },
                palette.rule,
            );
        }
        previous_right = Some(slot.rect.right);

        let face = face(slot.tool, session);
        let hovered = face.enabled && pointer.hover == Some(slot.tool);
        let down = hovered && pointer.pressed == Some(slot.tool);
        let colors = if down || face.checked {
            Some((palette.down_fill, palette.down_edge))
        } else if hovered {
            Some((palette.hover_fill, palette.hover_edge))
        } else {
            None
        };
        if let Some((fill, edge)) = colors {
            gdi::fill(hdc, slot.rect, fill);
            gdi::outline(hdc, slot.rect, 1, edge);
        }
        let _ = hdc.SetTextColor(if face.enabled { text_color } else { gray });
        let icon = w::RECT {
            left: slot.rect.left + m.pad,
            right: slot.rect.left + m.pad + m.icon,
            ..slot.rect
        };
        if let Ok(_font) = hdc.SelectObject(&*fonts.icons) {
            gdi::text(
                hdc,
                &face.glyph.to_string(),
                icon,
                co::DT::CENTER | co::DT::VCENTER | co::DT::SINGLELINE,
            );
        }
        let label = w::RECT {
            left: icon.right + m.gap,
            right: slot.rect.right,
            ..slot.rect
        };
        if let Ok(_font) = hdc.SelectObject(&*fonts.ui) {
            gdi::text(
                hdc,
                face.label,
                label,
                co::DT::LEFT | co::DT::VCENTER | co::DT::SINGLELINE,
            );
        }
    }
}
