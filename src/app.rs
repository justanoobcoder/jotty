//! The Wayland application: window, input, clipboard, input method and the
//! event loop tying them to the terminal and the renderer.

use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Write};

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
use smithay_client_toolkit::data_device_manager::{DataDeviceManagerState, WritePipe};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::generic::Generic;
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
use crate::pty::{Pty, WindowSize};
use crate::renderer::{Frame, Renderer, Theme, gl, software};
use crate::selection::{Point, Selection, Unit};
use crate::terminal::{MouseMode, Terminal};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// Clicks on the same cell within this many milliseconds of each other make
/// a double or triple click.
const DOUBLE_CLICK_MS: u32 = 400;
/// Read at most this much child output before drawing, to stay responsive.
const READ_BUDGET: usize = 1 << 20;
const PASTE_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

pub struct App {
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    data_device_manager: Option<DataDeviceManagerState>,
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

    /// Window size in logical pixels and the integer buffer scale.
    size: (u32, u32),
    scale: u32,
    font_points: f32,
    configured: bool,
    /// The screen changed and needs drawing.
    dirty: bool,
    /// A frame callback is outstanding; drawing waits for it.
    frame_pending: bool,
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
    /// Time, cell and count (1-3) of the last left click, to detect double
    /// and triple clicks.
    last_click: Option<(u32, Point, u8)>,
    data_device: Option<DataDevice>,
    copy_source: Option<(CopyPasteSource, String)>,

    text_input: Option<ZwpTextInputV3>,
    ime: ImeState,
    ime_caused_change: bool,
    ime_cursor_rect: (i32, i32, i32, i32),
}

pub fn run(config: Config, command: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
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
    window.commit();

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
    let cursor = config.colors.cursor();
    term.screen_mut()
        .set_default_cursor_color((cursor.0, cursor.1, cursor.2));
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
        scale: 1,
        font_points,
        configured: false,
        dirty: true,
        frame_pending: false,
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
        last_click: None,
        data_device: None,
        copy_source: None,
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
        (self.size.0 * self.scale, self.size.1 * self.scale)
    }

    fn padding(&self) -> u32 {
        self.config.window.padding * self.scale
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
        // The bell is not implemented yet; drop it.
        screen.take_bell();
        let dirty = screen.take_dirty();
        if !responses.is_empty() {
            self.send(&responses);
        }
        if let Some(title) = title {
            self.window.set_title(title);
        }
        if dirty {
            // Selections refer to screen contents that may have moved.
            self.selection = None;
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
        if self.configured && !self.frame_pending {
            self.draw();
        }
    }

    fn draw(&mut self) {
        self.dirty = false;
        let surface = self.window.wl_surface().clone();
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        self.frame_pending = true;

        let pad = self.padding();
        let frame = Frame {
            screen: self.term.screen(),
            selection: self.selection.as_ref(),
            preedit: self.ime.preedit.as_ref(),
            focused: self.focused,
            origin: (pad, pad),
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
        }
        if self.term.screen_mut().take_dirty() {
            self.request_redraw();
        }
        true
    }

    /// The visible cell under a surface-local position, as (column, row).
    fn cell_at(&self, (x, y): (f64, f64)) -> (usize, usize) {
        let scale = f64::from(self.scale);
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
        let cell_height = f64::from(self.renderer.cell().height) / f64::from(self.scale);
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
        let mime = offer.with_mime_types(|offered| {
            PASTE_MIMES
                .iter()
                .find(|m| offered.iter().any(|o| o == *m))
                .map(|m| m.to_string())
        });
        let Some(mime) = mime else {
            return;
        };
        let pipe = match offer.receive(mime) {
            Ok(pipe) => pipe,
            Err(e) => {
                log::warn!("cannot read clipboard: {e}");
                return;
            }
        };
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
                if !bytes.is_empty() {
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
        let (pad, scale) = (self.padding(), self.scale);
        let rect = (
            ((pad + col as u32 * cell.width) / scale) as i32,
            ((pad + row as u32 * cell.height) / scale) as i32,
            (cell.width / scale).max(1) as i32,
            (cell.height / scale).max(1) as i32,
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
        let scale = factor.max(1) as u32;
        if scale == self.scale {
            return;
        }
        log::info!("scale factor {scale}");
        self.scale = scale;
        self.window.wl_surface().set_buffer_scale(scale as i32);
        self.renderer
            .set_font_size(points_to_px(self.font_points, scale as f32));
        self.apply_size();
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
            self.draw();
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
            match &event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(pointer) = &self.pointer
                        && let Err(e) = pointer.set_cursor(conn, CursorIcon::Text)
                    {
                        log::debug!("cannot set cursor: {e}");
                    }
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
                    self.request_redraw();
                }
                PointerEventKind::Press {
                    button: BTN_MIDDLE,
                    serial,
                    ..
                } => {
                    self.last_serial = *serial;
                    self.paste();
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
        mut pipe: WritePipe,
    ) {
        let Some((_, text)) = self
            .copy_source
            .as_ref()
            .filter(|(s, _)| s.inner() == source)
        else {
            return;
        };
        let text = text.clone();
        // The reader may be slow (or be us); never block the event loop.
        std::thread::spawn(move || {
            if let Err(e) = pipe.write_all(text.as_bytes()) {
                log::debug!("clipboard write: {e}");
            }
        });
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
