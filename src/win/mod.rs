//! The Win32 shell: a main window holding the drive list, a hand-drawn
//! toolbar and breadcrumb row, the map, and a hand-drawn status row.
//!
//! Everything drawn here takes its colours from the active
//! [`Palette`] — light, dark, or as Windows apps are set — so one switch
//! repaints the window; the title bar and the drive list follow through
//! their own dark themes. Menus and task dialogs stay light: Windows has no
//! documented way to darken them.
//!
//! All behavior lives in [`Session`]; this module turns input into session
//! calls and draws what the session says. The map is painted once per
//! layout into a cached bitmap — tiles from the library's pixel buffer in
//! one copy, then captions with GDI text — so moving the mouse costs one
//! bitmap copy plus the highlight and tooltip.
//!
//! Re-entrancy: dialogs, menus, and the shell's "open" run their own message
//! loops, which paint and fire timers while they are up. No `RefCell` borrow
//! is held across such a call, and painting uses `try_borrow`, so a nested
//! message never finds state borrowed and never panics.

mod gdi;
mod pages;
mod toolbar;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Instant;

use topos::format::{self, exact_bytes, pretty_bytes};
use topos::scanner;
use topos::session::{Activity, DeleteRequest, Session, Settings};
use topos::theme::{Appearance, Palette};
use topos::treemap::{self, Density, Direction, NO_PARENT, TileKind};
use topos::view::{self, FileType, Rgb, View};
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use gdi::Fonts;
use toolbar::{Pointer, Slot, Tool};

/// Polls background work while a scan or delete runs.
const POLL_TIMER: usize = 1;
const POLL_MS: u32 = 33;
/// Shows the hover tooltip once the pointer rests on a tile.
const TIP_TIMER: usize = 2;
const TIP_DELAY_MS: u32 = 400;

/// The window's icon group in the exe (`build.rs`): Windows halves its large
/// frame for the title bar, and this group's large frames are the small
/// drawings doubled, so the halving is exact.
const WINDOW_ICON_ID: u16 = 2;

// Menu command identifiers.
const CMD_OPEN: u16 = 101;
const CMD_REVEAL: u16 = 102;
const CMD_ZOOM_IN: u16 = 103;
const CMD_ZOOM_OUT: u16 = 104;
const CMD_DELETE: u16 = 105;
const SET_FREE_SPACE: u16 = 201;
const SET_CONFIRM: u16 = 202;
const SET_BY_TYPE: u16 = 203;
const SET_CLASSIC_FONT: u16 = 204;
const SET_APPEARANCE_SYSTEM: u16 = 211;
const SET_APPEARANCE_LIGHT: u16 = 212;
const SET_APPEARANCE_DARK: u16 = 213;
const SET_DENSITY_DETAILED: u16 = 221;
const SET_DENSITY_BALANCED: u16 = 222;
const SET_DENSITY_SIMPLE: u16 = 223;
/// Drive menu items are this plus the item's index.
const CMD_DRIVE: u16 = 301;
// Keyboard accelerators (Alt+Left, Alt+Right, Alt+Up), as in Explorer.
const ACC_BACK: u16 = 401;
const ACC_FORWARD: u16 = 402;
const ACC_UP: u16 = 403;
const ACC_HELP: u16 = 404;
const CMD_HELP: u16 = 501;
const CMD_ABOUT: u16 = 502;
/// One notch of a mouse wheel.
const WHEEL_NOTCH: i32 = 120;
const BUTTON_MOVE: u16 = 1001;

/// Open the window, scanning `start` (or the first drive), and run until it
/// closes.
pub fn run(start: Option<PathBuf>) -> ExitCode {
    // COM for the taskbar button's progress, for the life of the process
    // (the guard is kept, never dropped, so COM is never shut down under
    // the window).
    if let Ok(com) = w::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)
    {
        std::mem::forget(com);
    }
    let outcome = App::new(start).and_then(|app| app.wnd.run_main(None));
    match outcome {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            let _ = w::HWND::NULL.MessageBox(&err.to_string(), "Topos", co::MB::ICONERROR);
            ExitCode::FAILURE
        }
    }
}

/// The painted map for one layout.
struct MapCache {
    view: View,
    /// Tiles and captions, ready to copy to the screen.
    bitmap: DeleteObjectGuard<w::HBITMAP>,
    classic_font: bool,
}

/// Where a click in the breadcrumb row goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Crumb {
    Back,
    Forward,
    /// The whole drive or folder that was scanned.
    Home,
    /// Zoom to this folder.
    Zoom(Vec<usize>),
}

/// Text last pushed to native controls, so unchanged text is not resent.
#[derive(Default)]
struct Shown {
    title: String,
    status: [String; 2],
    drives: Vec<String>,
    drive_index: Option<u32>,
}

/// Window-side state: fonts, the painted map, and pointer tracking.
struct Ui {
    dpi: u32,
    fonts: Fonts,
    /// Caption line heights for the two map fonts, in pixels.
    line_classic: u32,
    line_modern: u32,
    cache: Option<MapCache>,
    /// Reused for map paints; replaced on resize and released on minimise.
    map_back_buffer: Option<gdi::BackBuffer>,
    /// Last pointer position over the map.
    mouse: Option<(i32, i32)>,
    /// Tile under the pointer.
    hover: Option<usize>,
    /// Where the tooltip is shown, once the pointer has rested.
    tip: Option<(i32, i32)>,
    map_tracking: bool,
    slots: Vec<Slot>,
    pointer: Pointer,
    tools_tracking: bool,
    /// Breadcrumb links (and the Back and Forward arrows) and where each
    /// one goes.
    crumbs: Vec<(w::RECT, Crumb)>,
    /// The folder tile a double-click under the pointer would open, outlined
    /// while the pointer rests on the map, and the status line saying so.
    target: Option<usize>,
    hint: Option<String>,
    /// Wheel movement not yet a whole notch (touchpads send small steps).
    wheel: i32,
    crumb_hover: Option<usize>,
    /// The breadcrumb button held down, drawn pressed.
    crumb_pressed: Option<usize>,
    crumbs_tracking: bool,
    drives_hover: bool,
    drives_tracking: bool,
    polling: bool,
    shown: Shown,
    /// The colours everything is drawn with right now.
    palette: &'static Palette,
    /// When the running background work began, for the sweeping bars.
    busy_since: Option<Instant>,
    /// The taskbar button, for showing progress on it, and what it shows.
    taskbar: Option<w::ITaskbarList3>,
    taskbar_shown: Option<(co::TBPF, u64)>,
}

impl Ui {
    fn new(dpi: u32) -> w::AnyResult<Self> {
        let fonts = Fonts::new(dpi)?;
        let screen = w::HWND::NULL.GetDC()?;
        let line = |font: &w::HFONT| -> w::AnyResult<u32> {
            let _old = screen.SelectObject(font)?;
            let metrics = screen.GetTextMetrics()?;
            Ok(u32::try_from(metrics.tmHeight.max(1)).unwrap_or(13) + 1)
        };
        let line_classic = line(&fonts.map_classic)?;
        let line_modern = line(&fonts.map_modern)?;
        Ok(Self {
            dpi,
            fonts,
            line_classic,
            line_modern,
            cache: None,
            map_back_buffer: None,
            mouse: None,
            hover: None,
            tip: None,
            map_tracking: false,
            slots: Vec::new(),
            pointer: Pointer::default(),
            tools_tracking: false,
            crumbs: Vec::new(),
            target: None,
            hint: None,
            wheel: 0,
            crumb_hover: None,
            crumb_pressed: None,
            crumbs_tracking: false,
            drives_hover: false,
            drives_tracking: false,
            polling: false,
            shown: Shown::default(),
            palette: Palette::of(false),
            busy_since: None,
            taskbar: None,
            taskbar_shown: None,
        })
    }

    fn px(&self, value: i32) -> i32 {
        gdi::mul_div(value, self.dpi, 96)
    }

    fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

#[derive(Clone)]
struct App {
    wnd: gui::WindowMain,
    map: gui::WindowControl,
    /// The drive field: shows the drive or folder on the map and opens
    /// the drive menu. Drawn by hand, so it follows the palette (a native
    /// combo box cannot be made dark without undocumented calls).
    drives: gui::WindowControl,
    tools: gui::WindowControl,
    crumbs: gui::WindowControl,
    status: gui::WindowControl,
    session: Rc<RefCell<Session>>,
    /// `%LOCALAPPDATA%\Topos`: settings and each drive's last scan.
    data_dir: Option<PathBuf>,
    settings_file: Option<PathBuf>,
    ui: Rc<RefCell<Ui>>,
}

impl App {
    fn new(start: Option<PathBuf>) -> w::AnyResult<Self> {
        let dpi = u32::try_from(gui::dpi_x(96)).unwrap_or(96);
        let ui = Ui::new(dpi)?;
        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: "Topos",
            class_icon: gui::Icon::Id(WINDOW_ICON_ID),
            size: gui::dpi(1280, 800),
            style: co::WS::OVERLAPPEDWINDOW | co::WS::CLIPCHILDREN | co::WS::VISIBLE,
            // The map handles its own keys; dialog navigation would take
            // Enter and Escape from it.
            process_dlg_msgs: false,
            accel_table: accelerators(),
            ..Default::default()
        });
        let custom = || gui::WindowControlOpts {
            class_bg_brush: gui::Brush::None,
            size: (1, 1),
            style: co::WS::CHILD | co::WS::VISIBLE | co::WS::CLIPSIBLINGS,
            ..Default::default()
        };
        // Created first, so it is the child that takes the focus.
        let map = gui::WindowControl::new(
            &wnd,
            gui::WindowControlOpts {
                style: co::WS::CHILD | co::WS::VISIBLE | co::WS::TABSTOP | co::WS::CLIPSIBLINGS,
                ..custom()
            },
        );
        let drives = gui::WindowControl::new(&wnd, custom());
        let tools = gui::WindowControl::new(&wnd, custom());
        let crumbs = gui::WindowControl::new(&wnd, custom());
        let status = gui::WindowControl::new(&wnd, custom());
        // Setup choices and each drive's last scan share the app folder.
        let data_dir = std::env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty())
            .map(|dir| PathBuf::from(dir).join("Topos"));
        let settings_file = data_dir.as_ref().map(|dir| dir.join("settings.txt"));
        let settings = settings_file
            .as_deref()
            .and_then(|path| Settings::load(path).ok())
            .unwrap_or_default();
        let mut session = Session::new(start, scanner::drives(), data_dir.clone());
        session.settings = settings;
        session.system_dark = system_wants_dark();
        let app = Self {
            wnd,
            map,
            drives,
            tools,
            crumbs,
            status,
            session: Rc::new(RefCell::new(session)),
            data_dir,
            settings_file,
            ui: Rc::new(RefCell::new(ui)),
        };
        app.main_events();
        app.map_events();
        app.tools_events();
        app.crumbs_events();
        app.status_events();
        app.drives_events();
        Ok(app)
    }

    // ---- main window -------------------------------------------------

    fn main_events(&self) {
        let app = self.clone();
        self.wnd.on().wm_create(move |_| {
            app.connect_taskbar();
            app.apply_theme();
            app.layout();
            app.ensure_polling();
            app.refresh();
            let _ = app.map.hwnd().SetFocus();
            Ok(0)
        });
        let app = self.clone();
        self.wnd.on().wm_size(move |_| {
            if app.wnd.hwnd().IsIconic()
                && let Ok(mut ui) = app.ui.try_borrow_mut()
            {
                ui.map_back_buffer = None;
            }
            app.layout();
            Ok(())
        });
        let app = self.clone();
        self.wnd.on().wm_get_min_max_info(move |p| {
            if let Ok(ui) = app.ui.try_borrow() {
                p.info.ptMinTrackSize = w::POINT::with(ui.px(640), ui.px(420));
            }
            Ok(())
        });
        let app = self.clone();
        self.wnd.on().wm_command_acc_menu(ACC_HELP, move || {
            app.show_page(pages::Kind::Help);
            Ok(())
        });
        for (id, action) in [
            (ACC_BACK, Nav::Back),
            (ACC_FORWARD, Nav::Forward),
            (ACC_UP, Nav::Up),
        ] {
            let app = self.clone();
            self.wnd.on().wm_command_acc_menu(id, move || {
                app.navigate(action);
                Ok(())
            });
        }
        // Sign-out, shutdown, and Setup closing Topos to update it (Restart
        // Manager): close as the window's own close button would. Nothing
        // must finish first: settings are saved as they change, and a scan's
        // cache file is written whole or not at all.
        let app = self.clone();
        self.wnd.on().wm(co::WM::ENDSESSION, move |p| {
            if p.wparam != 0 {
                app.wnd.close();
            }
            Ok(0)
        });
        let app = self.clone();
        self.wnd.on().wm_timer(POLL_TIMER, move || {
            app.tick();
            Ok(())
        });
        // Keys belong to the map, whichever child winsafe hands the main
        // window's focus to.
        let app = self.clone();
        self.wnd.on().wm_set_focus(move |_| {
            let _ = app.map.hwnd().SetFocus();
            Ok(())
        });
        // Windows announces a change of the app light/dark mode (among many
        // other settings) to every top-level window; re-read it each time.
        let app = self.clone();
        self.wnd.on().wm(co::WM::WININICHANGE, move |_| {
            let dark = system_wants_dark();
            let changed = app.session.try_borrow_mut().is_ok_and(|mut session| {
                let changed = session.system_dark != dark;
                session.system_dark = dark;
                changed
            });
            if changed {
                app.apply_theme();
            }
            Ok(0)
        });
    }

    /// Place the child windows for the current client size.
    fn layout(&self) {
        let Ok(client) = self.wnd.hwnd().GetClientRect() else {
            return;
        };
        let (bar, row, gap, combo_x, combo_w) = match self.ui.try_borrow() {
            // The breadcrumb row is tall enough for its Back, Forward, and
            // Home buttons to be easy targets.
            Ok(ui) => (ui.px(34), ui.px(30), ui.px(8), ui.px(6), ui.px(220)),
            Err(_) => return,
        };
        let status_h = match self.ui.try_borrow() {
            Ok(ui) => ui.px(23),
            Err(_) => return,
        };
        let place = |hwnd: &w::HWND, x: i32, y: i32, cx: i32, cy: i32| {
            let _ = hwnd.SetWindowPos(
                w::HwndPlace::None,
                w::POINT::with(x, y),
                w::SIZE::with(cx.max(0), cy.max(0)),
                co::SWP::NOZORDER | co::SWP::NOACTIVATE,
            );
        };
        // The drive control fills the toolbar row's left end, so no part of
        // the main window's own background shows; its field sits inside.
        let tools_x = combo_x + combo_w + gap;
        place(self.drives.hwnd(), 0, 0, tools_x, bar);
        place(self.tools.hwnd(), tools_x, 0, client.right - tools_x, bar);
        place(self.crumbs.hwnd(), 0, bar, client.right, row);
        place(
            self.map.hwnd(),
            0,
            bar + row,
            client.right,
            client.bottom - bar - row - status_h,
        );
        place(
            self.status.hwnd(),
            0,
            client.bottom - status_h,
            client.right,
            status_h,
        );
    }

    /// Resolve the palette from the Appearance choice and Windows' mode, and
    /// repaint everything with it: the title bar and every drawn row. The map rebuilds itself, since its key holds
    /// the darkness.
    fn apply_theme(&self) {
        let Ok(dark) = self.session.try_borrow().map(|session| session.dark()) else {
            return;
        };
        if let Ok(mut ui) = self.ui.try_borrow_mut() {
            ui.palette = Palette::of(dark);
        }
        let _ = self
            .wnd
            .hwnd()
            .DwmSetWindowAttribute(w::DwmAttr::UseImmersiveDarkMode(dark));
        let _ = self.wnd.hwnd().InvalidateRect(None, true);
        for hwnd in [
            self.drives.hwnd(),
            self.tools.hwnd(),
            self.crumbs.hwnd(),
            self.map.hwnd(),
            self.status.hwnd(),
        ] {
            let _ = hwnd.InvalidateRect(None, true);
        }
    }

    /// The taskbar button's progress, when the shell offers it.
    fn connect_taskbar(&self) {
        let taskbar = w::CoCreateInstance::<w::ITaskbarList3>(
            &co::CLSID::TaskbarList,
            None::<&w::IUnknown>,
            co::CLSCTX::INPROC_SERVER,
        )
        .ok()
        .filter(|taskbar| taskbar.HrInit().is_ok());
        if let Ok(mut ui) = self.ui.try_borrow_mut() {
            ui.taskbar = taskbar;
        }
    }

    /// Keep the progress shown in step with the work: note when it began
    /// (for the sweeping bars), and set the taskbar button's bar, which
    /// shows even while Topos is minimised. The button is told only of
    /// changes; a call that fails (the button may not exist yet) is retried
    /// on the next poll.
    fn update_progress(&self, activity: Option<Activity>) {
        let Ok(mut ui) = self.ui.try_borrow_mut() else {
            return;
        };
        match (activity.is_some(), ui.busy_since.is_some()) {
            (true, false) => ui.busy_since = Some(Instant::now()),
            (false, true) => ui.busy_since = None,
            _ => {}
        }
        let wanted = match activity {
            None => (co::TBPF::NOPROGRESS, 0),
            // In 0..=990 by the share's cap.
            Some(Activity::Scanning { done: Some(done) }) => {
                (co::TBPF::NORMAL, (done * 1000.0) as u64)
            }
            Some(_) => (co::TBPF::INDETERMINATE, 0),
        };
        if ui.taskbar_shown == Some(wanted) {
            return;
        }
        let Some(taskbar) = &ui.taskbar else {
            return;
        };
        let hwnd = self.wnd.hwnd();
        let shown = if wanted.0 == co::TBPF::NORMAL {
            taskbar.SetProgressValue(hwnd, wanted.1, 1000).is_ok()
        } else {
            taskbar.SetProgressState(hwnd, wanted.0).is_ok()
        };
        if shown {
            ui.taskbar_shown = Some(wanted);
        }
    }

    /// Keep polling while background work runs.
    fn ensure_polling(&self) {
        let busy = self.session.borrow().busy();
        let mut ui = self.ui.borrow_mut();
        if busy && !ui.polling {
            ui.polling = self.wnd.hwnd().SetTimer(POLL_TIMER, POLL_MS, None).is_ok();
        }
    }

    fn tick(&self) {
        let Ok(mut session) = self.session.try_borrow_mut() else {
            return;
        };
        let changed = session.poll();
        let busy = session.busy();
        drop(session);
        if !busy && let Ok(mut ui) = self.ui.try_borrow_mut() {
            let _ = self.wnd.hwnd().KillTimer(POLL_TIMER);
            ui.polling = false;
        }
        self.refresh_after_poll(changed);
    }

    /// Bring the title, status bar, drive list, and drawn rows up to date
    /// with the session.
    fn refresh(&self) {
        self.refresh_after_poll(true);
    }

    /// Polling keeps progress animated, but a map already on screen needs
    /// no paint until the session changes. Commands request a full refresh.
    fn refresh_after_poll(&self, changed: bool) {
        let (title, status, drives, drive_index) = {
            let Ok(session) = self.session.try_borrow() else {
                return;
            };
            self.update_progress(session.activity());
            let status = [
                session
                    .progress_line()
                    .unwrap_or_else(|| session.status().to_owned()),
                session.totals_line().unwrap_or_default(),
            ];
            let (drives, drive_index) = drive_items(&session);
            (session.title(), status, drives, drive_index)
        };
        let Ok(mut ui) = self.ui.try_borrow_mut() else {
            return;
        };
        let shown = &mut ui.shown;
        let new_title = (shown.title != title).then(|| title.clone());
        let new_status = (shown.status != status).then(|| status.clone());
        let new_drives = (shown.drives != drives).then(|| drives.clone());
        let new_index = new_drives.is_some() || shown.drive_index != drive_index;
        shown.title = title;
        shown.status = status;
        shown.drives = drives;
        shown.drive_index = drive_index;
        // Before the first map (or after a failed bitmap allocation), keep
        // painting the progress line and retrying map creation.
        let repaint_map = changed || ui.cache.is_none();
        drop(ui);

        // These send messages to other windows; nothing is borrowed now.
        if let Some(title) = new_title {
            let _ = self.wnd.hwnd().SetWindowText(&title);
        }
        let status_changed = new_status.is_some();
        if new_drives.is_some() || new_index {
            let _ = self.drives.hwnd().InvalidateRect(None, false);
        }
        let _ = self.crumbs.hwnd().InvalidateRect(None, false);
        if changed {
            let _ = self.tools.hwnd().InvalidateRect(None, false);
        }
        if repaint_map {
            let _ = self.map.hwnd().InvalidateRect(None, false);
        }
        if status_changed {
            let _ = self.status.hwnd().InvalidateRect(None, false);
        }
    }

    /// The drive menu, under the drive field: every drive with its free
    /// space, plus the folder on the map when it is not a drive.
    fn drive_menu(&self) {
        let (items, current, below) = {
            let Ok(ui) = self.ui.try_borrow() else {
                return;
            };
            let below = self
                .drives
                .hwnd()
                .GetClientRect()
                .map_or(w::POINT::default(), |rc| {
                    let field = drive_field(rc, &ui);
                    w::POINT::with(field.left, field.bottom)
                });
            (ui.shown.drives.clone(), ui.shown.drive_index, below)
        };
        let entries: Vec<(u16, &str, bool, Option<bool>)> = items
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                let id = CMD_DRIVE.checked_add(u16::try_from(index).ok()?)?;
                let current = current.and_then(|c| usize::try_from(c).ok()) == Some(index);
                Some((id, text.as_str(), true, Some(current)))
            })
            .collect();
        let choice = popup(self.drives.hwnd(), below, &entries);
        if let Some(id) = choice.and_then(|id| id.checked_sub(CMD_DRIVE)) {
            self.drive_chosen(usize::from(id));
        }
        if let Ok(mut ui) = self.ui.try_borrow_mut() {
            ui.drives_hover = false;
        }
        let _ = self.drives.hwnd().InvalidateRect(None, false);
    }

    fn drive_chosen(&self, index: usize) {
        {
            let Ok(mut session) = self.session.try_borrow_mut() else {
                return;
            };
            let mount = session
                .drives()
                .get(index)
                .map(|drive| PathBuf::from(&drive.mount));
            // The extra item past the drives is the folder being shown.
            if let Some(root) = mount
                && session.target() != Some(root.as_path())
            {
                session.start_scan(root);
            }
        }
        self.ensure_polling();
        self.refresh();
        let _ = self.map.hwnd().SetFocus();
    }

    // ---- commands ----------------------------------------------------

    fn run_tool(&self, tool: Tool) {
        match tool {
            Tool::Open => self.open_selected(false),
            Tool::Delete => self.delete(),
            Tool::Setup => self.setup_menu(),
            Tool::Help => self.help_menu(),
            _ => {
                let Ok(mut session) = self.session.try_borrow_mut() else {
                    return;
                };
                match tool {
                    Tool::Reload if session.is_scanning() => session.stop_scan(),
                    Tool::Reload => session.reload(),
                    Tool::ZoomFull => session.zoom_to(Vec::new()),
                    Tool::ZoomIn => session.zoom_in(),
                    Tool::ZoomOut => session.zoom_out(),
                    Tool::FreeSpace => {
                        session.settings.show_free_space = !session.settings.show_free_space;
                    }
                    _ => {}
                }
                drop(session);
                if tool == Tool::FreeSpace {
                    self.save_settings();
                }
            }
        }
        self.ensure_polling();
        self.refresh();
    }

    /// Back, Forward, or up one level, from the keyboard, the mouse's side
    /// buttons, or the breadcrumb's arrows.
    fn navigate(&self, action: Nav) {
        if let Ok(mut session) = self.session.try_borrow_mut() {
            match action {
                Nav::Back => session.go_back(),
                Nav::Forward => session.go_forward(),
                Nav::Up => session.zoom_out(),
                Nav::Home => session.zoom_to(Vec::new()),
            }
        }
        self.refresh();
    }

    /// Move the selection to the neighbouring tile with an arrow key; with
    /// nothing selected on the map, start at its biggest tile.
    fn move_selection(&self, direction: Direction) {
        let next = (|| {
            let ui = self.ui.try_borrow().ok()?;
            let tiles = &ui.cache.as_ref()?.view.tiles;
            let session = self.session.try_borrow().ok()?;
            let zoom = session.zoom();
            let current = session
                .selected()
                .filter(|path| path.len() > zoom.len() && path.starts_with(zoom))
                .and_then(|path| treemap::find(tiles, &path[zoom.len()..]));
            let index = match current {
                Some(index) => treemap::neighbour(tiles, index, direction)?,
                None => {
                    tiles
                        .iter()
                        .enumerate()
                        .filter(|(_, tile)| tile.parent == NO_PARENT && tile.kind.is_node())
                        .max_by(|a, b| a.1.rect.area().total_cmp(&b.1.rect.area()))?
                        .0
                }
            };
            let mut path = zoom.to_vec();
            path.extend(treemap::path_of(tiles, index)?);
            Some(path)
        })();
        if let Some(path) = next
            && let Ok(mut session) = self.session.try_borrow_mut()
        {
            session.select(Some(path));
            if let Some(line) = session.selection_line() {
                session.set_status(line);
            }
        }
        self.refresh();
    }

    /// Open the selection with its associated program (or show it in
    /// Explorer). The shell call can show its own UI, so nothing is
    /// borrowed while it runs.
    fn open_selected(&self, reveal: bool) {
        let path = {
            let session = self.session.borrow();
            session.selected().and_then(|path| session.fs_path(path))
        };
        let status = match path {
            None => String::from("Nothing selected."),
            Some(path) => {
                let outcome = if reveal {
                    opener::reveal(&path)
                } else {
                    opener::open(&path)
                };
                match outcome {
                    Ok(()) if reveal => format!("Showed {} in Explorer.", path.display()),
                    Ok(()) => format!("Opened {}.", path.display()),
                    Err(err) => format!("Could not open {}: {err}", path.display()),
                }
            }
        };
        self.session.borrow_mut().set_status(status);
        self.refresh();
    }

    fn delete(&self) {
        let request = self.session.borrow_mut().delete_request();
        if let DeleteRequest::Confirm {
            node_path,
            fs_path,
            size,
            generation,
        } = request
        {
            let (yes, stop_asking) = self.confirm_delete(&fs_path, size);
            if yes {
                let mut session = self.session.borrow_mut();
                if stop_asking {
                    session.settings.confirm_before_delete = false;
                }
                session.delete_confirmed(&node_path, generation);
                drop(session);
                if stop_asking {
                    self.save_settings();
                }
            }
        }
        self.ensure_polling();
        self.refresh();
    }

    /// Ask before moving `path` to the Recycle Bin: (move, don't ask again).
    fn confirm_delete(&self, path: &Path, size: u64) -> (bool, bool) {
        let content = format!(
            "{}\n{} ({})\n\nYou can restore it from the Recycle Bin until the bin is emptied.",
            path.display(),
            pretty_bytes(size),
            exact_bytes(size)
        );
        let outcome = w::TaskDialogIndirect(&w::TASKDIALOGCONFIG {
            hwnd_parent: Some(self.wnd.hwnd()),
            flags: co::TDF::ALLOW_DIALOG_CANCELLATION | co::TDF::POSITION_RELATIVE_TO_WINDOW,
            common_buttons: co::TDCBF::CANCEL,
            window_title: Some("Delete"),
            main_icon: w::IconIdTd::Td(co::TD_ICON::WARNING),
            main_instruction: Some("Move this to the Recycle Bin?"),
            content: Some(&content),
            buttons: &[(BUTTON_MOVE, "Move to Recycle Bin")],
            default_button_id: BUTTON_MOVE,
            verification_text: Some("Don't ask again"),
            ..Default::default()
        });
        match outcome {
            Ok((button, _, stop_asking)) => (button.raw() == BUTTON_MOVE, stop_asking),
            Err(_) => (false, false),
        }
    }

    fn setup_menu(&self) {
        let settings = self.session.borrow().settings;
        let anchor = {
            let ui = self.ui.borrow();
            ui.slots
                .iter()
                .find(|slot| slot.tool == Tool::Setup)
                .map(|slot| w::POINT::with(slot.rect.left, slot.rect.bottom))
        };
        let Some(anchor) = anchor else {
            return;
        };
        let choice = popup(
            self.tools.hwnd(),
            anchor,
            &[
                (
                    SET_FREE_SPACE,
                    "Show free space on the map",
                    true,
                    Some(settings.show_free_space),
                ),
                (
                    SET_CONFIRM,
                    "Confirm before delete",
                    true,
                    Some(settings.confirm_before_delete),
                ),
                (
                    SET_BY_TYPE,
                    "Colour files by type (video, audio, images, …)",
                    true,
                    Some(settings.color_by_type),
                ),
                (
                    SET_CLASSIC_FONT,
                    "Classic map font (MS Sans Serif)",
                    true,
                    Some(settings.classic_font),
                ),
                (0, "", false, None),
                (
                    SET_DENSITY_DETAILED,
                    "Detailed map: every item down to 8 × 8 pixels",
                    true,
                    Some(settings.density == Density::Detailed),
                ),
                (
                    SET_DENSITY_BALANCED,
                    "Balanced map: small items grouped, every tile named",
                    true,
                    Some(settings.density == Density::Balanced),
                ),
                (
                    SET_DENSITY_SIMPLE,
                    "Simple map: fewer, larger tiles",
                    true,
                    Some(settings.density == Density::Simple),
                ),
                (0, "", false, None),
                (
                    SET_APPEARANCE_SYSTEM,
                    "Light or dark as Windows apps are set",
                    true,
                    Some(settings.appearance == Appearance::System),
                ),
                (
                    SET_APPEARANCE_LIGHT,
                    "Light",
                    true,
                    Some(settings.appearance == Appearance::Light),
                ),
                (
                    SET_APPEARANCE_DARK,
                    "Dark",
                    true,
                    Some(settings.appearance == Appearance::Dark),
                ),
            ],
        );
        if let Some(choice) = choice {
            let mut session = self.session.borrow_mut();
            let s = &mut session.settings;
            match choice {
                SET_FREE_SPACE => s.show_free_space = !s.show_free_space,
                SET_CONFIRM => s.confirm_before_delete = !s.confirm_before_delete,
                SET_BY_TYPE => s.color_by_type = !s.color_by_type,
                SET_CLASSIC_FONT => s.classic_font = !s.classic_font,
                SET_APPEARANCE_SYSTEM => s.appearance = Appearance::System,
                SET_APPEARANCE_LIGHT => s.appearance = Appearance::Light,
                SET_APPEARANCE_DARK => s.appearance = Appearance::Dark,
                SET_DENSITY_DETAILED => s.density = Density::Detailed,
                SET_DENSITY_BALANCED => s.density = Density::Balanced,
                SET_DENSITY_SIMPLE => s.density = Density::Simple,
                _ => return,
            }
            drop(session);
            self.save_settings();
            if matches!(
                choice,
                SET_APPEARANCE_SYSTEM | SET_APPEARANCE_LIGHT | SET_APPEARANCE_DARK
            ) {
                self.apply_theme();
            }
        }
        self.refresh();
    }

    /// Save at each change, including the toolbar and "Don't ask again",
    /// so closing the window during a scan loses no choices.
    fn save_settings(&self) {
        let settings = self.session.borrow().settings;
        let result = match &self.settings_file {
            Some(path) => settings.save(path),
            None => Err(std::io::Error::other(
                "the local app-data folder is unavailable",
            )),
        };
        if let Err(err) = result {
            self.session.borrow_mut().set_status(format!(
                "Could not save Setup choices: {err}. These choices apply to this launch only."
            ));
        }
    }

    /// The Help menu under its toolbar button: Using Topos and About.
    fn help_menu(&self) {
        let anchor = {
            let ui = self.ui.borrow();
            ui.slots
                .iter()
                .find(|slot| slot.tool == Tool::Help)
                .map(|slot| w::POINT::with(slot.rect.left, slot.rect.bottom))
        };
        let Some(anchor) = anchor else {
            return;
        };
        let choice = popup(
            self.tools.hwnd(),
            anchor,
            &[
                (CMD_HELP, "Using Topos\tF1", true, None),
                (0, "", false, None),
                (CMD_ABOUT, "About Topos", true, None),
            ],
        );
        match choice {
            Some(CMD_HELP) => self.show_page(pages::Kind::Help),
            Some(CMD_ABOUT) => self.show_page(pages::Kind::About),
            _ => {}
        }
    }

    fn show_page(&self, kind: pages::Kind) {
        let (palette, dpi) = {
            let ui = self.ui.borrow();
            (ui.palette, ui.dpi)
        };
        pages::show(kind, &self.wnd, palette, dpi, self.data_dir.as_deref());
        let _ = self.map.hwnd().SetFocus();
    }

    // ---- map -----------------------------------------------------------

    fn map_events(&self) {
        let app = self.clone();
        self.map.on().wm_paint(move || {
            app.paint_map();
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_size(move |_| {
            let _ = app.map.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_mouse_move(move |p| {
            app.map_mouse_move(p.coords);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_mouse_leave(move || {
            if let Ok(mut ui) = app.ui.try_borrow_mut() {
                ui.map_tracking = false;
                ui.mouse = None;
                ui.hover = None;
                ui.tip = None;
                ui.target = None;
                ui.hint = None;
            }
            let _ = app.map.hwnd().KillTimer(TIP_TIMER);
            let _ = app.map.hwnd().InvalidateRect(None, false);
            let _ = app.status.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        // The wheel zooms like a map: up goes one level into the folder
        // under the pointer, down goes up a level. Its position comes in
        // screen coordinates.
        let app = self.clone();
        self.map.on().wm(co::WM::MOUSEWHEEL, move |p| {
            let delta = i32::from((p.wparam >> 16) as u16 as i16);
            let screen = w::POINT::with(
                i32::from(p.lparam as u16 as i16),
                i32::from((p.lparam >> 16) as u16 as i16),
            );
            let at = app.map.hwnd().ScreenToClient(screen).ok();
            app.wheel(delta, at);
            Ok(0)
        });
        let app = self.clone();
        self.map.on().wm_timer(TIP_TIMER, move || {
            let _ = app.map.hwnd().KillTimer(TIP_TIMER);
            if let Ok(mut ui) = app.ui.try_borrow_mut()
                && ui.hover.is_some()
            {
                ui.tip = ui.mouse;
            }
            let _ = app.map.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_l_button_down(move |p| {
            let _ = app.map.hwnd().SetFocus();
            app.select_at(p.coords);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_r_button_down(move |p| {
            let _ = app.map.hwnd().SetFocus();
            app.select_at(p.coords);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_l_button_dbl_clk(move |p| {
            app.zoom_at(p.coords);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_x_button_down(move |p| {
            // The mouse's side buttons walk the history, as in a browser.
            if p.vkey_code.has(co::MK::XBUTTON1) {
                app.navigate(Nav::Back);
            } else if p.vkey_code.has(co::MK::XBUTTON2) {
                app.navigate(Nav::Forward);
            }
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_key_down(move |p| {
            app.map_key(p.vkey_code);
            Ok(())
        });
        let app = self.clone();
        self.map.on().wm_context_menu(move |p| {
            app.map_menu(p.cursor_pos);
            Ok(())
        });
    }

    /// The index path (from the scan root) of the node under a map point.
    fn node_at(&self, at: w::POINT) -> Option<Vec<usize>> {
        self.map_path(at, treemap::path_of)
    }

    /// The index path (from the scan root) of the folder a double-click at a
    /// map point opens.
    fn folder_at(&self, at: w::POINT) -> Option<Vec<usize>> {
        self.map_path(at, treemap::folder_of)
    }

    fn map_path(
        &self,
        at: w::POINT,
        relative: fn(&[treemap::Tile], usize) -> Option<Vec<usize>>,
    ) -> Option<Vec<usize>> {
        let ui = self.ui.try_borrow().ok()?;
        let tiles = &ui.cache.as_ref()?.view.tiles;
        let index = treemap::hit(tiles, at.x as f32, at.y as f32)?;
        let relative = relative(tiles, index)?;
        let session = self.session.try_borrow().ok()?;
        let mut path = session.zoom().to_vec();
        path.extend(relative);
        Some(path)
    }

    /// Whole wheel notches: up zooms one level toward the tile at `at`,
    /// down zooms out one level.
    fn wheel(&self, delta: i32, at: Option<w::POINT>) {
        let notches = {
            let Ok(mut ui) = self.ui.try_borrow_mut() else {
                return;
            };
            ui.wheel += delta;
            let notches = ui.wheel / WHEEL_NOTCH;
            ui.wheel -= notches * WHEEL_NOTCH;
            notches
        };
        for _ in 0..notches.unsigned_abs() {
            if notches > 0 {
                let Some(path) = at.and_then(|at| self.folder_at(at)) else {
                    break;
                };
                if let Ok(mut session) = self.session.try_borrow_mut() {
                    let relative = path[session.zoom().len().min(path.len())..].to_vec();
                    session.zoom_toward(&relative);
                }
            } else if let Ok(mut session) = self.session.try_borrow_mut() {
                session.zoom_out();
            }
            // Lay the new level out before the next notch looks at it.
            self.refresh();
            let _ = self.map.hwnd().UpdateWindow();
        }
        if let Some(at) = at {
            self.map_mouse_move(at);
        }
    }

    fn select_at(&self, at: w::POINT) {
        let path = self.node_at(at);
        if let Ok(mut session) = self.session.try_borrow_mut() {
            session.select(path);
        }
        self.refresh();
    }

    /// Double-click: a folder zooms in; a file or a group of small files
    /// zooms to the folder holding it. Back returns.
    fn zoom_at(&self, at: w::POINT) {
        let Some(path) = self.folder_at(at) else {
            return;
        };
        if let Ok(mut session) = self.session.try_borrow_mut()
            && path.len() > session.zoom().len()
        {
            session.zoom_to(path);
        }
        self.refresh();
        self.map_mouse_move(at);
    }

    fn map_key(&self, key: co::VK) {
        match key {
            co::VK::BACK => self.run_tool(Tool::ZoomOut),
            co::VK::RETURN => self.run_tool(Tool::ZoomIn),
            co::VK::LEFT => self.move_selection(Direction::Left),
            co::VK::RIGHT => self.move_selection(Direction::Right),
            co::VK::UP => self.move_selection(Direction::Up),
            co::VK::DOWN => self.move_selection(Direction::Down),
            co::VK::HOME => self.run_tool(Tool::ZoomFull),
            co::VK::DELETE => self.delete(),
            co::VK::F5 => {
                if let Ok(mut session) = self.session.try_borrow_mut() {
                    session.reload();
                }
                self.ensure_polling();
                self.refresh();
            }
            co::VK::ESCAPE => {
                if let Ok(mut session) = self.session.try_borrow_mut() {
                    if session.is_scanning() {
                        session.stop_scan();
                    } else {
                        session.select(None);
                    }
                }
                self.refresh();
            }
            _ => {}
        }
    }

    /// Right-click menu, at the pointer (or the map's corner when opened from
    /// the keyboard, which reports -1, -1).
    fn map_menu(&self, screen: w::POINT) {
        let (selected, can_in, can_out, can_delete) = {
            let session = self.session.borrow();
            let selected = session.selected().is_some();
            (
                selected,
                session.can_zoom_in(),
                !session.zoom().is_empty(),
                session.can_delete(),
            )
        };
        let at = if screen.x == -1 && screen.y == -1 {
            w::POINT::with(8, 8)
        } else {
            self.map
                .hwnd()
                .ScreenToClient(screen)
                .unwrap_or(w::POINT::with(8, 8))
        };
        let choice = popup(
            self.map.hwnd(),
            at,
            &[
                (CMD_OPEN, "Run or Open", selected, None),
                (CMD_REVEAL, "Show in Explorer", selected, None),
                (0, "", false, None),
                (CMD_ZOOM_IN, "Zoom In", can_in, None),
                (CMD_ZOOM_OUT, "Zoom Out", can_out, None),
                (0, "", false, None),
                (CMD_DELETE, "Delete to Recycle Bin…", can_delete, None),
            ],
        );
        match choice {
            Some(CMD_OPEN) => self.open_selected(false),
            Some(CMD_REVEAL) => self.open_selected(true),
            Some(CMD_ZOOM_IN) => self.run_tool(Tool::ZoomIn),
            Some(CMD_ZOOM_OUT) => self.run_tool(Tool::ZoomOut),
            Some(CMD_DELETE) => self.delete(),
            _ => {}
        }
    }

    fn map_mouse_move(&self, at: w::POINT) {
        let Ok(mut ui) = self.ui.try_borrow_mut() else {
            return;
        };
        if !ui.map_tracking {
            ui.map_tracking = track_leave(self.wnd.hwnd(), self.map.ctrl_id());
        }
        ui.mouse = Some((at.x, at.y));
        let hover = ui
            .cache
            .as_ref()
            .and_then(|cache| treemap::hit(&cache.view.tiles, at.x as f32, at.y as f32));
        if hover != ui.hover {
            ui.hover = hover;
            ui.tip = None;
            // What a double-click here would open: outlined on the map and
            // named in the status row.
            let open = hover.and_then(|index| {
                let tiles = &ui.cache.as_ref()?.view.tiles;
                treemap::folder_of(tiles, index)
            });
            ui.target = open.as_ref().and_then(|open| {
                let tiles = &ui.cache.as_ref()?.view.tiles;
                treemap::find(tiles, open)
            });
            ui.hint = open.and_then(|open| {
                let session = self.session.try_borrow().ok()?;
                session.zoom_hint(&open)
            });
            drop(ui);
            let hwnd = self.map.hwnd();
            let _ = hwnd.KillTimer(TIP_TIMER);
            if hover.is_some() {
                let _ = hwnd.SetTimer(TIP_TIMER, TIP_DELAY_MS, None);
            }
            let _ = hwnd.InvalidateRect(None, false);
            let _ = self.status.hwnd().InvalidateRect(None, false);
        }
    }

    fn paint_map(&self) {
        let hwnd = self.map.hwnd();
        let Ok(hdc) = hwnd.BeginPaint() else {
            return;
        };
        // Borrowed elsewhere means a dialog or menu loop is painting under
        // a command; the command refreshes the map when it finishes.
        let (Ok(session), Ok(mut ui)) = (self.session.try_borrow(), self.ui.try_borrow_mut())
        else {
            return;
        };
        let _ = paint_map(&hdc, hwnd, &session, &mut ui);
    }

    // ---- toolbar -------------------------------------------------------

    fn tools_events(&self) {
        let app = self.clone();
        self.tools.on().wm_paint(move || {
            app.paint_tools();
            Ok(())
        });
        let app = self.clone();
        self.tools.on().wm_mouse_move(move |p| {
            let Ok(mut ui) = app.ui.try_borrow_mut() else {
                return Ok(());
            };
            if !ui.tools_tracking {
                ui.tools_tracking = track_leave(app.wnd.hwnd(), app.tools.ctrl_id());
            }
            let hover = toolbar::hit(&ui.slots, p.coords.x, p.coords.y);
            if hover != ui.pointer.hover {
                ui.pointer.hover = hover;
                drop(ui);
                let _ = app.tools.hwnd().InvalidateRect(None, false);
            }
            Ok(())
        });
        let app = self.clone();
        self.tools.on().wm_mouse_leave(move || {
            if let Ok(mut ui) = app.ui.try_borrow_mut() {
                ui.tools_tracking = false;
                ui.pointer = Pointer::default();
            }
            let _ = app.tools.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.tools.on().wm_l_button_down(move |p| {
            if let Ok(mut ui) = app.ui.try_borrow_mut() {
                ui.pointer.pressed = toolbar::hit(&ui.slots, p.coords.x, p.coords.y);
            }
            let _ = app.tools.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.tools.on().wm_l_button_up(move |p| {
            let clicked = match app.ui.try_borrow_mut() {
                Ok(mut ui) => {
                    let pressed = ui.pointer.pressed.take();
                    let here = toolbar::hit(&ui.slots, p.coords.x, p.coords.y);
                    pressed.filter(|tool| Some(*tool) == here)
                }
                Err(_) => None,
            };
            let _ = app.tools.hwnd().InvalidateRect(None, false);
            if let Some(tool) = clicked {
                let enabled = app
                    .session
                    .try_borrow()
                    .is_ok_and(|session| toolbar::face(tool, &session).enabled);
                if enabled {
                    app.run_tool(tool);
                }
            }
            Ok(())
        });
    }

    fn paint_tools(&self) {
        let hwnd = self.tools.hwnd();
        let Ok(hdc) = hwnd.BeginPaint() else {
            return;
        };
        let (Ok(session), Ok(mut ui)) = (self.session.try_borrow(), self.ui.try_borrow_mut())
        else {
            return;
        };
        let _ = buffered(&hdc, hwnd, |dc, rc| {
            let ui = &mut *ui;
            gdi::fill(dc, rc, ui.palette.face);
            if let Ok(_font) = dc.SelectObject(&*ui.fonts.ui) {
                ui.slots = toolbar::place(rc.bottom, ui.dpi, &session, |label| {
                    dc.GetTextExtentPoint32(label).map_or(0, |size| size.cx)
                });
            }
            toolbar::paint(
                dc, &ui.fonts, ui.dpi, &ui.slots, &session, ui.pointer, ui.palette,
            );
        });
    }

    // ---- breadcrumb ------------------------------------------------------

    fn crumbs_events(&self) {
        let app = self.clone();
        self.crumbs.on().wm_paint(move || {
            app.paint_crumbs();
            Ok(())
        });
        let app = self.clone();
        self.crumbs.on().wm_mouse_move(move |p| {
            let Ok(mut ui) = app.ui.try_borrow_mut() else {
                return Ok(());
            };
            if !ui.crumbs_tracking {
                ui.crumbs_tracking = track_leave(app.wnd.hwnd(), app.crumbs.ctrl_id());
            }
            let hover = crumb_at(&ui.crumbs, p.coords);
            if hover != ui.crumb_hover {
                ui.crumb_hover = hover;
                // The buttons have no labels: the status row says where each
                // one goes (and the keys that do the same).
                let crumb = hover.and_then(|index| ui.crumbs.get(index).map(|c| c.1.clone()));
                ui.hint = crumb.and_then(|crumb| app.crumb_hint(&crumb));
                drop(ui);
                let _ = app.crumbs.hwnd().InvalidateRect(None, false);
                let _ = app.status.hwnd().InvalidateRect(None, false);
            }
            Ok(())
        });
        let app = self.clone();
        self.crumbs.on().wm_mouse_leave(move || {
            if let Ok(mut ui) = app.ui.try_borrow_mut() {
                ui.crumbs_tracking = false;
                ui.crumb_hover = None;
                ui.crumb_pressed = None;
                ui.hint = None;
            }
            let _ = app.crumbs.hwnd().InvalidateRect(None, false);
            let _ = app.status.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.crumbs.on().wm_l_button_down(move |p| {
            if let Ok(mut ui) = app.ui.try_borrow_mut() {
                ui.crumb_pressed = crumb_at(&ui.crumbs, p.coords);
            }
            let _ = app.crumbs.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.crumbs.on().wm_l_button_up(move |p| {
            let target = app.ui.try_borrow_mut().ok().and_then(|mut ui| {
                ui.crumb_pressed = None;
                crumb_at(&ui.crumbs, p.coords)
                    .and_then(|index| ui.crumbs.get(index).map(|c| c.1.clone()))
            });
            let _ = app.crumbs.hwnd().InvalidateRect(None, false);
            match target {
                Some(Crumb::Back) => app.navigate(Nav::Back),
                Some(Crumb::Forward) => app.navigate(Nav::Forward),
                Some(Crumb::Home) => app.navigate(Nav::Home),
                Some(Crumb::Zoom(path)) => {
                    if let Ok(mut session) = app.session.try_borrow_mut() {
                        session.zoom_to(path);
                    }
                    app.refresh();
                }
                None => {}
            }
            Ok(())
        });
    }

    /// The status line for a breadcrumb button or link under the pointer.
    fn crumb_hint(&self, crumb: &Crumb) -> Option<String> {
        let session = self.session.try_borrow().ok()?;
        Some(match crumb {
            Crumb::Back => format!(
                "Back to {}  ·  Alt+Left or mouse Back",
                session.folder_label(session.back_target()?)?
            ),
            Crumb::Forward => format!(
                "Forward to {}  ·  Alt+Right or mouse Forward",
                session.folder_label(session.forward_target()?)?
            ),
            Crumb::Home => format!("Show all of {}  ·  Home key", session.folder_label(&[])?),
            Crumb::Zoom(path) => format!("Open {}", session.folder_label(path)?),
        })
    }

    fn paint_crumbs(&self) {
        let hwnd = self.crumbs.hwnd();
        let Ok(hdc) = hwnd.BeginPaint() else {
            return;
        };
        let (Ok(session), Ok(mut ui)) = (self.session.try_borrow(), self.ui.try_borrow_mut())
        else {
            return;
        };
        let _ = buffered(&hdc, hwnd, |dc, rc| {
            let ui = &mut *ui;
            gdi::fill(dc, rc, ui.palette.face);
            // The row's bottom edge: a hairline, or while Topos is scanning
            // or deleting, a progress bar across the whole window.
            let thickness = if session.activity().is_some() {
                ui.px(3)
            } else {
                1
            };
            let edge = w::RECT {
                top: rc.bottom - thickness,
                ..rc
            };
            gdi::fill(dc, edge, ui.palette.rule);
            if let Some(activity) = session.activity() {
                paint_progress(dc, edge, ui, activity);
            }
            ui.crumbs = paint_crumbs(dc, rc, ui, &session);
        });
    }

    // ---- drive field ---------------------------------------------------

    fn drives_events(&self) {
        let app = self.clone();
        self.drives.on().wm_paint(move || {
            app.paint_drives();
            Ok(())
        });
        let app = self.clone();
        self.drives.on().wm_mouse_move(move |_| {
            let Ok(mut ui) = app.ui.try_borrow_mut() else {
                return Ok(());
            };
            if !ui.drives_tracking {
                ui.drives_tracking = track_leave(app.wnd.hwnd(), app.drives.ctrl_id());
            }
            if !ui.drives_hover {
                ui.drives_hover = true;
                drop(ui);
                let _ = app.drives.hwnd().InvalidateRect(None, false);
            }
            Ok(())
        });
        let app = self.clone();
        self.drives.on().wm_mouse_leave(move || {
            if let Ok(mut ui) = app.ui.try_borrow_mut() {
                ui.drives_tracking = false;
                ui.drives_hover = false;
            }
            let _ = app.drives.hwnd().InvalidateRect(None, false);
            Ok(())
        });
        let app = self.clone();
        self.drives.on().wm_l_button_down(move |_| {
            app.drive_menu();
            Ok(())
        });
    }

    /// The drive field: the current drive or folder in an input-like box
    /// with a drop-down chevron.
    fn paint_drives(&self) {
        let hwnd = self.drives.hwnd();
        let Ok(hdc) = hwnd.BeginPaint() else {
            return;
        };
        let Ok(ui) = self.ui.try_borrow() else {
            return;
        };
        let _ = buffered(&hdc, hwnd, |dc, client| {
            let palette = ui.palette;
            gdi::fill(dc, client, palette.face);
            let rc = drive_field(client, &ui);
            gdi::fill(dc, rc, palette.field);
            let edge = if ui.drives_hover {
                palette.down_edge
            } else {
                palette.field_edge
            };
            gdi::outline(dc, rc, 1, edge);
            let _ = dc.SetBkMode(co::BKMODE::TRANSPARENT);
            let _ = dc.SetTextColor(gdi::colorref(palette.text));
            let chevron = ui.px(22);
            let text = ui
                .shown
                .drive_index
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| ui.shown.drives.get(index))
                .map_or("No drive", String::as_str);
            let single = co::DT::SINGLELINE | co::DT::VCENTER;
            if let Ok(_font) = dc.SelectObject(&*ui.fonts.ui) {
                gdi::text(
                    dc,
                    text,
                    w::RECT {
                        left: rc.left + ui.px(6),
                        right: rc.right - chevron,
                        ..rc
                    },
                    co::DT::LEFT | single | co::DT::END_ELLIPSIS,
                );
            }
            if let Ok(_font) = dc.SelectObject(&*ui.fonts.chevron) {
                // Segoe MDL2 Assets: ChevronDown.
                gdi::text(
                    dc,
                    "\u{E70D}",
                    w::RECT {
                        left: rc.right - chevron,
                        ..rc
                    },
                    co::DT::CENTER | single,
                );
            }
        });
    }

    // ---- status row ----------------------------------------------------

    fn status_events(&self) {
        let app = self.clone();
        self.status.on().wm_paint(move || {
            app.paint_status();
            Ok(())
        });
    }

    /// The status row: what is happening on the left, the scan's totals in
    /// a fixed column on the right, a hairline above both.
    fn paint_status(&self) {
        let hwnd = self.status.hwnd();
        let Ok(hdc) = hwnd.BeginPaint() else {
            return;
        };
        let Ok(ui) = self.ui.try_borrow() else {
            return;
        };
        let _ = buffered(&hdc, hwnd, |dc, rc| {
            let palette = ui.palette;
            gdi::fill(dc, rc, palette.face);
            gdi::fill(
                dc,
                w::RECT {
                    bottom: rc.top + 1,
                    ..rc
                },
                palette.rule,
            );
            let pad = ui.px(8);
            let totals_w = ui.px(380).min(rc.right / 2);
            let split = rc.right - totals_w;
            gdi::fill(
                dc,
                w::RECT {
                    left: split,
                    right: split + 1,
                    top: rc.top + ui.px(4),
                    bottom: rc.bottom - ui.px(4),
                },
                palette.rule,
            );
            let _ = dc.SetBkMode(co::BKMODE::TRANSPARENT);
            let _ = dc.SetTextColor(gdi::colorref(palette.text));
            let single = co::DT::SINGLELINE | co::DT::VCENTER | co::DT::END_ELLIPSIS;
            if let Ok(_font) = dc.SelectObject(&*ui.fonts.ui) {
                let row = w::RECT {
                    top: rc.top + 1,
                    ..rc
                };
                // While the pointer is on the map, the row says what a
                // double-click and the wheel would open there.
                let left = ui.hint.as_deref().unwrap_or(&ui.shown.status[0]);
                gdi::text(
                    dc,
                    left,
                    w::RECT {
                        left: pad,
                        right: split - pad,
                        ..row
                    },
                    co::DT::LEFT | single,
                );
                gdi::text(
                    dc,
                    &ui.shown.status[1],
                    w::RECT {
                        left: split + pad,
                        right: rc.right - pad,
                        ..row
                    },
                    co::DT::LEFT | single,
                );
            }
        });
    }
}

/// The drive list's items (drives with free space, plus the scanned folder
/// when it is not a drive) and which one is current.
fn drive_items(session: &Session) -> (Vec<String>, Option<u32>) {
    let mut items: Vec<String> = session
        .drives()
        .iter()
        .map(|drive| {
            format!(
                "{}   {} free of {}",
                drive.mount,
                pretty_bytes(drive.free_bytes),
                pretty_bytes(drive.total_bytes)
            )
        })
        .collect();
    let target = session.target();
    let mut index = target.and_then(|target| {
        session
            .drives()
            .iter()
            .position(|drive| Path::new(&drive.mount) == target)
    });
    // A folder from the command line: name it (the breadcrumb shows its
    // whole path; a narrow list would cut the path off).
    if index.is_none()
        && let Some(target) = target
    {
        index = Some(items.len());
        let name = target.file_name().map_or_else(
            || target.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        items.push(format!("Folder: {name}"));
    }
    (items, index.and_then(|index| u32::try_from(index).ok()))
}

/// Whether Windows apps are set to dark (`AppsUseLightTheme` is 0). A
/// missing or unreadable value means light, the Windows default.
fn system_wants_dark() -> bool {
    let value = w::HKEY::CURRENT_USER.RegGetValue(
        Some(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        Some("AppsUseLightTheme"),
        co::RRF::RT_REG_DWORD,
    );
    matches!(value, Ok(w::RegistryValue::Dword(0)))
}

/// A step through where the map has been.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nav {
    Back,
    Forward,
    Up,
    /// The whole scanned drive or folder.
    Home,
}

/// Explorer's history keys — Alt+Left (Back), Alt+Right (Forward), Alt+Up
/// (up one level) — and F1 for help. An accelerator table turns them into
/// commands before the map sees them, so Alt with any other key (Alt+F4)
/// keeps its default.
fn accelerators() -> Option<w::guard::DestroyAcceleratorTableGuard> {
    let alt = co::ACCELF::VIRTKEY | co::ACCELF::ALT;
    w::HACCEL::CreateAcceleratorTable(&[
        w::ACCEL {
            fVirt: alt,
            key: co::VK::LEFT,
            cmd: ACC_BACK,
        },
        w::ACCEL {
            fVirt: alt,
            key: co::VK::RIGHT,
            cmd: ACC_FORWARD,
        },
        w::ACCEL {
            fVirt: alt,
            key: co::VK::UP,
            cmd: ACC_UP,
        },
        w::ACCEL {
            fVirt: co::ACCELF::VIRTKEY,
            key: co::VK::F1,
            cmd: ACC_HELP,
        },
    ])
    .ok()
}

/// The drive field inside the drive control: 220 pixels (at 96 dpi) wide
/// from a small margin, 24 high, centred in the toolbar row.
fn drive_field(client: w::RECT, ui: &Ui) -> w::RECT {
    let height = ui.px(24);
    let top = ((client.bottom - height) / 2).max(0);
    w::RECT {
        left: ui.px(6),
        top,
        right: ui.px(6) + ui.px(220),
        bottom: top + height,
    }
}

/// Ask for a `WM_MOUSELEAVE` when the pointer leaves the child `ctrl_id` of
/// `parent`. Returns whether tracking started.
fn track_leave(parent: &w::HWND, ctrl_id: u16) -> bool {
    let Ok(hwnd) = parent.GetDlgItem(ctrl_id) else {
        return false;
    };
    let mut tme = w::TRACKMOUSEEVENT::default();
    tme.dwFlags = co::TME::LEAVE;
    tme.hwndTrack = hwnd;
    w::TrackMouseEvent(&mut tme).is_ok()
}

/// Show a popup menu at `at` (client coordinates of `hwnd`) and return the
/// chosen command. Items are (id, text, enabled, checked); id 0 is a
/// separator. The menu runs its own message loop: call with nothing
/// borrowed.
fn popup(hwnd: &w::HWND, at: w::POINT, items: &[(u16, &str, bool, Option<bool>)]) -> Option<u16> {
    let mut menu = w::HMENU::CreatePopupMenu().ok()?;
    for &(id, text, enabled, checked) in items {
        let item = if id == 0 {
            w::MenuItem::Separator
        } else {
            w::MenuItem::Entry { cmd_id: id, text }
        };
        let _ = menu.append_item(&[item]);
        if id != 0 {
            let _ = menu.EnableMenuItem(w::IdPos::Id(id), enabled);
            if let Some(checked) = checked {
                let _ = menu.CheckMenuItem(w::IdPos::Id(id), checked);
            }
        }
    }
    let screen = hwnd.ClientToScreen(at).ok();
    let choice = screen.and_then(|screen| {
        menu.TrackPopupMenu(co::TPM::RETURNCMD | co::TPM::RIGHTBUTTON, screen, hwnd)
            .ok()
            .flatten()
    });
    let _ = menu.DestroyMenu();
    choice.and_then(|id| u16::try_from(id).ok())
}

/// Paint through a back buffer the size of `hwnd`'s client area, then copy
/// it to `hdc` in one go (no flicker).
fn buffered(hdc: &w::HDC, hwnd: &w::HWND, draw: impl FnOnce(&w::HDC, w::RECT)) -> w::SysResult<()> {
    let rc = hwnd.GetClientRect()?;
    if rc.right <= 0 || rc.bottom <= 0 {
        return Ok(());
    }
    let size = w::SIZE::with(rc.right, rc.bottom);
    let mem = hdc.CreateCompatibleDC()?;
    let back = hdc.CreateCompatibleBitmap(size.cx, size.cy)?;
    let _back = mem.SelectObject(&*back)?;
    draw(&mem, rc);
    hdc.BitBlt(
        w::POINT::default(),
        size,
        &mem,
        w::POINT::default(),
        co::ROP::SRCCOPY,
    )
}

fn crumb_at(crumbs: &[(w::RECT, Crumb)], at: w::POINT) -> Option<usize> {
    crumbs.iter().position(|(rc, _)| {
        at.x >= rc.left && at.x < rc.right && at.y >= rc.top && at.y < rc.bottom
    })
}

/// Draw the breadcrumb row: Back and Forward arrows, the ancestors as
/// links, the zoomed folder in bold with its size, and — when files are
/// coloured by type — the colour legend at the right end. Returns where each
/// link and arrow is.
fn paint_crumbs(dc: &w::HDC, rc: w::RECT, ui: &Ui, session: &Session) -> Vec<(w::RECT, Crumb)> {
    let _ = dc.SetBkMode(co::BKMODE::TRANSPARENT);
    let palette = ui.palette;
    let mut links = Vec::new();
    let mut x = ui.px(4);
    // Back, Forward, and Home: Segoe MDL2 Assets symbols at the toolbar's
    // size, on buttons the height of the row; grey when there is nowhere to
    // go.
    let size = ui.px(30).min(rc.bottom - rc.top - ui.px(2));
    let top = rc.top + (rc.bottom - 1 - rc.top - size) / 2;
    for (glyph, crumb, enabled) in [
        ('\u{E72B}', Crumb::Back, session.can_go_back()),
        ('\u{E72A}', Crumb::Forward, session.can_go_forward()),
        ('\u{E80F}', Crumb::Home, !session.zoom().is_empty()),
    ] {
        let cell = w::RECT {
            left: x,
            top,
            right: x + size,
            bottom: top + size,
        };
        let index = links.len();
        let pressed = enabled && ui.crumb_pressed == Some(index);
        let hovered = enabled && ui.crumb_hover == Some(index);
        if pressed {
            gdi::fill(dc, cell, palette.down_fill);
            gdi::outline(dc, cell, 1, palette.down_edge);
        } else if hovered {
            gdi::fill(dc, cell, palette.hover_fill);
            gdi::outline(dc, cell, 1, palette.hover_edge);
        }
        if let Ok(_font) = dc.SelectObject(&*ui.fonts.icons) {
            let ink = if enabled {
                palette.text
            } else {
                palette.disabled
            };
            let _ = dc.SetTextColor(gdi::colorref(ink));
            gdi::text(
                dc,
                &glyph.to_string(),
                cell,
                co::DT::CENTER | co::DT::VCENTER | co::DT::SINGLELINE,
            );
        }
        if enabled {
            links.push((cell, crumb));
        }
        x = cell.right + ui.px(2);
    }
    // A hairline between the buttons and the path.
    x += ui.px(4);
    gdi::fill(
        dc,
        w::RECT {
            left: x,
            top: rc.top + ui.px(7),
            right: x + 1,
            bottom: rc.bottom - ui.px(7),
        },
        palette.rule,
    );
    x += ui.px(10);
    let row = w::RECT {
        bottom: rc.bottom - 1,
        ..rc
    };
    let draw = |text: &str, font: &w::HFONT, color: Rgb, x: &mut i32| -> w::RECT {
        let Ok(_font) = dc.SelectObject(font) else {
            return w::RECT::default();
        };
        let width = dc.GetTextExtentPoint32(text).map_or(0, |size| size.cx);
        let cell = w::RECT {
            left: *x,
            right: (*x + width).min(rc.right),
            ..row
        };
        let _ = dc.SetTextColor(gdi::colorref(color));
        gdi::text(
            dc,
            text,
            cell,
            co::DT::LEFT | co::DT::VCENTER | co::DT::SINGLELINE,
        );
        *x += width;
        cell
    };
    let crumbs = session.breadcrumb();
    if crumbs.is_empty() {
        let text = session.target().map_or_else(
            || String::from("No drive"),
            |target| target.display().to_string(),
        );
        draw(&text, &ui.fonts.ui, palette.muted, &mut x);
        return links;
    }
    let last = crumbs.len() - 1;
    for (index, (name, path)) in crumbs.into_iter().enumerate() {
        if index > 0 {
            draw("  ›  ", &ui.fonts.ui, palette.muted, &mut x);
        }
        if index == last {
            draw(&name, &ui.fonts.ui_bold, palette.text, &mut x);
        } else {
            let font = if ui.crumb_hover == Some(links.len()) {
                &ui.fonts.link
            } else {
                &ui.fonts.ui
            };
            let cell = draw(&name, font, palette.link, &mut x);
            links.push((cell, Crumb::Zoom(path)));
        }
    }
    if let Some(node) = session
        .scanned()
        .and_then(|scanned| scanned.tree.node_at(session.zoom()))
    {
        let text = format!(
            "   {}  ·  {}",
            pretty_bytes(node.size()),
            exact_bytes(node.size())
        );
        draw(&text, &ui.fonts.ui, palette.muted, &mut x);
    }
    if let Some(when) = session.stale_since().and_then(format::local_time) {
        let note = if session.is_scanning() {
            format!("   ·   last scan, from {when}; rescanning")
        } else {
            format!("   ·   last scan, from {when}")
        };
        draw(&note, &ui.fonts.ui, palette.stale, &mut x);
    }
    if session.settings.color_by_type {
        draw_legend(dc, rc, x, ui);
    }
    links
}

/// The colours by file type, right-aligned in the breadcrumb row: a swatch
/// and a name each. Left out when the row has no room after the crumbs.
fn draw_legend(dc: &w::HDC, rc: w::RECT, used: i32, ui: &Ui) {
    let Ok(_font) = dc.SelectObject(&*ui.fonts.ui) else {
        return;
    };
    let (swatch, gap, spacing) = (ui.px(10), ui.px(4), ui.px(12));
    let entries: Vec<(Rgb, &str, i32)> = FileType::ALL
        .into_iter()
        .map(|kind| {
            let label = kind.label();
            let width = dc.GetTextExtentPoint32(label).map_or(0, |size| size.cx);
            (ui.palette.types[kind as usize], label, width)
        })
        .collect();
    let total: i32 = entries
        .iter()
        .map(|(_, _, width)| swatch + gap + width + spacing)
        .sum();
    let mut x = rc.right - ui.px(8) - total + spacing;
    if x < used + ui.px(24) {
        return;
    }
    let middle = (rc.top + rc.bottom - 1) / 2;
    let _ = dc.SetTextColor(gdi::colorref(ui.palette.muted));
    for (color, label, width) in entries {
        let square = w::RECT {
            left: x,
            top: middle - swatch / 2,
            right: x + swatch,
            bottom: middle - swatch / 2 + swatch,
        };
        gdi::fill(dc, square, color);
        gdi::outline(dc, square, 1, ui.palette.border);
        x += swatch + gap;
        gdi::text(
            dc,
            label,
            w::RECT {
                left: x,
                right: x + width,
                bottom: rc.bottom - 1,
                ..rc
            },
            co::DT::LEFT | co::DT::VCENTER | co::DT::SINGLELINE,
        );
        x += width + spacing;
    }
}

/// Paint the map: the cached tiles and captions, then the selection,
/// hover outline, and tooltip over them; or the scan progress before a map
/// exists.
fn paint_map(hdc: &w::HDC, hwnd: &w::HWND, session: &Session, ui: &mut Ui) -> w::SysResult<()> {
    let rc = hwnd.GetClientRect()?;
    let (Ok(width), Ok(height)) = (u32::try_from(rc.right), u32::try_from(rc.bottom)) else {
        return Ok(());
    };
    if width == 0 || height == 0 {
        ui.map_back_buffer = None;
        return Ok(());
    }
    let classic = session.settings.classic_font;
    let line = if classic {
        ui.line_classic
    } else {
        ui.line_modern
    };
    let key = session.view_key(width, height, ui.scale(), line);
    let stale = match (&ui.cache, &key) {
        (Some(cache), Some(key)) => cache.view.key != *key || cache.classic_font != classic,
        _ => true,
    };
    if stale {
        ui.cache = None;
        ui.hover = None;
        ui.tip = None;
        if let Some(key) = key
            && let Some((view, frame)) = session.build_view(key)
        {
            let bitmap = gdi::frame_bitmap(hdc, &frame)?;
            drop(frame);
            let font = if classic {
                &ui.fonts.map_classic
            } else {
                &ui.fonts.map_modern
            };
            draw_captions(hdc, &bitmap, &view, font, ui.palette.caption)?;
            if let Some((x, y)) = ui.mouse {
                ui.hover = treemap::hit(&view.tiles, x as f32, y as f32);
            }
            ui.cache = Some(MapCache {
                view,
                bitmap,
                classic_font: classic,
            });
        }
    }

    // Take the buffer out while the paint closure reads the rest of Ui.
    let mut buffer = ui.map_back_buffer.take();
    let result = gdi::paint_buffered(hdc, w::SIZE::with(rc.right, rc.bottom), &mut buffer, |dc| {
        let palette = ui.palette;
        let Some(cache) = &ui.cache else {
            gdi::fill(dc, rc, palette.canvas);
            let text = session
                .progress_line()
                .unwrap_or_else(|| session.status().to_owned());
            let _ = dc.SetBkMode(co::BKMODE::TRANSPARENT);
            let _ = dc.SetTextColor(gdi::colorref(palette.caption));
            if let Ok(_font) = dc.SelectObject(&*ui.fonts.ui) {
                gdi::text(
                    dc,
                    &text,
                    rc,
                    co::DT::CENTER | co::DT::VCENTER | co::DT::SINGLELINE | co::DT::END_ELLIPSIS,
                );
            }
            // Before there is a map: a bigger bar under the progress line.
            if let Some(activity) = session.activity() {
                let width = ui.px(320).min(rc.right - ui.px(40));
                let left = (rc.right - width) / 2;
                let top = rc.bottom / 2 + ui.px(14);
                let bar = w::RECT {
                    left,
                    top,
                    right: left + width,
                    bottom: top + ui.px(6),
                };
                gdi::fill(dc, bar, palette.rule);
                paint_progress(dc, bar, ui, activity);
                gdi::outline(dc, bar, 1, palette.border);
            }
            return;
        };
        if let Ok(src) = dc.CreateCompatibleDC()
            && let Ok(_bitmap) = src.SelectObject(&*cache.bitmap)
        {
            let _ = dc.BitBlt(
                w::POINT::default(),
                w::SIZE::with(rc.right, rc.bottom),
                &src,
                w::POINT::default(),
                co::ROP::SRCCOPY,
            );
        }
        let tiles = &cache.view.tiles;
        let zoom = session.zoom();
        let selected = session
            .selected()
            .filter(|path| path.len() > zoom.len() && path.starts_with(zoom))
            .and_then(|path| treemap::find(tiles, &path[zoom.len()..]));
        // The folder a double-click would open, then the selection, then
        // the tile under the pointer, each over the last.
        if let Some(index) = ui.target.filter(|&index| index < tiles.len()) {
            let r = gdi::rect(view::PixelRect::from_rect(tiles[index].rect));
            gdi::outline(dc, r, ui.px(2), palette.link);
        }
        if let Some(index) = selected {
            let r = gdi::rect(view::PixelRect::from_rect(tiles[index].rect));
            gdi::outline(dc, r, ui.px(2), palette.selected);
        }
        if let Some(index) = ui.hover.filter(|&index| index < tiles.len()) {
            let tile = tiles[index];
            let r = gdi::rect(view::PixelRect::from_rect(tile.rect));
            gdi::outline(dc, r, 1, palette.hovered);
            if let Some(at) = ui.tip {
                let (title, body) = match tile.kind {
                    TileKind::Group { folders, largest } => {
                        let folder = treemap::folder_of(tiles, index).unwrap_or_default();
                        session.group_tooltip(&folder, tile.child, tile.bytes, folders, largest)
                    }
                    _ => session.tooltip(treemap::path_of(tiles, index).as_deref()),
                };
                draw_tip(dc, rc, ui, at, &title, &body);
            }
        }
    });
    ui.map_back_buffer = buffer;
    result
}

/// Fill a progress bar's track `bar` for `activity`: the share done from
/// the left, or a segment sweeping across when there is no estimate.
fn paint_progress(dc: &w::HDC, bar: w::RECT, ui: &Ui, activity: Activity) {
    let done = match activity {
        Activity::Scanning { done } => done,
        Activity::Deleting => None,
    };
    let seconds = ui
        .busy_since
        .map_or(0.0, |since| since.elapsed().as_secs_f32());
    let (start, length) = view::progress_span(bar.right - bar.left, done, seconds);
    if length > 0 {
        let fill = w::RECT {
            left: bar.left + start,
            right: bar.left + start + length,
            ..bar
        };
        gdi::fill(dc, fill, ui.palette.link);
    }
}

/// Draw every caption into the map bitmap. Each line is centred and ends in
/// "…" when it is too wide for its tile, so a cut name always shows it is
/// cut.
fn draw_captions(
    hdc: &w::HDC,
    bitmap: &w::HBITMAP,
    view: &View,
    font: &w::HFONT,
    ink: Rgb,
) -> w::SysResult<()> {
    let dc = hdc.CreateCompatibleDC()?;
    let _bitmap = dc.SelectObject(bitmap)?;
    let _font = dc.SelectObject(font)?;
    dc.SetBkMode(co::BKMODE::TRANSPARENT)?;
    dc.SetTextColor(gdi::colorref(ink))?;
    let line = dc.GetTextMetrics()?.tmHeight.max(1);
    let single = co::DT::SINGLELINE | co::DT::VCENTER | co::DT::END_ELLIPSIS;
    let space = gdi::measure(&dc, "  ").cx;
    for caption in &view.captions {
        let clip = gdi::rect(caption.clip);
        if !caption.centered {
            // A folder strip: its name, then its size when both fit.
            let name_w = gdi::measure(&dc, &caption.text).cx;
            let detail = caption.detail.as_deref().filter(|detail| {
                name_w + space + gdi::measure(&dc, detail).cx <= clip.right - clip.left
            });
            gdi::text(&dc, &caption.text, clip, co::DT::LEFT | single);
            if let Some(detail) = detail {
                let after = w::RECT {
                    left: clip.left + name_w + space,
                    ..clip
                };
                gdi::text(&dc, detail, after, co::DT::LEFT | single);
            }
            continue;
        }
        let lines = caption.text.lines().count();
        let height = i32::try_from(lines).unwrap_or(1).saturating_mul(line);
        let mut top = clip.top + ((clip.bottom - clip.top - height) / 2).max(0);
        for text in caption.text.lines() {
            if top >= clip.bottom {
                break;
            }
            let row = w::RECT {
                top,
                bottom: (top + line).min(clip.bottom),
                ..clip
            };
            gdi::text(&dc, text, row, co::DT::CENTER | single);
            top += line;
        }
    }
    Ok(())
}

/// The hover tooltip: name in bold, details below, near the pointer and
/// kept inside the map.
fn draw_tip(dc: &w::HDC, rc: w::RECT, ui: &Ui, at: (i32, i32), title: &str, body: &str) {
    if title.is_empty() {
        return;
    }
    let pad = ui.px(6);
    let measure = |font: &w::HFONT, text: &str| {
        dc.SelectObject(font)
            .map_or(w::SIZE::default(), |_font| gdi::measure(dc, text))
    };
    let head = measure(&ui.fonts.ui_bold, title);
    let rest = measure(&ui.fonts.ui, body);
    let width = head.cx.max(rest.cx) + 2 * pad;
    let height = head.cy + rest.cy + 2 * pad + ui.px(2);
    let mut x = at.0 + ui.px(14);
    let mut y = at.1 + ui.px(20);
    if x + width > rc.right {
        x = (at.0 - width - ui.px(4)).max(0);
    }
    if y + height > rc.bottom {
        y = (at.1 - height - ui.px(4)).max(0);
    }
    let frame = w::RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    };
    gdi::fill(dc, frame, ui.palette.tip_fill);
    gdi::outline(dc, frame, 1, ui.palette.tip_edge);
    let _ = dc.SetBkMode(co::BKMODE::TRANSPARENT);
    let _ = dc.SetTextColor(gdi::colorref(ui.palette.text));
    let inner = w::RECT {
        left: x + pad,
        top: y + pad,
        right: x + width - pad,
        bottom: y + height - pad,
    };
    if let Ok(_font) = dc.SelectObject(&*ui.fonts.ui_bold) {
        gdi::text(dc, title, inner, co::DT::LEFT | co::DT::TOP);
    }
    if let Ok(_font) = dc.SelectObject(&*ui.fonts.ui) {
        gdi::text(
            dc,
            body,
            w::RECT {
                top: inner.top + head.cy + ui.px(2),
                ..inner
            },
            co::DT::LEFT | co::DT::TOP,
        );
    }
}
