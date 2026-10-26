//! The Wayland application: window, input, clipboard, input method and the
//! event loop tying them to the terminal and the renderer.

use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use smithay_client_toolkit::background_effect::{BackgroundEffectHandler, BackgroundEffectState};
use smithay_client_toolkit::compositor::{
    CompositorHandler, CompositorState, FrameCallbackData, Region,
};
use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::Capability as EffectCapability;
use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
use smithay_client_toolkit::data_device_manager::data_device::{DataDevice, DataDeviceHandler};
use smithay_client_toolkit::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use smithay_client_toolkit::data_device_manager::data_source::{
    CopyPasteSource, DataSourceHandler,
};
use smithay_client_toolkit::data_device_manager::{DataDeviceManagerState, ReadPipe, WritePipe};
use smithay_client_toolkit::activation::{ActivationHandler, ActivationState, RequestData};
use smithay_client_toolkit::primary_selection::PrimarySelectionManagerState;
use smithay_client_toolkit::primary_selection::device::{
    PrimarySelectionDevice, PrimarySelectionDeviceHandler,
};
use smithay_client_toolkit::primary_selection::selection::{
    PrimarySelectionSource, PrimarySelectionSourceHandler,
};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use smithay_client_toolkit::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use smithay_client_toolkit::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1;
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction,
};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    Keymap,
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
use wayland_client::globals::registry_queue_init;
use xkbcommon::xkb;
use wayland_client::protocol::wl_data_device::WlDataDevice;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_data_source::WlDataSource;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat, wl_surface};
use wayland_client::{Connection, Proxy, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ChangeCause, ContentHint, ContentPurpose, ZwpTextInputV3,
};

use crate::config::{Config, RendererKind};
use crate::font::{Fonts, points_to_px};
use crate::ime::{ImeState, Surrounding};
use crate::input::{self, KeyModes, MouseAction, MouseButton, MouseEvent};
use crate::keybinds::Action;
use crate::links::{Detector, Link};
use crate::pty::{Pty, WindowSize};
use crate::renderer::{Frame, Renderer, Theme, gl, software};
use crate::search::{self, Direction};
use crate::selection::{Point, Selection, Unit};
use crate::terminal::{DefaultColors, MouseMode, Terminal};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// A scrollback search: the query and the current match.
#[derive(Default)]
struct Search {
    query: String,
    found: Option<(Point, Point)>,
}

impl Search {
    /// The text of the search bar.
    fn bar(&self) -> String {
        let status = if self.found.is_none() && !self.query.is_empty() {
            " (not found)"
        } else {
            ""
        };
        format!("search{status}: {}", self.query)
    }
}

/// Clicks on the same cell within this many milliseconds of each other make
/// a double or triple click.
const DOUBLE_CLICK_MS: u32 = 400;
/// Longest a program can hold drawing with synchronized output (mode 2026),
/// in case it never ends the update.
const SYNC_TIMEOUT: Duration = Duration::from_millis(150);
/// Read at most this much child output before drawing, to stay responsive.
const READ_BUDGET: usize = 1 << 20;
const PASTE_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// The preferred text type among those a paste offer has.
fn paste_mime(offered: &[String]) -> Option<String> {
    PASTE_MIMES
        .iter()
        .find(|m| offered.iter().any(|o| o == *m))
        .map(|m| m.to_string())
}

pub struct App {
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    data_device_manager: Option<DataDeviceManagerState>,
    primary_selection_manager: Option<PrimarySelectionManagerState>,
    /// xdg-activation, to mark the window urgent when the bell rings.
    activation: Option<ActivationState>,
    text_input_manager: Option<ZwpTextInputManagerV3>,
    background_effect: BackgroundEffectState,
    /// Present once the compositor agreed to blur behind the window.
    blur: Option<ExtBackgroundEffectSurfaceV1>,

    config: Config,
    // Declared before `window` so the EGL surface is destroyed before the
    // wl_surface it renders to.
    renderer: Renderer,
    window: Window,
    term: Terminal,
    pty: Pty,
    pty_write_queued: bool,

    /// Window size in logical pixels and the scale factor, which is
    /// fractional with wp-fractional-scale.
    size: (u32, u32),
    scale: f64,
    /// With fractional scaling, the buffer is drawn at the scaled size and
    /// the viewport maps it back to the logical size.
    viewport: Option<WpViewport>,
    _fractional_scale: Option<WpFractionalScaleV1>,
    font_points: f32,
    configured: bool,
    /// The screen changed and needs drawing.
    dirty: bool,
    /// A frame callback is outstanding; drawing waits for it.
    frame_pending: bool,
    /// When the program started a synchronized update that is still open.
    sync_since: Option<Instant>,
    exit: bool,

    keyboard: Option<wl_keyboard::WlKeyboard>,
    modifiers: Modifiers,
    /// Our own copy of the keyboard's keymap, to find what a key produces
    /// without modifiers (for key bindings), and the active layout.
    keymap: Option<xkb::Keymap>,
    layout: u32,
    focused: bool,
    /// Serial of the latest input event, for clipboard requests.
    last_serial: u32,
    pointer: Option<ThemedPointer>,
    pointer_pos: (f64, f64),
    scroll_accum: f64,
    /// Button held down while reporting to the program, if any.
    mouse_button: Option<MouseButton>,
    /// Cell of the last reported pointer event, to report motion once per cell.
    mouse_cell: Option<(usize, usize)>,
    selection: Option<Selection>,
    selecting: bool,
    /// Scrollback search in progress.
    search: Option<Search>,
    /// The `--config` file, passed on to new windows.
    config_path: Option<PathBuf>,
    links: Detector,
    /// The link under the pointer while Ctrl is held, drawn underlined.
    hovered_link: Option<Link>,
    pointer_inside: bool,
    conn: Connection,
    /// Time, cell and count (1-3) of the last left click, to detect double
    /// and triple clicks.
    last_click: Option<(u32, Point, u8)>,
    data_device: Option<DataDevice>,
    copy_source: Option<(CopyPasteSource, String)>,
    /// The primary selection: set by selecting text, pasted by middle click.
    primary_device: Option<PrimarySelectionDevice>,
    primary_source: Option<(PrimarySelectionSource, String)>,

    text_input: Option<ZwpTextInputV3>,
    ime: ImeState,
    ime_caused_change: bool,
    ime_cursor_rect: (i32, i32, i32, i32),
}

/// Runs a window. `config_path` is the config file given on the command
/// line, which new windows also use.
pub fn run(
    config: Config,
    config_path: Option<PathBuf>,
    command: Option<Vec<String>>,
) -> Result<(), Box<dyn Error>> {
    let conn =
        Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let (globals, event_queue) = registry_queue_init::<App>(&conn)?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| format!("cannot watch the Wayland socket: {e}"))?;

    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let xdg_shell = XdgShell::bind(&globals, &qh).map_err(|e| format!("xdg_wm_base: {e}"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("wl_shm: {e}"))?;
    let background_effect = BackgroundEffectState::new(&globals, &qh);
    if config.window.blur
        && background_effect
            .ext_background_effect_manager_v1()
            .is_err()
    {
        log::warn!("blur requested, but the compositor lacks ext-background-effect-v1");
    }
    let data_device_manager = DataDeviceManagerState::bind(&globals, &qh).ok();
    let activation = ActivationState::bind(&globals, &qh)
        .map_err(|e| log::info!("no xdg-activation, the bell won't mark the window urgent: {e}"))
        .ok();
    let primary_selection_manager = PrimarySelectionManagerState::bind(&globals, &qh)
        .map_err(|e| log::info!("no primary selection: {e}"))
        .ok();
    let text_input_manager = globals
        .bind::<ZwpTextInputManagerV3, _, _>(&qh, 1..=1, TextInputManagerData)
        .map_err(|e| log::warn!("text-input-v3 unavailable, no input method support: {e}"))
        .ok();

    let load_fonts = || {
        Fonts::new(
            &config.font.family,
            &config.font.fallback,
            points_to_px(config.font.size, 1.0),
        )
    };
    let fonts = load_fonts()?;
    let cell = fonts.metrics;
    let pad = config.window.padding;
    let cols = u32::from(config.window.columns);
    let lines = u32::from(config.window.lines);
    let size = (cols * cell.width + 2 * pad, lines * cell.height + 2 * pad);

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title(config.window.title.clone());
    window.set_app_id(config.window.app_id.clone());
    window.set_min_size(Some((cell.width * 2 + 2 * pad, cell.height + 2 * pad)));
    if config.window.opacity >= 1.0 {
        // Lets the compositor skip drawing what is behind the window.
        set_whole_region(&compositor, |region| {
            window.wl_surface().set_opaque_region(region)
        });
    }
    // Fractional scaling needs both protocols; otherwise the scale is the
    // integer buffer scale.
    let viewporter = globals
        .bind::<WpViewporter, _, _>(&qh, 1..=1, ScaleData)
        .ok();
    let fractional_manager = globals
        .bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ScaleData)
        .ok();
    let (viewport, fractional_scale) = match (&viewporter, &fractional_manager) {
        (Some(viewporter), Some(manager)) => (
            Some(viewporter.get_viewport(window.wl_surface(), &qh, ScaleData)),
            Some(manager.get_fractional_scale(window.wl_surface(), &qh, ScaleData)),
        ),
        _ => {
            log::info!("no fractional scaling (wp-fractional-scale or wp-viewporter missing)");
            (None, None)
        }
    };
    window.commit();

    let kinds: Vec<Vec<String>> = config
        .files
        .0
        .values()
        .map(|k| k.extensions.clone())
        .collect();
    let links = Detector::new(&kinds, &config.url.regex).unwrap_or_else(|e| {
        log::error!("invalid url.regex, ignoring it: {e}");
        Detector::new(&kinds, &[]).expect("the built-in patterns are valid")
    });
    let theme = Theme::from_config(&config);
    let mut fonts = Some(fonts);
    let gpu = match config.window.renderer {
        // SAFETY: the display outlives the renderer (the event loop holds a
        // connection until after `App` is dropped), and `App` drops the
        // renderer before the window.
        RendererKind::Gpu => match unsafe {
            gl::Renderer::new(
                conn.backend().display_ptr().cast(),
                window.wl_surface().id(),
                size,
                fonts.take().unwrap(),
                theme.clone(),
            )
        } {
            Ok(renderer) => Some(Renderer::Gpu(Box::new(renderer))),
            Err(e) => {
                log::warn!("cannot render with the GPU, using the CPU instead: {e}");
                None
            }
        },
        RendererKind::Cpu => None,
    };
    let renderer = match gpu {
        Some(renderer) => renderer,
        None => Renderer::Cpu(Box::new(software::Renderer::new(
            &shm,
            window.wl_surface().clone(),
            size,
            fonts.map_or_else(load_fonts, Ok)?,
            theme,
        )?)),
    };

    let (program, args) = match command {
        Some(mut command) => {
            let program = command.remove(0);
            (program, command)
        }
        None => (
            config
                .terminal
                .shell
                .clone()
                .unwrap_or_else(crate::pty::default_shell),
            Vec::new(),
        ),
    };
    let window_size = WindowSize {
        cols: cols as u16,
        rows: lines as u16,
        width: size.0 as u16,
        height: size.1 as u16,
    };
    let term = crate::terminfo::resolve(&config.terminal.term, &crate::terminfo::search_dirs());
    log::info!("TERM={} (terminfo: {:?})", term.name, term.terminfo);
    let pty = Pty::spawn(&program, &args, &term, window_size)
        .map_err(|e| format!("cannot run {program:?}: {e}"))?;
    let reader = File::from(pty.fd().try_clone_to_owned()?);
    loop_handle
        .insert_source(
            Generic::new(reader, Interest::READ, Mode::Level),
            |_, _, app| {
                app.read_pty();
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| format!("cannot watch the pty: {e}"))?;

    let mut term = Terminal::new(cols as usize, lines as usize, config.terminal.scrollback);
    let defaults = Theme::from_config(&config);
    let rgb = |c: crate::config::Rgb| (c.0, c.1, c.2);
    term.screen_mut().set_default_colors(DefaultColors {
        foreground: rgb(defaults.foreground),
        background: rgb(defaults.background),
        cursor: rgb(defaults.cursor),
        palette: std::array::from_fn(|i| rgb(defaults.indexed(i as u8))),
    });
    let font_points = config.font.size;
    let mut app = App {
        qh: qh.clone(),
        loop_handle,
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        data_device_manager,
        primary_selection_manager,
        activation,
        text_input_manager,
        background_effect,
        blur: None,
        config,
        renderer,
        window,
        term,
        pty,
        pty_write_queued: false,
        size,
        scale: 1.0,
        viewport,
        _fractional_scale: fractional_scale,
        font_points,
        configured: false,
        dirty: true,
        frame_pending: false,
        sync_since: None,
        exit: false,
        keyboard: None,
        modifiers: Modifiers::default(),
        keymap: None,
        layout: 0,
        focused: false,
        last_serial: 0,
        pointer: None,
        pointer_pos: (0.0, 0.0),
        scroll_accum: 0.0,
        mouse_button: None,
        mouse_cell: None,
        selection: None,
        selecting: false,
        search: None,
        config_path,
        links,
        hovered_link: None,
        pointer_inside: false,
        conn: conn.clone(),
        last_click: None,
        data_device: None,
        copy_source: None,
        primary_device: None,
        primary_source: None,
        text_input: None,
        ime: ImeState::default(),
        ime_caused_change: false,
        ime_cursor_rect: (0, 0, 0, 0),
    };

    // SCTK reports only seats added later through `new_seat`.
    let seats: Vec<_> = app.seat_state.seats().collect();
    for seat in seats {
        app.new_seat(&conn, &qh, seat);
    }

    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
    }
    if let Ok(Some(status)) = app.pty.try_wait() {
        log::info!("child exited: {status}");
    }
    Ok(())
}

impl App {
    fn physical_size(&self) -> (u32, u32) {
        let scaled = |v: u32| (f64::from(v) * self.scale).round() as u32;
        (scaled(self.size.0), scaled(self.size.1))
    }

    fn padding(&self) -> u32 {
        (f64::from(self.config.window.padding) * self.scale).round() as u32
    }

    /// Applies a new scale factor: re-rasterizes the font and resizes.
    fn set_scale(&mut self, scale: f64) {
        if scale == self.scale || scale <= 0.0 {
            return;
        }
        log::info!("scale factor {scale}");
        self.scale = scale;
        if self.viewport.is_none() {
            self.window.wl_surface().set_buffer_scale(scale as i32);
        }
        self.renderer
            .set_font_size(points_to_px(self.font_points, scale as f32));
        self.apply_size();
    }

    /// Grid dimensions that fit the window.
    fn grid_size(&self) -> (usize, usize) {
        let (w, h) = self.physical_size();
        let pad = self.padding();
        let cell = self.renderer.cell();
        let cols = (w.saturating_sub(2 * pad) / cell.width).max(1);
        let rows = (h.saturating_sub(2 * pad) / cell.height).max(1);
        (cols as usize, rows as usize)
    }

    /// Propagates a window, scale or font size change to the renderer,
    /// the terminal and the child.
    fn apply_size(&mut self) {
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(self.size.0.max(1) as i32, self.size.1.max(1) as i32);
        }
        self.renderer.resize(self.physical_size());
        let (cols, rows) = self.grid_size();
        self.term.screen_mut().resize(cols, rows);
        let (w, h) = self.physical_size();
        let size = WindowSize {
            cols: cols as u16,
            rows: rows as u16,
            width: w.min(u16::MAX.into()) as u16,
            height: h.min(u16::MAX.into()) as u16,
        };
        if let Err(e) = self.pty.resize(size) {
            log::warn!("cannot resize pty: {e}");
        }
        self.selection = None;
        self.request_redraw();
    }

    fn set_font_points(&mut self, points: f32) {
        self.font_points = points.clamp(4.0, 200.0);
        self.renderer
            .set_font_size(points_to_px(self.font_points, self.scale as f32));
        self.apply_size();
    }

    fn read_pty(&mut self) {
        // What the selection covers, to keep it while output goes elsewhere.
        let selected = self.selection.map(|s| {
            let screen = self.term.screen();
            (
                s.live_rows(screen),
                screen.grid().scrolled,
                screen.is_alt_screen(),
            )
        });
        let mut buf = [0u8; 1 << 16];
        let mut total = 0;
        while total < READ_BUDGET {
            match self.pty.read(&mut buf) {
                Ok(0) => {
                    self.exit = true;
                    break;
                }
                Ok(n) => {
                    self.term.feed(&buf[..n]);
                    total += n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    // EIO: every process holding the pty has exited.
                    log::debug!("pty closed: {e}");
                    self.exit = true;
                    break;
                }
            }
        }
        if total == 0 {
            return;
        }

        let screen = self.term.screen_mut();
        let responses = screen.take_responses();
        let title = screen.take_title();
        let bell = screen.take_bell();
        let dirty = screen.take_dirty();
        if !responses.is_empty() {
            self.send(&responses);
        }
        if let Some(title) = title {
            self.window.set_title(title);
        }
        if bell {
            self.ring_bell();
        }
        if dirty {
            if self.hovered_link.is_some() {
                self.update_hovered_link();
            }
            let screen = self.term.screen();
            if let (Some(selection), Some((rows, scrolled, alt))) = (&mut self.selection, selected)
            {
                let delta = screen.grid().scrolled.wrapping_sub(scrolled) as isize;
                // Another buffer means other text; history and alternate
                // screens never share lines.
                let keep =
                    alt == screen.is_alt_screen() && selection.follow_output(screen, &rows, delta);
                if !keep {
                    self.selection = None;
                    self.selecting = false;
                }
            }
            self.request_redraw();
        }
    }

    /// Sends input to the child, queueing what the pty cannot take yet.
    fn send(&mut self, bytes: &[u8]) {
        if let Err(e) = self.pty.write(bytes) {
            log::warn!("cannot write to pty: {e}");
            return;
        }
        if !self.pty.has_pending() || self.pty_write_queued {
            return;
        }
        let Ok(fd) = self.pty.fd().try_clone_to_owned() else {
            return;
        };
        let source = Generic::new(File::from(fd), Interest::WRITE, Mode::Level);
        let inserted = self.loop_handle.insert_source(source, |_, _, app| {
            if let Err(e) = app.pty.flush() {
                log::warn!("cannot write to pty: {e}");
            }
            if app.pty.has_pending() {
                Ok(PostAction::Continue)
            } else {
                app.pty_write_queued = false;
                Ok(PostAction::Remove)
            }
        });
        self.pty_write_queued = inserted.is_ok();
    }

    fn request_redraw(&mut self) {
        self.dirty = true;
        if self.configured && !self.frame_pending && !self.holding_for_sync() {
            self.draw();
        }
    }

    /// Whether drawing waits for the program to finish a synchronized
    /// update. A timer redraws anyway after `SYNC_TIMEOUT`.
    fn holding_for_sync(&mut self) -> bool {
        if !self.term.screen().modes.synchronized {
            self.sync_since = None;
            return false;
        }
        let since = *self.sync_since.get_or_insert_with(|| {
            let timer = Timer::from_duration(SYNC_TIMEOUT);
            let inserted = self.loop_handle.insert_source(timer, |_, _, app| {
                if app.dirty {
                    app.request_redraw();
                }
                TimeoutAction::Drop
            });
            if let Err(e) = inserted {
                log::warn!("cannot start the synchronized-output timer: {e}");
            }
            Instant::now()
        });
        since.elapsed() < SYNC_TIMEOUT
    }

    fn draw(&mut self) {
        self.dirty = false;
        let surface = self.window.wl_surface().clone();
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        self.frame_pending = true;

        let pad = self.padding();
        let search_bar = self.search.as_ref().map(Search::bar);
        let frame = Frame {
            screen: self.term.screen(),
            selection: self.selection.as_ref(),
            preedit: self.ime.preedit.as_ref(),
            focused: self.focused,
            origin: (pad, pad),
            link: self.hovered_link.as_ref().map(|l| (l.start, l.end)),
            search: search_bar.as_deref(),
        };
        match self.renderer.draw(&frame) {
            Ok(again) => self.dirty |= again,
            Err(e) => {
                log::error!("{e}");
                self.exit = true;
            }
        }
        self.update_text_input(false);
    }

    /// The symbol `raw_code` produces without modifiers, in the active layout.
    fn base_keysym(&self, raw_code: u32) -> Option<Keysym> {
        let keymap = self.keymap.as_ref()?;
        // Wayland key codes are evdev codes; xkb's are offset by 8.
        let code = xkb::Keycode::new(raw_code + 8);
        keymap
            .key_get_syms_by_level(code, self.layout, 0)
            .first()
            .copied()
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if self.search.is_some() {
            return self.search_key(&event);
        }
        let base = self.base_keysym(event.raw_code);
        if let Some(action) = self
            .config
            .keybinds
            .lookup(event.keysym, base, self.modifiers)
            && self.run_action(action)
        {
            return;
        }
        let modes = self.term.screen().modes;
        let modes = KeyModes {
            app_cursor: modes.app_cursor,
            app_keypad: modes.app_keypad,
        };
        if let Some(bytes) =
            input::encode_key(event.keysym, event.utf8.as_deref(), self.modifiers, modes)
        {
            self.scroll_to_bottom();
            self.send(&bytes);
        }
    }

    /// Handles a key while searching: it edits the query or moves between
    /// matches instead of going to the program.
    fn search_key(&mut self, event: &KeyEvent) {
        let ctrl = self.modifiers.ctrl;
        match event.keysym {
            Keysym::Escape => {
                self.search = None;
                self.selection = None;
            }
            Keysym::Return | Keysym::KP_Enter => {
                self.search = None;
                self.set_primary();
            }
            Keysym::BackSpace => {
                if let Some(search) = &mut self.search {
                    search.query.pop();
                }
                self.search_step(Direction::Older, true);
            }
            Keysym::Up => self.search_step(Direction::Older, false),
            Keysym::Down => self.search_step(Direction::Newer, false),
            Keysym::r if ctrl => self.search_step(Direction::Older, false),
            Keysym::s if ctrl => self.search_step(Direction::Newer, false),
            _ if !ctrl && !self.modifiers.alt => {
                if let Some(text) = &event.utf8 {
                    self.search_type(text);
                }
            }
            _ => {}
        }
        self.request_redraw();
    }

    /// Adds typed (or input-method) text to the search query.
    fn search_type(&mut self, text: &str) {
        let Some(search) = &mut self.search else {
            return;
        };
        search
            .query
            .extend(text.chars().filter(|c| !c.is_control()));
        self.search_step(Direction::Older, true);
    }

    /// Moves to the next match in `direction` from the current one (or from
    /// the bottom), selecting it and scrolling it into view. `inclusive`
    /// keeps the current match if it still matches: the query changed.
    fn search_step(&mut self, direction: Direction, inclusive: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        let screen = self.term.screen();
        let from = search.found.map_or(
            Point {
                line: screen.rows() as isize - 1,
                col: usize::MAX,
            },
            |m| m.0,
        );
        match search::find(screen, &search.query, from, direction, inclusive) {
            Some(found) => {
                search.found = Some(found);
                self.selection = Some(Selection::span(found.0, found.1, Unit::Word));
                self.term.screen_mut().show_line(found.0.line);
            }
            // Moving past the last match keeps it; a changed query that
            // matches nothing clears it.
            None if inclusive => {
                search.found = None;
                self.selection = None;
            }
            None => {}
        }
    }

    fn scroll_to_bottom(&mut self) {
        if self.term.screen().display_offset() > 0 {
            self.term.screen_mut().reset_display();
            self.request_redraw();
        }
    }

    /// Runs a bound action. Returns false when the action declined the key,
    /// which then goes to the program as usual.
    fn run_action(&mut self, action: Action) -> bool {
        let page = self.term.screen().rows() as isize;
        match action {
            Action::Copy => self.copy(),
            Action::CopyOrInterrupt => {
                if !self.selection.is_some_and(|s| !s.is_empty()) {
                    return false;
                }
                self.copy();
                // Cleared, so pressing the key again interrupts.
                self.selection = None;
                self.request_redraw();
            }
            Action::Paste => self.paste(),
            Action::ScrollLineUp => self.term.screen_mut().scroll_display(1),
            Action::ScrollLineDown => self.term.screen_mut().scroll_display(-1),
            Action::ScrollPageUp => self.term.screen_mut().scroll_display(page),
            Action::ScrollPageDown => self.term.screen_mut().scroll_display(-page),
            Action::ScrollToTop => self.term.screen_mut().scroll_display(isize::MAX),
            Action::ScrollToBottom => self.term.screen_mut().reset_display(),
            Action::FontLarger => self.set_font_points(self.font_points + 1.0),
            Action::FontSmaller => self.set_font_points(self.font_points - 1.0),
            Action::FontReset => self.set_font_points(self.config.font.size),
            Action::Search => {
                self.search = Some(Search::default());
                self.request_redraw();
            }
            Action::NewWindow => self.new_window(),
        }
        if self.term.screen_mut().take_dirty() {
            self.request_redraw();
        }
        true
    }

    /// The visible cell under a surface-local position, as (column, row).
    fn cell_at(&self, (x, y): (f64, f64)) -> (usize, usize) {
        let scale = self.scale;
        let pad = f64::from(self.padding());
        let cell = self.renderer.cell();
        let screen = self.term.screen();
        let col = ((x * scale - pad) / f64::from(cell.width)).floor().max(0.0) as usize;
        let row = ((y * scale - pad) / f64::from(cell.height))
            .floor()
            .max(0.0) as usize;
        (col.min(screen.cols() - 1), row.min(screen.rows() - 1))
    }

    /// The grid cell under a surface-local position, in selection coordinates.
    fn point_at(&self, pos: (f64, f64)) -> Point {
        let (col, row) = self.cell_at(pos);
        Point {
            line: row as isize - self.term.screen().display_offset() as isize,
            col,
        }
    }

    /// A selection of the whole command output under `point`, for a
    /// double click, if that is enabled. Otherwise a double click selects
    /// a word.
    fn output_selection(&self, point: Point) -> Option<Selection> {
        if !self.config.terminal.double_click_selects_output {
            return None;
        }
        let screen = self.term.screen();
        let (first, last) = screen.command_output(point.line)?;
        let end = screen.row_at(last)?.cells.len().saturating_sub(1);
        Some(Selection::span(
            Point {
                line: first,
                col: 0,
            },
            Point {
                line: last,
                col: end,
            },
            Unit::Word,
        ))
    }

    /// Underlines the link under the pointer while Ctrl is held, and shows
    /// a hand pointer over it.
    fn update_hovered_link(&mut self) {
        let link = if self.modifiers.ctrl && self.pointer_inside && !self.reporting_mouse() {
            let cwd = std::cell::OnceCell::new();
            let resolve = |path: &str| {
                let path = match path.strip_prefix("~/") {
                    Some(rest) => std::env::home_dir()?.join(rest),
                    None => {
                        let path = std::path::Path::new(path);
                        cwd.get_or_init(|| self.pty.foreground_cwd())
                            .as_ref()
                            .map_or(path.to_path_buf(), |dir| dir.join(path))
                    }
                };
                path.is_file().then(|| path.to_string_lossy().into_owned())
            };
            self.links
                .link_at(self.term.screen(), self.point_at(self.pointer_pos), resolve)
        } else {
            None
        };
        if link == self.hovered_link {
            return;
        }
        let icon = if link.is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Text
        };
        if let Some(pointer) = &self.pointer
            && let Err(e) = pointer.set_cursor(&self.conn, icon)
        {
            log::debug!("cannot set cursor: {e}");
        }
        self.hovered_link = link;
        self.request_redraw();
    }

    /// Starts another jotty in the current directory of the program in the
    /// foreground, with the same config file.
    fn new_window(&self) {
        let program = match std::env::current_exe() {
            Ok(program) => program,
            Err(e) => return log::warn!("cannot find jotty's executable: {e}"),
        };
        let mut command = std::process::Command::new(program);
        if let Some(path) = &self.config_path {
            // The new window starts elsewhere; a relative path would break.
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            command.arg("--config").arg(path);
        }
        if let Some(dir) = self.pty.foreground_cwd() {
            log::info!("opening a new window in {}", dir.display());
            command.current_dir(dir);
        }
        // Its own process group: a hangup sent to ours (when the terminal
        // that started this jotty closes) must not take it down.
        use std::os::unix::process::CommandExt;
        command.process_group(0).stdin(std::process::Stdio::null());
        match command.spawn() {
            // Reap it when it exits so it doesn't linger as a zombie.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => log::warn!("cannot start a new window: {e}"),
        }
    }

    /// Opens a link with its launcher (its file kind's, or else
    /// `url.launcher`), without waiting for it.
    fn open_link(&self, link: &Link) {
        let target = &link.target;
        let launcher = link
            .kind
            .and_then(|i| self.config.files.0.values().nth(i))
            .map(|kind| &kind.launcher)
            .filter(|launcher| !launcher.is_empty())
            .unwrap_or(&self.config.url.launcher);
        let Some((program, args)) = launcher.split_first() else {
            return;
        };
        log::info!("opening {target}");
        // Browsers print noise on startup (Chromium's "zygote" errors when
        // an instance is already running); only the exit status matters.
        let child = std::process::Command::new(program)
            .args(args)
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            // Reap it in the background so it doesn't linger as a zombie.
            Ok(mut child) => {
                let program = program.clone();
                std::thread::spawn(move || match child.wait() {
                    Ok(status) if !status.success() => {
                        log::warn!("{program} failed to open the link: {status}")
                    }
                    _ => {}
                });
            }
            Err(e) => log::warn!("cannot run {program}: {e}"),
        }
    }

    /// Whether pointer events go to the program: it asked for them, and
    /// Shift isn't held (Shift keeps selecting and scrolling in jotty).
    fn reporting_mouse(&self) -> bool {
        self.term.screen().modes.mouse != MouseMode::Off && !self.modifiers.shift
    }

    /// Sends a pointer event to the program, if its mode includes it.
    fn report_mouse(&mut self, action: MouseAction, pos: (f64, f64)) {
        let (col, row) = self.cell_at(pos);
        if matches!(action, MouseAction::Motion(_)) && self.mouse_cell == Some((col, row)) {
            return;
        }
        self.mouse_cell = Some((col, row));
        let modes = self.term.screen().modes;
        let event = MouseEvent {
            action,
            col,
            row,
            mods: self.modifiers,
        };
        if let Some(bytes) = input::encode_mouse(event, modes.mouse, modes.mouse_sgr) {
            self.send(&bytes);
        }
    }

    /// Converts a scroll event into whole steps, `per_notch` per wheel notch
    /// (touchpads scroll by pixels: one step per cell height).
    fn scroll_steps(
        &mut self,
        axis: &smithay_client_toolkit::seat::pointer::AxisScroll,
        per_notch: f64,
    ) -> isize {
        let cell_height = f64::from(self.renderer.cell().height) / self.scale;
        self.scroll_accum += if axis.value120 != 0 {
            f64::from(axis.value120) / 120.0 * per_notch
        } else if axis.discrete != 0 {
            f64::from(axis.discrete) * per_notch
        } else {
            axis.absolute / cell_height
        };
        let steps = self.scroll_accum.trunc();
        self.scroll_accum -= steps;
        steps as isize
    }

    /// Reports wheel movement as button 64–67 presses, one per notch.
    fn report_wheel(
        &mut self,
        vertical: &smithay_client_toolkit::seat::pointer::AxisScroll,
        horizontal: &smithay_client_toolkit::seat::pointer::AxisScroll,
        pos: (f64, f64),
    ) {
        for (axis, back, forward) in [
            (vertical, MouseButton::WheelUp, MouseButton::WheelDown),
            (horizontal, MouseButton::WheelLeft, MouseButton::WheelRight),
        ] {
            if axis.is_none() {
                continue;
            }
            let steps = self.scroll_steps(axis, 1.0);
            let button = if steps < 0 { back } else { forward };
            for _ in 0..steps.unsigned_abs() {
                self.mouse_cell = None;
                self.report_mouse(MouseAction::Press(button), pos);
            }
        }
    }

    fn scroll(&mut self, axis: &smithay_client_toolkit::seat::pointer::AxisScroll) {
        let lines = self.scroll_steps(axis, 3.0);
        if lines == 0 {
            return;
        }
        let screen = self.term.screen();
        if screen.is_alt_screen() {
            // Full-screen programs have no scrollback; send arrow keys instead.
            let key: &[u8] = match (lines < 0, screen.modes.app_cursor) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.send(&key.repeat(lines.unsigned_abs()));
        } else {
            self.term.screen_mut().scroll_display(-lines);
            if self.term.screen_mut().take_dirty() {
                self.request_redraw();
            }
        }
    }

    fn copy(&mut self) {
        let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
            return;
        };
        let text = selection.text(self.term.screen());
        let (Some(manager), Some(device)) = (&self.data_device_manager, &self.data_device) else {
            log::warn!("no clipboard support (wl_data_device_manager missing)");
            return;
        };
        if text.is_empty() {
            return;
        }
        let source = manager.create_copy_paste_source(&self.qh, PASTE_MIMES);
        source.set_selection(device, self.last_serial);
        self.copy_source = Some((source, text));
    }

    /// Offers the selection as the primary selection, which a middle click
    /// pastes (here or in other applications).
    fn set_primary(&mut self) {
        let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
            return;
        };
        let (Some(manager), Some(device)) = (&self.primary_selection_manager, &self.primary_device)
        else {
            return;
        };
        let text = selection.text(self.term.screen());
        if text.is_empty() {
            return;
        }
        let source = manager.create_selection_source(&self.qh, PASTE_MIMES);
        source.set_selection(device, self.last_serial);
        self.primary_source = Some((source, text));
    }

    /// Pastes the primary selection, or the clipboard where the compositor
    /// has no primary selection.
    fn paste_primary(&mut self) {
        if self.primary_selection_manager.is_none() {
            return self.paste();
        }
        if let Some((_, text)) = &self.primary_source {
            let text = text.clone();
            return self.paste_text(&text);
        }
        let Some(offer) = self
            .primary_device
            .as_ref()
            .and_then(|d| d.data().selection_offer())
        else {
            return;
        };
        let Some(mime) = offer.with_mime_types(paste_mime) else {
            return;
        };
        match offer.receive(mime) {
            Ok(pipe) => self.paste_from(pipe),
            Err(e) => log::warn!("cannot read the primary selection: {e}"),
        }
    }

    fn paste(&mut self) {
        // Our own selection: skip the round trip through the compositor.
        if let Some((_, text)) = &self.copy_source {
            let text = text.clone();
            self.paste_text(&text);
            return;
        }
        let Some(offer) = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().selection_offer())
        else {
            return;
        };
        let Some(mime) = offer.with_mime_types(paste_mime) else {
            return;
        };
        match offer.receive(mime) {
            Ok(pipe) => self.paste_from(pipe),
            Err(e) => log::warn!("cannot read clipboard: {e}"),
        }
    }

    /// Reads pasted text from another client without blocking, then sends it.
    fn paste_from(&mut self, pipe: ReadPipe) {
        let mut data = Vec::new();
        let inserted = self.loop_handle.insert_source(pipe, move |_, file, app| {
            // SAFETY: the file is only read, never closed or replaced.
            let file: &mut File = unsafe { file.get_mut() };
            let mut buf = [0u8; 1 << 14];
            match file.read(&mut buf) {
                Ok(0) => {
                    app.paste_text(&String::from_utf8_lossy(&data));
                    PostAction::Remove
                }
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    PostAction::Continue
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) =>
                {
                    PostAction::Continue
                }
                Err(e) => {
                    log::warn!("cannot read clipboard: {e}");
                    PostAction::Remove
                }
            }
        });
        if let Err(e) = inserted {
            log::warn!("cannot read clipboard: {e}");
        }
    }

    /// Asks the compositor to draw attention to the window, if it isn't
    /// focused: an activation request without a valid input serial, which
    /// compositors show as urgency instead of switching focus.
    fn ring_bell(&mut self) {
        if self.focused || !self.config.window.bell_urgent {
            return;
        }
        if let Some(activation) = &self.activation {
            activation.request_token(
                &self.qh,
                RequestData {
                    app_id: Some(self.config.window.app_id.clone()),
                    seat_and_serial: None,
                    surface: Some(self.window.wl_surface().clone()),
                    udata: (),
                },
            );
        }
    }

    fn paste_text(&mut self, text: &str) {
        let bytes = input::encode_paste(text, self.term.screen().modes.bracketed_paste);
        self.scroll_to_bottom();
        self.send(&bytes);
    }

    fn text_input_event(&mut self, text_input: &ZwpTextInputV3, event: zwp_text_input_v3::Event) {
        use zwp_text_input_v3::Event;
        match event {
            Event::Enter { surface } if &surface == self.window.wl_surface() => {
                self.ime.active = true;
                text_input.enable();
                text_input.set_content_type(ContentHint::None, ContentPurpose::Terminal);
                self.update_text_input(true);
            }
            Event::Leave { .. } => {
                text_input.disable();
                text_input.commit();
                self.ime.serial = self.ime.serial.wrapping_add(1);
                self.ime.reset();
                self.request_redraw();
            }
            Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => self.ime.set_preedit(text, cursor_begin, cursor_end),
            Event::CommitString { text } => self.ime.set_commit(text),
            Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => self.ime.set_delete(before_length, after_length),
            Event::Done { .. } => {
                let bytes = self.ime.done();
                if self.search.is_some() {
                    self.search_type(&String::from_utf8_lossy(&bytes));
                } else if !bytes.is_empty() {
                    self.ime_caused_change = true;
                    self.scroll_to_bottom();
                    self.send(&bytes);
                }
                self.request_redraw();
            }
            _ => {}
        }
    }

    /// Tells the input method about the text around the cursor and where
    /// the cursor is, if either changed (or always, with `force`).
    fn update_text_input(&mut self, force: bool) {
        if !self.ime.active {
            return;
        }
        let Some(text_input) = self.text_input.clone() else {
            return;
        };
        let screen = self.term.screen();
        let (row, col) = screen.cursor();
        let surrounding =
            Surrounding::from_row(&screen.grid().rows[row], screen.input_start(), col);
        let cell = self.renderer.cell();
        let pad = self.padding();
        let logical = |px: u32| (f64::from(px) / self.scale) as i32;
        let rect = (
            logical(pad + col as u32 * cell.width),
            logical(pad + row as u32 * cell.height),
            logical(cell.width).max(1),
            logical(cell.height).max(1),
        );
        if !force && surrounding == self.ime.surrounding && rect == self.ime_cursor_rect {
            return;
        }
        let cursor = surrounding.cursor as i32;
        text_input.set_surrounding_text(surrounding.text.clone(), cursor, cursor);
        let cause = if std::mem::take(&mut self.ime_caused_change) {
            ChangeCause::InputMethod
        } else {
            ChangeCause::Other
        };
        text_input.set_text_change_cause(cause);
        text_input.set_cursor_rectangle(rect.0, rect.1, rect.2, rect.3);
        text_input.commit();
        self.ime.serial = self.ime.serial.wrapping_add(1);
        self.ime.surrounding = surrounding;
        self.ime_cursor_rect = rect;
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // wp-fractional-scale reports the exact scale instead.
        if self.viewport.is_none() {
            self.set_scale(f64::from(factor.max(1)));
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.dirty {
            self.request_redraw();
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        if let Some(w) = configure.new_size.0 {
            self.size.0 = w.get();
        }
        if let Some(h) = configure.new_size.1 {
            self.size.1 = h.get();
        }
        self.configured = true;
        self.apply_size();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        // Created before the window gets focus, so the compositor's focus
        // change reaches it as an `enter` event.
        if let (None, Some(manager)) = (&self.text_input, &self.text_input_manager) {
            self.text_input = Some(manager.get_text_input(&seat, qh, TextInputData));
        }
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                let keyboard = self.seat_state.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.loop_handle.clone(),
                    Box::new(|app, _, event| app.handle_key(event)),
                );
                match keyboard {
                    Ok(keyboard) => self.keyboard = Some(keyboard),
                    Err(e) => log::warn!("cannot use keyboard: {e}"),
                }
                if let (None, Some(manager)) = (&self.data_device, &self.data_device_manager) {
                    self.data_device = Some(manager.get_data_device(qh, &seat));
                }
                if let (None, Some(manager)) =
                    (&self.primary_device, &self.primary_selection_manager)
                {
                    self.primary_device = Some(manager.get_selection_device(qh, &seat));
                }
            }
            Capability::Pointer if self.pointer.is_none() => {
                let surface = self.compositor.create_surface(qh);
                let pointer = self.seat_state.get_pointer_with_theme::<_, ()>(
                    qh,
                    &seat,
                    self.shm.wl_shm(),
                    surface,
                    ThemeSpec::default(),
                );
                match pointer {
                    Ok(pointer) => self.pointer = Some(pointer),
                    Err(e) => log::warn!("cannot use pointer: {e}"),
                }
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                if let Some(keyboard) = self.keyboard.take() {
                    keyboard.release();
                }
            }
            Capability::Pointer => {
                if let Some(pointer) = self.pointer.take() {
                    pointer.pointer().release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        serial: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if surface == self.window.wl_surface() {
            self.focused = true;
            self.last_serial = serial;
            if self.term.screen().modes.focus_events {
                self.send(b"\x1b[I");
            }
            self.request_redraw();
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if surface == self.window.wl_surface() {
            self.focused = false;
            if self.term.screen().modes.focus_events {
                self.send(b"\x1b[O");
            }
            self.request_redraw();
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        self.last_serial = serial;
        self.handle_key(event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        layout: u32,
    ) {
        self.modifiers = modifiers;
        self.layout = layout;
        self.update_hovered_link();
    }

    fn update_keymap(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        keymap: Keymap<'_>,
    ) {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        self.keymap = xkb::Keymap::new_from_string(
            &context,
            keymap.as_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        );
        if self.keymap.is_none() {
            log::warn!("cannot compile the keymap; key bindings match shifted symbols only");
        }
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }
            self.pointer_pos = event.position;
            if matches!(event.kind, PointerEventKind::Motion { .. })
                && (self.modifiers.ctrl || self.hovered_link.is_some())
            {
                self.update_hovered_link();
            }
            match &event.kind {
                PointerEventKind::Enter { .. } => {
                    self.pointer_inside = true;
                    if let Some(pointer) = &self.pointer
                        && let Err(e) = pointer.set_cursor(conn, CursorIcon::Text)
                    {
                        log::debug!("cannot set cursor: {e}");
                    }
                    self.update_hovered_link();
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_inside = false;
                    self.update_hovered_link();
                }
                PointerEventKind::Motion { .. } if self.reporting_mouse() => {
                    self.report_mouse(MouseAction::Motion(self.mouse_button), event.position);
                }
                PointerEventKind::Motion { .. } if self.selecting => {
                    let point = self.point_at(event.position);
                    if let Some(selection) = &mut self.selection {
                        selection.extend(self.term.screen(), point);
                        self.request_redraw();
                    }
                }
                PointerEventKind::Press { button, serial, .. }
                    if self.reporting_mouse() && mouse_button(*button).is_some() =>
                {
                    self.last_serial = *serial;
                    let button = mouse_button(*button).unwrap();
                    self.mouse_button = Some(button);
                    self.mouse_cell = None;
                    self.report_mouse(MouseAction::Press(button), event.position);
                }
                // A release always follows a reported press, even if Shift
                // went down in between.
                PointerEventKind::Release { button, .. }
                    if self.mouse_button.is_some()
                        && self.mouse_button == mouse_button(*button) =>
                {
                    self.mouse_button = None;
                    self.mouse_cell = None;
                    self.report_mouse(
                        MouseAction::Release(mouse_button(*button).unwrap()),
                        event.position,
                    );
                }
                PointerEventKind::Axis {
                    vertical,
                    horizontal,
                    ..
                } if self.reporting_mouse() => {
                    self.report_wheel(vertical, horizontal, event.position)
                }
                PointerEventKind::Press {
                    button: BTN_LEFT,
                    serial,
                    ..
                } if self.hovered_link.is_some() => {
                    self.last_serial = *serial;
                    if let Some(link) = &self.hovered_link {
                        self.open_link(link);
                    }
                }
                PointerEventKind::Press {
                    button: BTN_LEFT,
                    serial,
                    time,
                } => {
                    self.last_serial = *serial;
                    let point = self.point_at(event.position);
                    // A fourth click starts over as a single click.
                    let clicks = match self.last_click {
                        Some((last, at, n))
                            if at == point && time.wrapping_sub(last) <= DOUBLE_CLICK_MS =>
                        {
                            n % 3 + 1
                        }
                        _ => 1,
                    };
                    self.last_click = Some((*time, point, clicks));
                    let screen = self.term.screen();
                    let selection = match clicks {
                        2 => self
                            .output_selection(point)
                            .or_else(|| Selection::word(screen, point)),
                        3 => Some(Selection::line(screen, point)),
                        _ => None,
                    };
                    self.selection = Some(selection.unwrap_or_else(|| Selection::new(point)));
                    self.selecting = true;
                    self.request_redraw();
                }
                PointerEventKind::Release {
                    button: BTN_LEFT, ..
                } => {
                    self.selecting = false;
                    if self.selection.is_some_and(|s| s.is_empty()) {
                        self.selection = None;
                    }
                    self.set_primary();
                    self.request_redraw();
                }
                PointerEventKind::Press {
                    button: BTN_MIDDLE,
                    serial,
                    ..
                } => {
                    self.last_serial = *serial;
                    self.paste_primary();
                }
                PointerEventKind::Axis { vertical, .. } => self.scroll(vertical),
                _ => {}
            }
        }
    }
}

/// Calls `apply` with a region covering any surface size; compositors clip
/// it to the surface, so it never needs updating on resize.
fn set_whole_region(
    compositor: &CompositorState,
    apply: impl FnOnce(Option<&wl_region::WlRegion>),
) {
    match Region::new(compositor) {
        Ok(region) => {
            region.add(0, 0, i32::MAX, i32::MAX);
            apply(Some(region.wl_region()));
        }
        Err(e) => log::warn!("cannot create region: {e}"),
    }
}

impl BackgroundEffectHandler for App {
    fn background_effect_state(&mut self) -> &mut BackgroundEffectState {
        &mut self.background_effect
    }

    fn update_capabilities(&mut self) {
        if !self.config.window.blur || self.blur.is_some() {
            return;
        }
        let supported = self
            .background_effect
            .capabilities()
            .is_some_and(|c| c.contains(EffectCapability::Blur));
        if !supported {
            log::warn!("blur requested, but the compositor cannot blur");
            return;
        }
        let surface = self.window.wl_surface().clone();
        match self
            .background_effect
            .get_background_effect(&surface, &self.qh)
        {
            Ok(effect) => {
                set_whole_region(&self.compositor, |region| effect.set_blur_region(region));
                self.blur = Some(effect);
                // The blur region applies on the next commit, i.e. frame.
                self.request_redraw();
            }
            Err(e) => log::warn!("cannot enable blur: {e}"),
        }
    }
}

/// The reportable button for a Linux input button code.
fn mouse_button(code: u32) -> Option<MouseButton> {
    match code {
        BTN_LEFT => Some(MouseButton::Left),
        BTN_MIDDLE => Some(MouseButton::Middle),
        BTN_RIGHT => Some(MouseButton::Right),
        _ => None,
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &wl_surface::WlSurface,
    ) {
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}

    // The offer is read lazily, when the user pastes.
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl ActivationHandler for App {
    type RequestUdata = ();

    fn new_token(&mut self, token: String, _: &RequestData<()>) {
        if let Some(activation) = &self.activation {
            activation.activate::<App>(self.window.wl_surface(), token);
        }
    }
}

impl PrimarySelectionDeviceHandler for App {
    // The offer is read lazily, on middle click.
    fn selection(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpPrimarySelectionDeviceV1,
    ) {
    }
}

impl PrimarySelectionSourceHandler for App {
    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
        _: String,
        pipe: WritePipe,
    ) {
        if let Some((_, text)) = self
            .primary_source
            .as_ref()
            .filter(|(s, _)| s.inner() == source)
        {
            write_in_background(pipe, text.clone());
        }
    }

    fn cancelled(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
    ) {
        if self
            .primary_source
            .as_ref()
            .is_some_and(|(s, _)| s.inner() == source)
        {
            self.primary_source = None;
        }
    }
}

/// Writes selection data to a reader, which may be slow (or be us), without
/// blocking the event loop.
fn write_in_background(mut pipe: WritePipe, text: String) {
    std::thread::spawn(move || {
        if let Err(e) = pipe.write_all(text.as_bytes()) {
            log::debug!("selection write: {e}");
        }
    });
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &WlDataSource,
        _: String,
        pipe: WritePipe,
    ) {
        if let Some((_, text)) = self
            .copy_source
            .as_ref()
            .filter(|(s, _)| s.inner() == source)
        {
            write_in_background(pipe, text.clone());
        }
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self
            .copy_source
            .as_ref()
            .is_some_and(|(s, _)| s.inner() == source)
        {
            self.copy_source = None;
        }
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

/// User data for the viewport and fractional-scale objects.
struct ScaleData;

impl Dispatch2<WpViewporter, App> for ScaleData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpViewport, App> for ScaleData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpFractionalScaleManagerV1, App> for ScaleData {
    fn event(
        &self,
        _: &mut App,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpFractionalScaleV1, App> for ScaleData {
    fn event(
        &self,
        app: &mut App,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            // In 120ths.
            app.set_scale(f64::from(scale) / 120.0);
        }
    }
}

/// User data for the text-input manager global.
struct TextInputManagerData;

impl Dispatch2<ZwpTextInputManagerV3, App> for TextInputManagerData {
    fn event(
        &self,
        _: &mut App,
        _: &ZwpTextInputManagerV3,
        _: <ZwpTextInputManagerV3 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        // The manager has no events.
    }
}

/// User data for a seat's text-input object.
struct TextInputData;

impl Dispatch2<ZwpTextInputV3, App> for TextInputData {
    fn event(
        &self,
        app: &mut App,
        text_input: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        app.text_input_event(text_input, event);
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
delegate_dispatch2!(App);
