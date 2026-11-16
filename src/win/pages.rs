//! The Help menu's two windows, drawn in the window's palette so they are
//! light or dark with the rest of Topos:
//!
//! - Using Topos (F1): how to read the map, move around it, and the keys,
//!   in two columns so it fits the screen at any display scale.
//! - About Topos: what it is, how it keeps files safe, where it keeps its
//!   data, the license, and the publisher.

use std::path::Path;

use topos::icon;
use topos::theme::Palette;
use topos::view::{Frame, Rgb};
use winsafe::{self as w, co, gui, prelude::*};

use super::gdi::{self, mul_div};

const PROJECT_URL: &str = "https://github.com/Tuigse-oss/Topos";
const PROJECT_LABEL: &str = "github.com/Tuigse-oss/Topos";

/// Layout constants in 96-dpi pixels.
const MARGIN: i32 = 24;
const EMBLEM: i32 = 72;
const BUTTON_W: i32 = 88;
const BUTTON_H: i32 = 28;
/// Space between the help's two columns.
const GUTTER: i32 = 32;

/// Which window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Using Topos: how to read and move around the map.
    Help,
    About,
}

impl Kind {
    /// Client width, icon size, and column width in 96-dpi pixels.
    fn metrics(self) -> (i32, i32, i32) {
        match self {
            Kind::Help => (2 * 400 + GUTTER + 2 * MARGIN, 64, 400),
            Kind::About => (620, 96, 620 - 2 * MARGIN),
        }
    }
}

/// One block of the body text; a `Key` line's text is what the key does.
#[derive(Clone, Copy)]
enum Block<'a> {
    Heading(&'a str),
    Text(&'a str),
    Strong(&'a str),
    Bullet(&'a str),
    /// A key or mouse button, with what it does as the block's text.
    Key(&'a str),
    /// Continue in the next column.
    Column,
}

/// Fonts for the About text, at the window's DPI.
struct Fonts {
    title: w::guard::DeleteObjectGuard<w::HFONT>,
    heading: w::guard::DeleteObjectGuard<w::HFONT>,
    body: w::guard::DeleteObjectGuard<w::HFONT>,
    strong: w::guard::DeleteObjectGuard<w::HFONT>,
    link: w::guard::DeleteObjectGuard<w::HFONT>,
}

impl Fonts {
    fn new(dpi: u32) -> w::SysResult<Self> {
        let font = |points: i32, weight: co::FW, underline: bool| {
            let mut lf = w::LOGFONT::new_face(-mul_div(points, dpi, 72), "Segoe UI");
            lf.lfWeight = weight;
            lf.lfUnderline = u8::from(underline);
            lf.lfQuality = co::QUALITY::CLEARTYPE;
            w::HFONT::CreateFontIndirect(&lf)
        };
        Ok(Self {
            title: font(16, co::FW::SEMIBOLD, false)?,
            heading: font(11, co::FW::SEMIBOLD, false)?,
            body: font(9, co::FW::NORMAL, false)?,
            strong: font(9, co::FW::SEMIBOLD, false)?,
            link: font(9, co::FW::NORMAL, true)?,
        })
    }

    fn of(&self, block: Block<'_>) -> &w::HFONT {
        match block {
            Block::Heading(_) => &self.heading,
            Block::Strong(_) | Block::Key(_) => &self.strong,
            Block::Text(_) | Block::Bullet(_) | Block::Column => &self.body,
        }
    }
}

/// Everything the window shows, and where.
struct Page {
    kind: Kind,
    dpi: u32,
    palette: &'static Palette,
    fonts: Fonts,
    title: String,
    subtitle: String,
    blocks: Vec<(Block<'static>, String)>,
    /// Client size in pixels.
    width: i32,
    height: i32,
    /// Where the project link is drawn.
    link: w::RECT,
}

impl Page {
    fn px(&self, value: i32) -> i32 {
        mul_div(value, self.dpi, 96)
    }

    /// Lay the page out on `dc`, or paint it when `paint` is set; returns
    /// the height the content takes.
    fn walk(&mut self, dc: &w::HDC, paint: bool) -> i32 {
        let px = |v: i32| mul_div(v, self.dpi, 96);
        let (_, icon, column) = self.kind.metrics();
        let (margin, icon, column) = (px(MARGIN), px(icon), px(column));
        let right = self.width - margin;
        let about = self.kind == Kind::About;
        let palette = self.palette;
        let _ = dc.SetBkMode(co::BKMODE::TRANSPARENT);
        let draw = |text: &str, font: &w::HFONT, color: Rgb, rc: w::RECT, flags: co::DT| {
            let Ok(_font) = dc.SelectObject(font) else {
                return 0;
            };
            let mut bounds = rc;
            let _ = dc.DrawText(
                text,
                &mut bounds,
                flags | co::DT::CALCRECT | co::DT::NOPREFIX,
            );
            if paint {
                let _ = dc.SetTextColor(gdi::colorref(color));
                gdi::text(dc, text, rc, flags);
            }
            bounds.bottom - bounds.top
        };

        // Header: the icon, the title, and a line under it; About also has
        // the publisher's emblem.
        let text_x = margin + icon + px(20);
        let header_right = if about {
            right - px(EMBLEM) - px(16)
        } else {
            right
        };
        let mut y = margin + (icon - px(44)).max(0) / 2;
        y += draw(
            &self.title,
            &self.fonts.title,
            palette.text,
            rect(text_x, y, header_right, y + px(40)),
            co::DT::LEFT | co::DT::SINGLELINE,
        );
        draw(
            &self.subtitle,
            &self.fonts.body,
            palette.muted,
            rect(text_x, y, header_right, y + px(20)),
            co::DT::LEFT | co::DT::SINGLELINE,
        );
        if about {
            let emblem_x = right - px(EMBLEM);
            draw(
                "Tuigse LLC",
                &self.fonts.body,
                palette.muted,
                rect(
                    emblem_x - px(10),
                    margin + px(EMBLEM) + px(4),
                    right + px(10),
                    margin + px(EMBLEM) + px(24),
                ),
                co::DT::CENTER | co::DT::SINGLELINE,
            );
        }

        // Body, flowing down one column, then the next after a break.
        let top = margin + icon.max(if about { px(EMBLEM + 24) } else { 0 }) + px(18);
        let mut y = top;
        let mut bottom = top;
        let mut left = margin;
        let wrap = co::DT::LEFT | co::DT::WORDBREAK;
        for (block, text) in &self.blocks {
            let font = self.fonts.of(*block);
            let right = left + column;
            match block {
                Block::Column => {
                    bottom = bottom.max(y);
                    left += column + px(GUTTER);
                    y = top;
                }
                Block::Key(key) => {
                    // The key in a fixed first column, what it does beside it.
                    let split = left + column * 9 / 20;
                    let key_h = draw(
                        key,
                        font,
                        palette.text,
                        rect(left, y, split - px(8), y + 1000),
                        wrap,
                    );
                    let what_h = draw(
                        text,
                        &self.fonts.body,
                        palette.text,
                        rect(split, y, right, y + 1000),
                        wrap,
                    );
                    y += key_h.max(what_h) + px(3);
                }
                Block::Heading(_) => {
                    if y > top {
                        y += px(8);
                    }
                    y += draw(
                        text,
                        font,
                        palette.text,
                        rect(left, y, right, y + 1000),
                        wrap,
                    );
                    y += px(4);
                }
                Block::Bullet(_) => {
                    let indent = px(16);
                    draw(
                        "•",
                        font,
                        palette.text,
                        rect(left + px(2), y, left + indent, y + 1000),
                        co::DT::LEFT | co::DT::SINGLELINE,
                    );
                    y += draw(
                        text,
                        font,
                        palette.text,
                        rect(left + indent, y, right, y + 1000),
                        wrap,
                    );
                    y += px(3);
                }
                Block::Text(_) | Block::Strong(_) => {
                    y += draw(
                        text,
                        font,
                        palette.text,
                        rect(left, y, right, y + 1000),
                        wrap,
                    );
                    y += px(6);
                }
            }
        }
        let mut y = bottom.max(y);

        // Footer: About's project link beside the OK button.
        y += px(10);
        let mut link_h = 0;
        if about {
            link_h = draw(
                PROJECT_LABEL,
                &self.fonts.link,
                palette.link,
                rect(margin, y, right, y + 1000),
                co::DT::LEFT | co::DT::SINGLELINE,
            );
            if let Ok(_font) = dc.SelectObject(&*self.fonts.link) {
                let width = gdi::measure(dc, PROJECT_LABEL).cx;
                self.link = rect(margin, y, margin + width, y + link_h);
            }
        }
        y + px(BUTTON_H).max(link_h) + margin
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> w::RECT {
    w::RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn text(block: Block<'static>) -> (Block<'static>, String) {
    let words = match block {
        Block::Heading(t) | Block::Text(t) | Block::Strong(t) | Block::Bullet(t) => t,
        Block::Key(_) | Block::Column => "",
    };
    (block, words.to_owned())
}

fn key(keys: &'static str, what: &str) -> (Block<'static>, String) {
    (Block::Key(keys), what.to_owned())
}

/// Using Topos.
fn help_blocks() -> Vec<(Block<'static>, String)> {
    vec![
        text(Block::Heading("Reading the map")),
        text(Block::Bullet(
            "Every folder and file is a tile sized by the space it takes. A folder shows its \
             name and size on a title strip, with its contents inside.",
        )),
        text(Block::Bullet(
            "Folder colours follow depth below the drive, so a folder keeps its colour as you \
             zoom. Files are a lighter tint of their folder's colour, or coloured by type \
             (Setup), with a legend beside the path.",
        )),
        text(Block::Bullet(
            "A hatched tile such as \"2,500 small files\" holds items too small to name one by \
             one, with their total and the largest of them; double-click it to open their \
             folder. Setup's density decides how small is too small.",
        )),
        text(Block::Bullet(
            "Grey folders could not be read. The free-space tile shows the drive's free space \
             (the Free Space button turns it off).",
        )),
        text(Block::Bullet(
            "Rest the pointer on a tile for its exact size, date, and path.",
        )),
        text(Block::Bullet(
            "While Topos scans, a bar under the path (and on the taskbar button) fills as it \
             goes, estimated from the last scan; with no last scan it sweeps across instead. A \
             rescan keeps the last map on screen until the new one is ready.",
        )),
        text(Block::Heading("Moving around")),
        text(Block::Bullet(
            "Double-click a folder to open it, or a file to open its folder. The folder a \
             double-click would open is outlined in blue, and the status row names it.",
        )),
        text(Block::Bullet(
            "Scroll up to go one level into the folder under the pointer; scroll down to go up \
             one.",
        )),
        text(Block::Bullet(
            "The buttons left of the path go back, forward, and home to the whole drive; the \
             path jumps to any level. The drive field picks another drive.",
        )),
        text(Block::Bullet(
            r#"From a terminal, Topos.exe D:\ or Topos.exe "D:\My Games" opens a drive or folder."#,
        )),
        text(Block::Column),
        text(Block::Heading("Keys and mouse buttons")),
        key("Arrow keys", "Select the neighbouring tile"),
        key("Enter", "Open the selected folder"),
        key("Backspace, Alt+Up", "Up one level"),
        key("Alt+Left, mouse Back", "Back"),
        key("Alt+Right, mouse Forward", "Forward"),
        key("Home", "The whole drive"),
        key("F5", "Scan again"),
        key("Delete", "Move the selection to the Recycle Bin"),
        key("Esc", "Stop a scan, or clear the selection"),
        key("F1", "This help"),
        text(Block::Heading("Doing things")),
        text(Block::Bullet(
            "Right-click a tile to run or open it, show it in Explorer, zoom, or delete it.",
        )),
        text(Block::Bullet(
            "Delete asks first, then moves the item to the Recycle Bin; Topos never deletes \
             anything permanently.",
        )),
        text(Block::Heading("Setup")),
        text(Block::Bullet(
            "Free space on the map, delete confirmation, colours by file type, the classic map \
             font, the map's density (detailed, balanced, or simple), and light, dark, or as \
             Windows apps are set. Choices are kept for next time.",
        )),
    ]
}

/// About Topos.
fn about_blocks(data_dir: Option<&Path>) -> Vec<(Block<'static>, String)> {
    let data = data_dir.map_or_else(
        || String::from(r"%LOCALAPPDATA%\Topos"),
        |dir| dir.display().to_string(),
    );
    vec![
        text(Block::Text(
            "Topos shows where your disk space went: every folder and file as a tile sized by \
             the space it takes, nested inside its folder, with the drive's free space beside \
             them. Help > Using Topos (F1) shows how to read and move around the map.",
        )),
        text(Block::Heading("How it keeps your files safe")),
        text(Block::Bullet(
            "Scanning only reads. Nothing changes until you choose Delete.",
        )),
        text(Block::Bullet(
            "Delete asks first, then moves the selection to the Recycle Bin; Topos never deletes \
             anything permanently.",
        )),
        text(Block::Bullet(
            "Topos sends nothing anywhere: no telemetry, no update checks.",
        )),
        text(Block::Heading("Where things are kept")),
        (
            Block::Text(""),
            format!("Settings and each drive's last scan: {data}"),
        ),
        text(Block::Strong(
            "Use at your own risk: provided as is, without warranty (MIT or Apache-2.0). Notices \
             for the open-source parts it includes are in THIRD-PARTY-NOTICES.txt, shipped with \
             it. The Tuigse LLC name and emblem are Tuigse LLC's marks.",
        )),
    ]
}

/// The icon and the emblem, composited on the window face.
fn art(page: &Page, size: w::SIZE) -> Frame {
    let width = usize::try_from(size.cx).unwrap_or(0);
    let height = usize::try_from(size.cy).unwrap_or(0);
    let mut frame = Frame::new(width, height, page.palette.face);
    let margin = page.px(MARGIN);
    let (_, icon, _) = page.kind.metrics();
    let icon_px = usize::try_from(page.px(icon)).unwrap_or(64);
    frame.blend(&icon::render(icon_px), margin, margin);
    if page.kind == Kind::About {
        let emblem_px = usize::try_from(page.px(EMBLEM)).unwrap_or(72);
        frame.blend(
            &topos::emblem::render(emblem_px),
            page.width - margin - page.px(EMBLEM),
            margin,
        );
    }
    frame
}

/// Show the Help or About window over `parent` until it is closed.
pub fn show(
    kind: Kind,
    parent: &gui::WindowMain,
    palette: &'static Palette,
    dpi: u32,
    data_dir: Option<&Path>,
) {
    let Ok(fonts) = Fonts::new(dpi) else {
        return;
    };
    let version = format!("Topos {}", env!("CARGO_PKG_VERSION"));
    let (title, subtitle, blocks, window_title) = match kind {
        Kind::Help => (
            String::from("Using Topos"),
            version,
            help_blocks(),
            "Using Topos",
        ),
        Kind::About => (
            version,
            String::from("a Tuigse LLC project"),
            about_blocks(data_dir),
            "About Topos",
        ),
    };
    let (width, ..) = kind.metrics();
    let mut page = Page {
        kind,
        dpi,
        palette,
        fonts,
        title,
        subtitle,
        blocks,
        width: mul_div(width, dpi, 96),
        height: 0,
        link: w::RECT::default(),
    };
    // Measure on a screen DC, so the window is created at its final size.
    let Ok(screen) = w::HWND::NULL.GetDC() else {
        return;
    };
    page.height = page.walk(&screen, false);
    drop(screen);

    let modal = gui::WindowModal::new(gui::WindowModalOpts {
        title: window_title,
        class_bg_brush: gui::Brush::None,
        size: (page.width, page.height),
        ..Default::default()
    });
    let ok = gui::Button::new(
        &modal,
        gui::ButtonOpts {
            text: "OK",
            position: (
                page.width - page.px(MARGIN) - page.px(BUTTON_W),
                page.height - page.px(MARGIN) - page.px(BUTTON_H),
            ),
            width: page.px(BUTTON_W),
            height: page.px(BUTTON_H),
            control_style: co::BS::DEFPUSHBUTTON,
            ctrl_id: 1,
            ..Default::default()
        },
    );
    let page = std::rc::Rc::new(std::cell::RefCell::new(page));

    let (modal2, ok2) = (modal.clone(), ok.clone());
    modal.on().wm_create(move |_| {
        let dark = palette.dark;
        let _ = modal2
            .hwnd()
            .DwmSetWindowAttribute(w::DwmAttr::UseImmersiveDarkMode(dark));
        let theme = if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        };
        let _ = ok2.hwnd().SetWindowTheme(theme, None);
        let _ = ok2.hwnd().SetFocus();
        Ok(0)
    });
    let (modal2, page2) = (modal.clone(), page.clone());
    modal.on().wm_paint(move || {
        let hwnd = modal2.hwnd();
        let Ok(hdc) = hwnd.BeginPaint() else {
            return Ok(());
        };
        let Ok(mut page) = page2.try_borrow_mut() else {
            return Ok(());
        };
        let _ = super::buffered(&hdc, hwnd, |dc, rc| {
            let size = w::SIZE::with(rc.right, rc.bottom);
            let frame = art(&page, size);
            if let Ok(bitmap) = gdi::frame_bitmap(dc, &frame)
                && let Ok(src) = dc.CreateCompatibleDC()
                && let Ok(_old) = src.SelectObject(&*bitmap)
            {
                let _ = dc.BitBlt(
                    w::POINT::default(),
                    size,
                    &src,
                    w::POINT::default(),
                    co::ROP::SRCCOPY,
                );
            } else {
                gdi::fill(dc, rc, page.palette.face);
            }
            page.walk(dc, true);
        });
        Ok(())
    });
    let page2 = page.clone();
    modal.on().wm_l_button_up(move |p| {
        let on_link = page2.try_borrow().is_ok_and(|page| {
            let r = page.link;
            p.coords.x >= r.left
                && p.coords.x < r.right
                && p.coords.y >= r.top
                && p.coords.y < r.bottom
        });
        if on_link {
            let _ = opener::open(PROJECT_URL);
        }
        Ok(())
    });
    let modal2 = modal.clone();
    ok.on().bn_clicked(move || {
        modal2.close();
        Ok(())
    });
    // Escape arrives as the dialog's Cancel command.
    let modal2 = modal.clone();
    modal
        .on()
        .wm_command(co::DLGID::CANCEL, co::BN::CLICKED, move || {
            modal2.close();
            Ok(())
        });
    let _ = modal.show_modal(parent);
}
