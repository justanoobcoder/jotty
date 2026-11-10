//! The Wayland application: window, input, clipboard, input method and the
//! event loop tying them to the terminal and the renderer.

use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::rc::Rc;
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
use smithay_client_toolkit::reexports::calloop::channel;
use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
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
use wayland_client::globals;
use xkbcommon::xkb;
use wayland_client::protocol::wl_data_device::WlDataDevice;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_data_source::WlDataSource;
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, Proxy, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ChangeCause, ContentHint, ContentPurpose, ZwpTextInputV3,
};

use crate::config::{Config, RendererKind};
use crate::font::{self, Fonts};
use crate::ime::{Content, ImeState, Surrounding};
use crate::input::{self, Key, KeyAction, KeyModes, MouseAction, MouseButton, MouseEvent};
use crate::instance::{self, Client, DecodeError, Received, Reply, Server};
use crate::keybinds::{Action, Wheel};
use crate::links::{Detector, Link};
use crate::notify::{self, Bus, RateLimit};
use crate::pty::{Pty, WindowSize};
use crate::renderer::{Frame, Renderer, Theme, gl, software};
use crate::search::{self, Direction};
use crate::selection::{Point, Selection, Unit};
use crate::terminal::{Flags, MouseMode, Notification, Terminal};
use crate::vi::{self, ViMode};
use crate::watch;
use smithay_client_toolkit::seat::pointer::AxisScroll;
use std::process::{Command, Stdio};
use wayland_client::protocol::wl_keyboard::WlKeyboard;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// A scrollback search: the query and the current match.
struct Search {
    query: String,
    found: Option<(Point, Point)>,
    /// Which way typing searches: up the scrollback (`?`, the `search`
    /// binding) or down (`/` in vi mode).
    direction: Direction,
    /// Where the search starts: the bottom, or the vi cursor.
    origin: Point,
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
/// How long the child gets to exit after hanging up, for its exit status.
const CHILD_EXIT_WAIT: Duration = Duration::from_millis(100);
/// The most clipboard data pasted; a client that keeps sending gets dropped.
const MAX_PASTE: usize = 64 << 20;
/// How long a client asking for the selection may take nothing.
const SELECTION_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait after a frame failed to draw, and how many failures in a
/// row close the window.
const DRAW_RETRY: Duration = Duration::from_millis(200);
const MAX_DRAW_FAILURES: u32 = 50;
const PASTE_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// The preferred text type among those a paste offer has.
fn paste_mime(offered: &[String]) -> Option<String> {
    PASTE_MIMES
        .iter()
        .find(|m| offered.iter().any(|o| o == *m))
        .map(|m| m.to_string())
}

/// Parts of a scroll step not used yet, per direction: a touchpad's diagonal
/// movement must not add up to a step in either.
#[derive(Default)]
struct Accumulated {
    vertical: f64,
    horizontal: f64,
}

#[derive(Clone, Copy)]
enum Axis {
    Vertical,
    Horizontal,
}

/// How long to wait after failing to reach the session bus before trying
/// again, as connecting takes a thread and may take seconds.
const BUS_RETRY: Duration = Duration::from_secs(30);
/// Notifications kept while the session bus connects.
const MAX_QUEUED_NOTIFICATIONS: usize = 4;

/// Getting notifications to the session bus, around the bus itself.
struct Notifier {
    limit: RateLimit,
    /// A thread is connecting to the session bus.
    connecting: bool,
    /// No connecting before this, after a failure.
    retry_at: Option<Instant>,
    /// The window, title and body of notifications waiting for the
    /// connection.
    queued: Vec<(WindowId, String, String)>,
    /// A loop source is waiting for the bus to take queued messages.
    write_queued: bool,
}

impl Notifier {
    fn new() -> Self {
        Notifier {
            limit: RateLimit::new(Instant::now()),
            connecting: false,
            retry_at: None,
            queued: Vec::new(),
            write_queued: false,
        }
    }
}

/// Identifies a window, for loop sources and notifications that refer back
/// to it after it may have closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowId(u32);

/// What a new window runs, and where.
#[derive(Debug, Default)]
pub struct Spawn {
    /// Program and arguments to run instead of the shell.
    pub command: Option<Vec<OsString>>,
    /// The child's directory, instead of jotty's.
    pub cwd: Option<PathBuf>,
    /// The child's environment, instead of jotty's.
    pub env: Option<Vec<(OsString, OsString)>>,
    /// An xdg-activation token from whoever asked for the window, to give
    /// it focus.
    pub activation_token: Option<String>,
}

/// Something a window asks of the whole application, done after the event
/// that asked for it.
enum Request {
    ReloadConfig,
    NewWindow(Spawn),
}

/// The application: what all windows share, and the windows.
pub struct App {
    shared: Shared,
    windows: Vec<TermWindow>,
    /// The exit code of the last window's child, which becomes jotty's.
    exit_code: u8,
}

/// The Wayland globals, the seat's devices and the state that goes with
/// them, the config, and the session bus: what all windows share.
pub struct Shared {
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    conn: Connection,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    xdg_shell: XdgShell,
    shm: Shm,
    /// Both are needed for fractional scaling.
    viewporter: Option<WpViewporter>,
    fractional_manager: Option<WpFractionalScaleManagerV1>,
    data_device_manager: Option<DataDeviceManagerState>,
    primary_selection_manager: Option<PrimarySelectionManagerState>,
    /// xdg-activation, to mark the window urgent when the bell rings.
    activation: Option<ActivationState>,
    /// The session bus, for desktop notifications; connected on the first
    /// one, with the loop source that reads it.
    bus: Option<(Bus, RegistrationToken)>,
    notifier: Notifier,
    text_input_manager: Option<ZwpTextInputManagerV3>,
    background_effect: BackgroundEffectState,

    config: Rc<Config>,
    /// The `--config` file, reloaded when it changes.
    config_path: Option<PathBuf>,
    /// A reload is scheduled (changes come in bursts while a file is saved).
    reload_pending: bool,
    links: Detector,
    /// What windows asked for, done once the current event is handled.
    requests: Vec<Request>,
    next_window: u32,

    keyboard: Option<WlKeyboard>,
    /// The window with keyboard focus.
    keyboard_focus: Option<WindowId>,
    modifiers: Modifiers,
    /// Our own copy of the keyboard's keymap, to find what a key produces
    /// without modifiers (for key bindings), and the active layout.
    keymap: Option<xkb::Keymap>,
    layout: u32,
    /// Serial of the latest input event, for clipboard requests.
    last_serial: u32,
    /// Keys whose press went to the program, by key code, so that their
    /// release goes there too (kitty keyboard protocol).
    sent_keys: HashSet<u32>,
    pointer: Option<ThemedPointer>,
    data_device: Option<DataDevice>,
    copy_source: Option<(CopyPasteSource, String)>,
    /// The primary selection: set by selecting text, pasted by middle click.
    primary_device: Option<PrimarySelectionDevice>,
    primary_source: Option<(PrimarySelectionSource, String)>,
    text_input: Option<ZwpTextInputV3>,
    /// The window the input method is working in.
    ime_focus: Option<WindowId>,
}

/// The state of one terminal window: its Wayland objects, the terminal and
/// the child behind it, input, selection and rendering.
pub struct TermWindow {
    id: WindowId,
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    config: Rc<Config>,
    /// Frames in a row that failed to draw.
    draw_failures: u32,
    /// Present once the compositor agreed to blur behind the window.
    blur: Option<ExtBackgroundEffectSurfaceV1>,

    // Declared before `window` so the EGL surface is destroyed before the
    // wl_surface it renders to.
    renderer: Renderer,
    window: Window,
    term: Terminal,
    pty: Pty,
    /// The loop source reading the pty, removed when the window closes.
    pty_source: Option<RegistrationToken>,
    pty_write_queued: bool,

    /// Window size in logical pixels and the scale factor, which is
    /// fractional with wp-fractional-scale.
    size: (u32, u32),
    scale: f64,
    /// With fractional scaling, the buffer is drawn at the scaled size and
    /// the viewport maps it back to the logical size.
    viewport: Option<WpViewport>,
    fractional_scale: Option<WpFractionalScaleV1>,
    font_points: f32,
    configured: bool,
    /// The screen changed and needs drawing.
    dirty: bool,
    /// A frame callback is outstanding; drawing waits for it.
    frame_pending: bool,
    /// When the program started a synchronized update that is still open.
    /// Which one, by its number, and whether it has taken too long.
    sync_hold: Option<(u64, bool)>,
    /// The window is to close.
    exit: bool,
    /// The child's side of the pty is gone.
    pty_closed: bool,

    focused: bool,
    pointer_pos: (f64, f64),
    scroll_accum: Accumulated,
    /// Button held down while reporting to the program, if any.
    held_buttons: Vec<MouseButton>,
    /// Cell of the last reported pointer event, to report motion once per cell.
    mouse_cell: Option<(usize, usize)>,
    selection: Option<Selection>,
    selecting: bool,
    /// Scrollback search in progress.
    search: Option<Search>,
    /// The last search's query and direction, for `n` and `N` in vi mode.
    last_search: Option<(String, Direction)>,
    /// Vi mode, when on: a keyboard cursor over the screen and scrollback.
    vi: Option<ViMode>,
    /// The link under the pointer while Ctrl is held, drawn underlined.
    hovered_link: Option<Link>,
    /// The cell the pointer was last checked for a link in.
    hover_cell: Option<Point>,
    /// When a link was last clicked.
    last_link_click: Option<u32>,
    pointer_inside: bool,
    /// A cell marked by Shift+click; the next Shift+click selects from it.
    shift_mark: Option<Point>,
    /// The current left press was a Shift+click (it marks a cell unless
    /// it becomes a drag).
    shift_press: bool,
    /// Time, cell and count (1-3) of the last left click, to detect double
    /// and triple clicks.
    last_click: Option<(u32, Point, u8)>,

    /// The seat's text input, while the input method works in this window.
    text_input: Option<ZwpTextInputV3>,
    ime: ImeState,
    ime_caused_change: bool,
    ime_cursor_rect: (i32, i32, i32, i32),
}

/// Runs the first window, and the ones opened from it, until all are
/// closed. `config_path` is the config file given on the command line. With
/// a `server`, other jottys can hand their windows over to this one.
pub fn run(
    config: Config,
    config_path: Option<PathBuf>,
    spawn: Spawn,
    server: Option<Server>,
) -> Result<u8, Box<dyn Error>> {
    let conn =
        Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let (globals, event_queue) = globals::registry_queue_init::<App>(&conn)?;
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
    // Fractional scaling needs both protocols; otherwise the scale is the
    // integer buffer scale.
    let viewporter = globals
        .bind::<WpViewporter, _, _>(&qh, 1..=1, ScaleData)
        .ok();
    let fractional_manager = globals
        .bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ScaleData)
        .ok();
    if viewporter.is_none() || fractional_manager.is_none() {
        log::info!("no fractional scaling (wp-fractional-scale or wp-viewporter missing)");
    }
    let links = links_detector(&config);

    let mut app = App {
        shared: Shared {
            qh: qh.clone(),
            loop_handle,
            conn: conn.clone(),
            registry_state: RegistryState::new(&globals),
            seat_state: SeatState::new(&globals, &qh),
            output_state: OutputState::new(&globals, &qh),
            compositor,
            xdg_shell,
            shm,
            viewporter,
            fractional_manager,
            data_device_manager,
            primary_selection_manager,
            activation,
            bus: None,
            notifier: Notifier::new(),
            text_input_manager,
            background_effect,
            config: Rc::new(config),
            config_path,
            reload_pending: false,
            links,
            requests: Vec::new(),
            next_window: 0,
            keyboard: None,
            keyboard_focus: None,
            modifiers: Modifiers::default(),
            keymap: None,
            layout: 0,
            last_serial: 0,
            sent_keys: HashSet::new(),
            pointer: None,
            data_device: None,
            copy_source: None,
            primary_device: None,
            primary_source: None,
            text_input: None,
            ime_focus: None,
        },
        windows: Vec::new(),
        exit_code: 0,
    };
    app.open_window(spawn)?;
    if let Some(server) = &server {
        app.serve(server);
    }

    app.watch_config();
    // SCTK reports only seats added later through `new_seat`.
    let seats: Vec<_> = app.shared.seat_state.seats().collect();
    for seat in seats {
        app.new_seat(&conn, &qh, seat);
    }

    while !app.windows.is_empty() {
        event_loop.dispatch(None, &mut app)?;
        app.after_dispatch();
    }
    Ok(app.exit_code)
}

/// The link detector for the file kinds and URL patterns in `config`.
fn links_detector(config: &Config) -> Detector {
    let kinds: Vec<Vec<String>> = config
        .files
        .0
        .values()
        .map(|k| k.extensions.clone())
        .collect();
    Detector::try_new(&kinds, &config.url.regex).unwrap_or_else(|e| {
        log::error!("invalid url.regex, ignoring it: {e}");
        Detector::try_new(&kinds, &[]).expect("the built-in patterns are valid")
    })
}

/// The exit code that reports how the child ended, as a shell would.
fn exit_code(status: ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => code as u8,
        (None, Some(signal)) => 128u8.wrapping_add(signal as u8),
        (None, None) => 1,
    }
}

impl TermWindow {
    /// Creates a window and starts its child.
    fn open(shared: &mut Shared, id: WindowId, spawn: Spawn) -> Result<Self, Box<dyn Error>> {
        let config = Rc::clone(&shared.config);
        let qh = &shared.qh;
        let load_fonts = || {
            Fonts::try_new(
                &config.font.family,
                &config.font.fallback,
                font::points_to_px(config.font.size, 1.0),
            )
        };
        let fonts = load_fonts()?;
        let cell = fonts.metrics;
        let pad = config.window.padding;
        let cols = u32::from(config.window.columns);
        let lines = u32::from(config.window.lines);
        let size = (cols * cell.width + 2 * pad, lines * cell.height + 2 * pad);

        let surface = shared.compositor.create_surface(qh);
        let window = shared
            .xdg_shell
            .create_window(surface, WindowDecorations::RequestServer, qh);
        window.set_title(config.window.title.clone());
        window.set_app_id(config.window.app_id.clone());
        window.set_min_size(Some((cell.width * 2 + 2 * pad, cell.height + 2 * pad)));
        if config.window.opacity >= 1.0 {
            // Lets the compositor skip drawing what is behind the window.
            set_whole_region(&shared.compositor, |region| {
                window.wl_surface().set_opaque_region(region)
            });
        }
        let (viewport, fractional_scale) = match (&shared.viewporter, &shared.fractional_manager) {
            (Some(viewporter), Some(manager)) => (
                Some(viewporter.get_viewport(window.wl_surface(), qh, ScaleData)),
                Some(manager.get_fractional_scale(
                    window.wl_surface(),
                    qh,
                    FractionalScaleData(id),
                )),
            ),
            _ => (None, None),
        };
        window.commit();

        let theme = Theme::from_config(&config);
        let mut fonts = Some(fonts);
        let gpu = match config.window.renderer {
            // SAFETY: the display outlives the renderer (the event loop holds a
            // connection until after `App` is dropped), and `TermWindow` drops
            // the renderer before the window.
            RendererKind::Gpu => match unsafe {
                gl::Renderer::try_new(
                    shared.conn.backend().display_ptr().cast(),
                    window.wl_surface().id(),
                    size,
                    fonts.take().expect("fonts are taken only here"),
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
            None => Renderer::Cpu(Box::new(software::Renderer::try_new(
                &shared.shm,
                window.wl_surface().clone(),
                size,
                fonts.map_or_else(load_fonts, Ok)?,
                theme,
            )?)),
        };

        let (program, args) = match spawn.command {
            Some(mut command) => {
                let program = command.remove(0);
                (program, command)
            }
            None => (
                config
                    .terminal
                    .shell
                    .clone()
                    .unwrap_or_else(crate::pty::default_shell)
                    .into(),
                Vec::new(),
            ),
        };
        // The pixel size of the grid, from which programs work out the cell
        // size for images.
        let window_size = WindowSize {
            cols: cols as u16,
            rows: lines as u16,
            width: (cols * cell.width).min(u16::MAX.into()) as u16,
            height: (lines * cell.height).min(u16::MAX.into()) as u16,
        };
        if let (Some(activation), Some(token)) = (&shared.activation, spawn.activation_token) {
            activation.activate::<App>(window.wl_surface(), token);
        }
        let term = crate::terminfo::resolve(&config.terminal.term, &crate::terminfo::search_dirs());
        log::info!("TERM={} (terminfo: {:?})", term.name, term.terminfo);
        let pty = Pty::spawn(
            &program,
            &args,
            &term,
            window_size,
            spawn.cwd.as_deref(),
            spawn.env.as_deref(),
        )
        .map_err(|e| format!("cannot run {program:?}: {e}"))?;
        let reader = File::from(pty.fd().try_clone_to_owned()?);
        let pty_source = shared
            .loop_handle
            .insert_source(
                Generic::new(reader, Interest::READ, Mode::Level),
                move |_, _, app| {
                    let Some((shared, window)) = app.split(id) else {
                        return Ok(PostAction::Remove);
                    };
                    window.read_pty(shared);
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| format!("cannot watch the pty: {e}"))?;

        let mut term = Terminal::new(cols as usize, lines as usize, config.terminal.scrollback);
        term.screen_mut()
            .set_default_colors(Theme::from_config(&config).default_colors());
        term.screen_mut().set_cell_size(cell.width, cell.height);
        let font_points = config.font.size;
        Ok(TermWindow {
            id,
            qh: qh.clone(),
            loop_handle: shared.loop_handle.clone(),
            config,
            draw_failures: 0,
            blur: None,
            renderer,
            window,
            term,
            pty,
            pty_source: Some(pty_source),
            pty_write_queued: false,
            size,
            scale: 1.0,
            viewport,
            fractional_scale,
            font_points,
            configured: false,
            dirty: true,
            frame_pending: false,
            sync_hold: None,
            exit: false,
            pty_closed: false,
            focused: false,
            pointer_pos: (0.0, 0.0),
            scroll_accum: Accumulated::default(),
            held_buttons: Vec::new(),
            mouse_cell: None,
            selection: None,
            selecting: false,
            search: None,
            last_search: None,
            vi: None,
            hovered_link: None,
            hover_cell: None,
            last_link_click: None,
            pointer_inside: false,
            last_click: None,
            shift_mark: None,
            shift_press: false,
            text_input: None,
            ime: ImeState::default(),
            ime_caused_change: false,
            ime_cursor_rect: (0, 0, 0, 0),
        })
    }

    /// How the child ended, if it did. When it hung up the pty it is about
    /// to exit, so that gets a moment.
    fn child_status(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + CHILD_EXIT_WAIT;
        loop {
            match self.pty.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if self.pty_closed && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => return None,
                Err(e) => {
                    log::debug!("cannot get the child's exit status: {e}");
                    return None;
                }
            }
        }
    }

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
            .set_font_size(font::points_to_px(self.font_points, scale as f32));
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
        let cell = self.renderer.cell();
        let screen = self.term.screen_mut();
        let same_grid = (screen.cols(), screen.rows()) == (cols, rows);
        screen.set_cell_size(cell.width, cell.height);
        screen.resize(cols, rows);
        // The pixel size of the grid, from which programs work out the
        // cell size for images.
        let (w, h) = (cols as u32 * cell.width, rows as u32 * cell.height);
        let size = WindowSize {
            cols: cols as u16,
            rows: rows as u16,
            width: w.min(u16::MAX.into()) as u16,
            height: h.min(u16::MAX.into()) as u16,
        };
        if let Err(e) = self.pty.resize(size) {
            log::warn!("cannot resize pty: {e}");
        }
        // Compositors also configure on focus and state changes; the text
        // only moves if the grid changes.
        if !same_grid {
            self.selection = None;
            self.shift_mark = None;
        }
        self.vi_refresh();
        self.request_redraw();
    }

    fn set_font_points(&mut self, points: f32) {
        self.font_points = points.clamp(4.0, 200.0);
        self.renderer
            .set_font_size(font::points_to_px(self.font_points, self.scale as f32));
        self.apply_size();
    }

    fn read_pty(&mut self, shared: &mut Shared) {
        // What the selection covers, to keep it while output goes elsewhere.
        let (scrolled_before, alt_before) = {
            let screen = self.term.screen();
            (screen.grid().scrolled, screen.is_alt_screen())
        };
        let selected = self.selection.map(|s| s.live_rows(self.term.screen()));
        let mut buf = [0u8; 1 << 16];
        let mut total = 0;
        while total < READ_BUDGET {
            match self.pty.read(&mut buf) {
                Ok(0) => {
                    self.exit = true;
                    self.pty_closed = true;
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
                    self.pty_closed = true;
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
        let notifications = screen.take_notifications();
        let dirty = screen.take_dirty();
        if !responses.is_empty() {
            let written = self.pty.write_response(&responses);
            self.finish_send(written);
        }
        if let Some(title) = title {
            self.window.set_title(title);
        }
        if bell {
            self.ring_bell(shared);
        }
        for notification in notifications {
            self.show_notification(shared, notification);
        }
        if dirty {
            if self.hovered_link.is_some() {
                self.update_hovered_link(shared);
            }
            let screen = self.term.screen();
            let delta = screen.grid().scrolled.wrapping_sub(scrolled_before) as isize;
            // Another buffer means other text; history and alternate
            // screens never share lines.
            let same_buffer = alt_before == screen.is_alt_screen();
            if let (Some(selection), Some(rows)) = (&mut self.selection, selected) {
                let keep = same_buffer && selection.follow_output(screen, &rows, delta);
                if !keep {
                    self.selection = None;
                    self.selecting = false;
                }
            }
            if let Some(vi) = &mut self.vi {
                if same_buffer {
                    vi.scroll(delta, screen);
                } else {
                    vi.move_to(vi.cursor, screen);
                }
            }
            if let Some(search) = &mut self.search {
                search.origin.line -= delta;
                if let Some((start, end)) = &mut search.found {
                    start.line -= delta;
                    end.line -= delta;
                }
            }
            if let Some(mark) = &mut self.shift_mark {
                mark.line -= delta;
                if !same_buffer || screen.row_at(mark.line).is_none() {
                    self.shift_mark = None;
                }
            }
            self.request_redraw();
        }
    }

    /// Sends input to the child, queueing what the pty cannot take yet.
    fn send(&mut self, bytes: &[u8]) {
        let written = self.pty.write(bytes);
        self.finish_send(written);
    }

    /// After queueing input: makes sure what the pty couldn't take yet gets
    /// written when it can.
    fn finish_send(&mut self, written: io::Result<()>) {
        if let Err(e) = written {
            log::warn!("cannot write to pty: {e}");
            self.pty.discard_pending();
            return;
        }
        if !self.pty.has_pending() || self.pty_write_queued {
            return;
        }
        let Ok(fd) = self.pty.fd().try_clone_to_owned() else {
            return;
        };
        let source = Generic::new(File::from(fd), Interest::WRITE, Mode::Level);
        let id = self.id;
        let inserted = self.loop_handle.insert_source(source, move |_, _, app| {
            let Some(window) = app.window_mut(id) else {
                return Ok(PostAction::Remove);
            };
            if let Err(e) = window.pty.flush() {
                // It won't get better, and a level-triggered source would
                // call back at once, forever.
                log::warn!("cannot write to pty: {e}");
                window.pty.discard_pending();
            }
            if window.pty.has_pending() {
                Ok(PostAction::Continue)
            } else {
                window.pty_write_queued = false;
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
        let screen = self.term.screen();
        if !screen.modes.synchronized {
            self.sync_hold = None;
            return false;
        }
        // By number: an update can end and another begin within one read.
        let update = screen.synchronized_updates();
        match self.sync_hold {
            Some((held, expired)) if held == update => return !expired,
            _ => self.sync_hold = Some((update, false)),
        }
        let timer = Timer::from_duration(SYNC_TIMEOUT);
        let id = self.id;
        let inserted = self.loop_handle.insert_source(timer, move |_, _, app| {
            if let Some(window) = app.window_mut(id)
                && let Some((held, expired)) = &mut window.sync_hold
                && *held == update
            {
                *expired = true;
                if window.dirty {
                    window.request_redraw();
                }
            }
            TimeoutAction::Drop
        });
        if let Err(e) = inserted {
            // Without a timer, nothing would ever end the hold.
            log::warn!("cannot start the synchronized-output timer: {e}");
            return false;
        }
        true
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
            mark: self.shift_mark,
            vi_cursor: self.vi.as_ref().map(|vi| vi.cursor),
            label: self.vi.as_ref().map(|vi| match vi.visual() {
                None => "VI",
                Some(vi::Visual::Char) => "VISUAL",
                Some(vi::Visual::Line) => "V-LINE",
                Some(vi::Visual::Block) => "V-BLOCK",
            }),
            search: search_bar.as_deref(),
        };
        match self.renderer.draw(&frame) {
            Ok(again) => {
                self.dirty |= again;
                self.draw_failures = 0;
            }
            Err(e) => self.draw_failed(&e),
        }
        self.update_text_input(/* force= */ false);
    }

    /// A frame couldn't be drawn. That may pass (a failed allocation under
    /// memory pressure), and closing the window kills whatever runs in it,
    /// so it takes many failures in a row to give up.
    fn draw_failed(&mut self, error: &str) {
        log::error!("{error}");
        // Nothing was committed, so no frame callback will come.
        self.frame_pending = false;
        self.draw_failures += 1;
        if self.draw_failures >= MAX_DRAW_FAILURES {
            log::error!("giving up after {MAX_DRAW_FAILURES} failed frames");
            self.exit = true;
            return;
        }
        let timer = Timer::from_duration(DRAW_RETRY);
        let id = self.id;
        let inserted = self.loop_handle.insert_source(timer, move |_, _, app| {
            if let Some(window) = app.window_mut(id) {
                window.request_redraw();
            }
            TimeoutAction::Drop
        });
        if let Err(e) = inserted {
            log::error!("cannot schedule a redraw: {e}");
            self.exit = true;
        }
    }

    fn key_modes(&self) -> KeyModes {
        let screen = self.term.screen();
        KeyModes {
            app_cursor: screen.modes.app_cursor,
            app_keypad: screen.modes.app_keypad,
            keyboard: screen.keyboard_flags(),
        }
    }

    fn handle_key(&mut self, shared: &mut Shared, event: KeyEvent, action: KeyAction) {
        if self.search.is_some() {
            return self.search_key(shared, &event);
        }
        if self.shift_mark.is_some() && event.keysym == Keysym::Escape {
            self.shift_mark = None;
            return self.request_redraw();
        }
        let base = shared.base_keysym(event.raw_code);
        if let Some(action) = self
            .config
            .keybinds
            .lookup(event.keysym, base, shared.modifiers)
            && self.run_action(shared, action)
        {
            return;
        }
        // In vi mode, keys move the vi cursor and never reach the program.
        if self.vi.is_some() {
            return self.vi_key(shared, &event, base);
        }
        let key = Key {
            keysym: event.keysym,
            base,
            text: event.utf8.as_deref(),
            action,
        };
        if let Some(bytes) = input::encode(&key, shared.modifiers, self.key_modes()) {
            shared.sent_keys.insert(event.raw_code);
            self.scroll_to_bottom();
            self.send(&bytes);
        }
    }

    /// Reports a key release to the program, if it got the press and asked
    /// for releases.
    fn release(&mut self, shared: &mut Shared, event: KeyEvent) {
        if !shared.sent_keys.remove(&event.raw_code) {
            return;
        }
        let key = Key {
            keysym: event.keysym,
            base: shared.base_keysym(event.raw_code),
            text: event.utf8.as_deref(),
            action: KeyAction::Release,
        };
        if let Some(bytes) = input::encode(&key, shared.modifiers, self.key_modes()) {
            self.send(&bytes);
        }
    }

    /// Handles a key while searching: it edits the query or moves between
    /// matches instead of going to the program.
    fn search_key(&mut self, shared: &mut Shared, event: &KeyEvent) {
        let ctrl = shared.modifiers.ctrl;
        match event.keysym {
            Keysym::Escape => {
                self.search = None;
                self.selection = None;
                self.vi_refresh();
            }
            Keysym::Return | Keysym::KP_Enter => {
                let Some(search) = self.search.take() else {
                    return;
                };
                if !search.query.is_empty() {
                    self.last_search = Some((search.query, search.direction));
                }
                // In vi mode, the cursor goes to the match.
                match (&mut self.vi, search.found) {
                    (Some(vi), Some((start, _))) => vi.move_to(start, self.term.screen()),
                    (Some(_), None) => {}
                    (None, _) => self.set_primary(shared),
                }
                self.vi_refresh();
            }
            Keysym::BackSpace => {
                let Some(search) = &mut self.search else {
                    return;
                };
                search.query.pop();
                let direction = search.direction;
                self.search_step(direction, /* inclusive= */ true);
            }
            Keysym::Up => self.search_step(Direction::Older, /* inclusive= */ false),
            Keysym::Down => self.search_step(Direction::Newer, /* inclusive= */ false),
            Keysym::r if ctrl => self.search_step(Direction::Older, /* inclusive= */ false),
            Keysym::s if ctrl => self.search_step(Direction::Newer, /* inclusive= */ false),
            _ if !ctrl && !shared.modifiers.alt => {
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
        let direction = search.direction;
        self.search_step(direction, /* inclusive= */ true);
    }

    /// Opens the search bar, searching in `direction` from the vi cursor,
    /// or from the bottom.
    fn start_search(&mut self, direction: Direction) {
        let screen = self.term.screen();
        let origin = self.vi.as_ref().map_or(
            Point {
                line: screen.rows() as isize - 1,
                col: usize::MAX,
            },
            |vi| vi.cursor,
        );
        self.search = Some(Search {
            query: String::new(),
            found: None,
            direction,
            origin,
        });
        self.request_redraw();
    }

    /// Moves to the next match in `direction` from the current one (or from
    /// where the search started), wrapping around at the ends, selecting it
    /// and scrolling it into view. `inclusive` keeps the current match if
    /// it still matches: the query changed.
    fn search_step(&mut self, direction: Direction, inclusive: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        let screen = self.term.screen();
        let (from, inclusive) = match search.found {
            Some(found) => (found.0, inclusive),
            None => (search.origin, false),
        };
        match search::find_wrapping(screen, &search.query, from, direction, inclusive) {
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

    /// Handles a key in vi mode.
    fn vi_key(&mut self, shared: &mut Shared, event: &KeyEvent, base: Option<Keysym>) {
        if shared.modifiers.alt || shared.modifiers.logo {
            return;
        }
        let key = match event.keysym {
            Keysym::Escape => vi::Key::Escape,
            Keysym::Left => vi::Key::Left,
            Keysym::Right => vi::Key::Right,
            Keysym::Up => vi::Key::Up,
            Keysym::Down => vi::Key::Down,
            Keysym::Home => vi::Key::Home,
            Keysym::End => vi::Key::End,
            Keysym::Page_Up => vi::Key::PageUp,
            Keysym::Page_Down => vi::Key::PageDown,
            _ if shared.modifiers.ctrl => match base.and_then(|k| k.key_char()) {
                Some(c) if c.is_ascii_alphabetic() || c == '[' => {
                    vi::Key::Ctrl(c.to_ascii_lowercase())
                }
                _ => return,
            },
            _ => {
                let mut chars = event.utf8.as_deref().unwrap_or("").chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => vi::Key::Char(c),
                    _ => return,
                }
            }
        };
        let Some(vi) = &mut self.vi else {
            return;
        };
        match vi.key(self.term.screen(), key) {
            vi::Outcome::Done => {}
            vi::Outcome::Ignored => return,
            vi::Outcome::Exit => {
                self.vi = None;
                self.selection = None;
                return self.request_redraw();
            }
            vi::Outcome::Yank(text) => self.copy_text(shared, text),
            vi::Outcome::Search(direction) => return self.start_search(direction),
            vi::Outcome::Next { reverse } => self.search_next(reverse),
        }
        self.vi_refresh();
    }

    /// `n` and `N`: moves the vi cursor to the next match of the last
    /// search, in its direction or reversed.
    fn search_next(&mut self, reverse: bool) {
        let (Some((query, direction)), Some(vi)) = (&self.last_search, &mut self.vi) else {
            return;
        };
        let direction = if reverse {
            direction.reversed()
        } else {
            *direction
        };
        let screen = self.term.screen();
        if let Some((start, _)) = search::find_wrapping(
            screen, query, vi.cursor, direction, /* inclusive= */ false,
        ) {
            vi.move_to(start, screen);
        }
    }

    /// After the vi cursor or the screen changed: selects what visual mode
    /// covers and keeps the cursor in view.
    fn vi_refresh(&mut self) {
        let Some(vi) = &mut self.vi else {
            return;
        };
        let screen = self.term.screen();
        vi.move_to(vi.cursor, screen);
        self.selection = vi.selection(screen);
        let line = vi.cursor.line;
        self.term.screen_mut().show_line(line);
        self.request_redraw();
    }

    fn scroll_to_bottom(&mut self) {
        if self.term.screen().display_offset() > 0 {
            self.term.screen_mut().reset_display();
            self.request_redraw();
        }
    }

    /// Runs a bound action. Returns false when the action declined the key,
    /// which then goes to the program as usual.
    fn run_action(&mut self, shared: &mut Shared, action: Action) -> bool {
        let page = self.term.screen().rows() as isize;
        match action {
            Action::Copy => self.copy(shared),
            Action::CopyOrInterrupt => {
                if !self.selection.is_some_and(|s| !s.is_empty()) {
                    return false;
                }
                self.copy(shared);
                // Cleared, so pressing the key again interrupts.
                self.selection = None;
                self.request_redraw();
            }
            Action::Paste => self.paste(shared),
            Action::ScrollLineUp => self.term.screen_mut().scroll_display(1),
            Action::ScrollLineDown => self.term.screen_mut().scroll_display(-1),
            Action::ScrollPageUp => self.term.screen_mut().scroll_display(page),
            Action::ScrollPageDown => self.term.screen_mut().scroll_display(-page),
            Action::ScrollToTop => self.term.screen_mut().scroll_display(isize::MAX),
            Action::ScrollToBottom => self.term.screen_mut().reset_display(),
            Action::FontLarger => {
                self.set_font_points(self.font_points + self.config.font.size_step)
            }
            Action::FontSmaller => {
                self.set_font_points(self.font_points - self.config.font.size_step)
            }
            Action::FontReset => self.set_font_points(self.config.font.size),
            Action::Search => self.start_search(Direction::Older),
            Action::ViMode => {
                if self.vi.take().is_some() {
                    self.selection = None;
                    self.request_redraw();
                } else {
                    let (row, col) = self.term.screen().cursor();
                    self.vi = Some(ViMode::new(Point {
                        line: row as isize,
                        col,
                    }));
                    self.selection = None;
                    self.vi_refresh();
                }
            }
            Action::NewWindow => self.new_window(shared),
            Action::ReloadConfig => shared.requests.push(Request::ReloadConfig),
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

    /// The first cell of the character at `p`: the second half of a wide
    /// character counts as the first.
    fn cell_start(&self, p: Point) -> Point {
        let spacer = self
            .term
            .screen()
            .row_at(p.line)
            .and_then(|r| r.cells().get(p.col))
            .is_some_and(|c| c.flags.contains(Flags::WIDE_SPACER));
        Point {
            col: if spacer {
                p.col.saturating_sub(1)
            } else {
                p.col
            },
            ..p
        }
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
    /// Ctrl+double click.
    fn output_selection(&self, point: Point) -> Option<Selection> {
        let screen = self.term.screen();
        let (first, last) = screen.command_output(point.line)?;
        let end = screen.row_at(last)?.cells().len().saturating_sub(1);
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
    fn update_hovered_link(&mut self, shared: &mut Shared) {
        let link = if shared.modifiers.ctrl && self.pointer_inside && !self.reporting_mouse(shared)
        {
            let cwd = std::cell::OnceCell::new();
            let resolve = |path: &str| {
                let path = match path.strip_prefix("~/") {
                    Some(rest) => std::env::home_dir()?.join(rest),
                    None => {
                        let path = Path::new(path);
                        cwd.get_or_init(|| self.pty.foreground_cwd())
                            .as_ref()
                            .map_or(path.to_path_buf(), |dir| dir.join(path))
                    }
                };
                path.is_file().then(|| path.to_string_lossy().into_owned())
            };
            shared
                .links
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
        if let Some(pointer) = &shared.pointer
            && let Err(e) = pointer.set_cursor(&shared.conn, icon)
        {
            log::debug!("cannot set cursor: {e}");
        }
        self.hovered_link = link;
        self.request_redraw();
    }

    /// Opens another window in the current directory of the program in
    /// the foreground.
    fn new_window(&self, shared: &mut Shared) {
        let cwd = self.pty.foreground_cwd();
        if let Some(dir) = &cwd {
            log::info!("opening a new window in {}", dir.display());
        }
        shared.requests.push(Request::NewWindow(Spawn {
            cwd,
            ..Spawn::default()
        }));
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
        if !crate::links::is_openable(target) {
            log::warn!("not opening {target:?}: the launcher would take it for an option");
            return;
        }
        log::info!("opening {target}");
        // Browsers print noise on startup (Chromium's "zygote" errors when
        // an instance is already running); only the exit status matters.
        let child = Command::new(program)
            .args(args)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
    fn reporting_mouse(&self, shared: &Shared) -> bool {
        self.term.screen().modes.mouse != MouseMode::Off && !shared.modifiers.shift
    }

    /// Sends a pointer event to the program, if its mode includes it.
    fn report_mouse(&mut self, shared: &mut Shared, action: MouseAction, pos: (f64, f64)) {
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
            mods: shared.modifiers,
        };
        if let Some(bytes) = input::encode_mouse(event, modes.mouse, modes.mouse_sgr) {
            self.send(&bytes);
        }
    }

    /// Converts a scroll event into whole steps, `per_notch` per wheel notch
    /// (touchpads scroll by pixels: one step per cell height).
    fn scroll_steps(&mut self, axis: &AxisScroll, direction: Axis, per_notch: f64) -> isize {
        let cell_height = f64::from(self.renderer.cell().height) / self.scale;
        let accum = match direction {
            Axis::Vertical => &mut self.scroll_accum.vertical,
            Axis::Horizontal => &mut self.scroll_accum.horizontal,
        };
        // The finger left the touchpad; a part of a step doesn't carry over.
        if axis.stop {
            *accum = 0.0;
        }
        *accum += if axis.value120 != 0 {
            f64::from(axis.value120) / 120.0 * per_notch
        } else if axis.discrete != 0 {
            f64::from(axis.discrete) * per_notch
        } else {
            axis.absolute / cell_height
        };
        let steps = accum.trunc();
        *accum -= steps;
        steps as isize
    }

    /// Runs the actions bound to the wheel with the held modifiers, or else
    /// reports it to the program or scrolls.
    fn wheel(
        &mut self,
        shared: &mut Shared,
        vertical: &AxisScroll,
        horizontal: &AxisScroll,
        pos: (f64, f64),
    ) {
        if self.run_wheel_bindings(shared, vertical, horizontal) {
            return;
        }
        if self.reporting_mouse(shared) {
            self.report_wheel(shared, vertical, horizontal, pos);
        } else {
            self.scroll(vertical);
        }
    }

    /// Runs the action bound to each direction the wheel turned in, once per
    /// notch. Returns whether any was bound.
    fn run_wheel_bindings(
        &mut self,
        shared: &mut Shared,
        vertical: &AxisScroll,
        horizontal: &AxisScroll,
    ) -> bool {
        let mut bound = false;
        for (axis, direction_of_axis, back, forward) in [
            (vertical, Axis::Vertical, Wheel::Up, Wheel::Down),
            (horizontal, Axis::Horizontal, Wheel::Left, Wheel::Right),
        ] {
            let direction = axis.value120.signum() + axis.discrete.signum();
            let wheel = match (direction, axis.absolute) {
                (d, _) if d < 0 => back,
                (d, _) if d > 0 => forward,
                (_, a) if a < 0.0 => back,
                (_, a) if a > 0.0 => forward,
                _ => continue,
            };
            let Some(action) = self.config.keybinds.lookup_wheel(wheel, shared.modifiers) else {
                continue;
            };
            bound = true;
            for _ in 0..self
                .scroll_steps(axis, direction_of_axis, 1.0)
                .unsigned_abs()
            {
                self.run_action(shared, action);
            }
        }
        bound
    }

    /// Reports wheel movement as button 64–67 presses, one per notch.
    fn report_wheel(
        &mut self,
        shared: &mut Shared,
        vertical: &AxisScroll,
        horizontal: &AxisScroll,
        pos: (f64, f64),
    ) {
        for (axis, direction, back, forward) in [
            (
                vertical,
                Axis::Vertical,
                MouseButton::WheelUp,
                MouseButton::WheelDown,
            ),
            (
                horizontal,
                Axis::Horizontal,
                MouseButton::WheelLeft,
                MouseButton::WheelRight,
            ),
        ] {
            if axis.is_none() {
                continue;
            }
            let steps = self.scroll_steps(axis, direction, 1.0);
            let button = if steps < 0 { back } else { forward };
            for _ in 0..steps.unsigned_abs() {
                self.mouse_cell = None;
                self.report_mouse(shared, MouseAction::Press(button), pos);
            }
        }
    }

    fn scroll(&mut self, axis: &AxisScroll) {
        let lines = self.scroll_steps(axis, Axis::Vertical, 3.0);
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

    fn copy(&mut self, shared: &mut Shared) {
        let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
            return;
        };
        self.copy_text(shared, selection.text(self.term.screen()));
    }

    /// Puts `text` on the clipboard.
    fn copy_text(&mut self, shared: &mut Shared, text: String) {
        let (Some(manager), Some(device)) = (&shared.data_device_manager, &shared.data_device)
        else {
            log::warn!("no clipboard support (wl_data_device_manager missing)");
            return;
        };
        if text.is_empty() {
            return;
        }
        let source = manager.create_copy_paste_source(&self.qh, PASTE_MIMES);
        source.set_selection(device, shared.last_serial);
        shared.copy_source = Some((source, text));
    }

    /// Offers the selection as the primary selection, which a middle click
    /// pastes (here or in other applications).
    fn set_primary(&mut self, shared: &mut Shared) {
        let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
            return;
        };
        let (Some(manager), Some(device)) =
            (&shared.primary_selection_manager, &shared.primary_device)
        else {
            return;
        };
        let text = selection.text(self.term.screen());
        if text.is_empty() {
            return;
        }
        let source = manager.create_selection_source(&self.qh, PASTE_MIMES);
        source.set_selection(device, shared.last_serial);
        shared.primary_source = Some((source, text));
    }

    /// Pastes the primary selection, or the clipboard where the compositor
    /// has no primary selection.
    fn paste_primary(&mut self, shared: &mut Shared) {
        if shared.primary_selection_manager.is_none() {
            return self.paste(shared);
        }
        if let Some((_, text)) = &shared.primary_source {
            let text = text.clone();
            return self.paste_text(&text);
        }
        let Some(offer) = shared
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

    fn paste(&mut self, shared: &mut Shared) {
        // Our own selection: skip the round trip through the compositor.
        if let Some((_, text)) = &shared.copy_source {
            let text = text.clone();
            self.paste_text(&text);
            return;
        }
        let Some(offer) = shared
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
        use std::io::Read;
        let mut data = Vec::new();
        let id = self.id;
        let inserted = self.loop_handle.insert_source(pipe, move |_, file, app| {
            // The window closed before the paste arrived.
            let Some(window) = app.window_mut(id) else {
                return PostAction::Remove;
            };
            // SAFETY: the file is only read, never closed or replaced.
            let file: &mut File = unsafe { file.get_mut() };
            let mut buf = [0u8; 1 << 14];
            match file.read(&mut buf) {
                Ok(0) => {
                    window.paste_text(&String::from_utf8_lossy(&data));
                    PostAction::Remove
                }
                Ok(n) if data.len() + n > MAX_PASTE => {
                    log::warn!("clipboard data over {MAX_PASTE} bytes; not pasting it");
                    data = Vec::new();
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
    fn ring_bell(&mut self, shared: &mut Shared) {
        if self.focused || !self.config.window.bell_urgent {
            return;
        }
        self.request_attention(shared);
    }

    fn request_attention(&mut self, shared: &mut Shared) {
        if let Some(activation) = &shared.activation {
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

    /// Shows a desktop notification, if the window isn't focused and the
    /// program isn't sending too many. Clicking it focuses the window.
    fn show_notification(&mut self, shared: &mut Shared, notification: Notification) {
        if self.focused || !self.config.window.notifications {
            return;
        }
        if !shared.notifier.limit.allow(Instant::now()) {
            log::debug!("too many notifications; dropping one");
            return;
        }
        let title = notification
            .title
            .as_deref()
            .or(self.term.screen().title())
            .unwrap_or(&self.config.window.title)
            .to_string();
        if shared.bus.is_some() {
            shared.send_notification(self.id, &title, &notification.body);
            return;
        }
        if shared.connect_bus() && shared.notifier.queued.len() < MAX_QUEUED_NOTIFICATIONS {
            shared
                .notifier
                .queued
                .push((self.id, title, notification.body));
        }
    }

    fn paste_text(&mut self, text: &str) {
        let bytes = input::encode_paste(text, self.term.screen().modes.bracketed_paste);
        self.scroll_to_bottom();
        self.send(&bytes);
    }

    /// Applies a reloaded config, `new`, that replaces `old`. `fonts` are
    /// the window's new fonts when the family changed.
    fn apply_config(
        &mut self,
        compositor: &CompositorState,
        old: &Config,
        new: &Rc<Config>,
        fonts: Option<Fonts>,
    ) {
        self.config = Rc::clone(new);
        let mut relayout = old.window.padding != new.window.padding;
        if let Some(fonts) = fonts {
            self.renderer.set_fonts(fonts);
            relayout = true;
        }
        if old.font.size != new.font.size {
            // A new configured size replaces any zooming.
            self.font_points = new.font.size;
            self.renderer
                .set_font_size(font::points_to_px(self.font_points, self.scale as f32));
            relayout = true;
        }

        let theme = Theme::from_config(new);
        self.term
            .screen_mut()
            .set_default_colors(theme.default_colors());
        self.renderer.set_theme(theme);

        if (old.window.opacity >= 1.0) != (new.window.opacity >= 1.0) {
            let surface = self.window.wl_surface();
            if new.window.opacity >= 1.0 {
                set_whole_region(compositor, |region| surface.set_opaque_region(region));
            } else {
                surface.set_opaque_region(None);
            }
        }
        if !new.window.blur
            && let Some(blur) = self.blur.take()
        {
            blur.destroy();
        }
        if (&old.url, &old.files) != (&new.url, &new.files) {
            self.hovered_link = None;
        }

        if relayout {
            self.apply_size();
        } else {
            self.request_redraw();
        }
    }

    /// An input method event for this window, which has the input method's
    /// focus (or just lost it).
    fn text_input_event(&mut self, text_input: &ZwpTextInputV3, event: zwp_text_input_v3::Event) {
        use zwp_text_input_v3::Event;
        match event {
            Event::Enter { .. } => {
                self.ime.active = true;
                text_input.enable();
                self.update_text_input(/* force= */ true);
            }
            Event::Leave { .. } => {
                text_input.disable();
                text_input.commit();
                self.ime.serial = self.ime.serial.wrapping_add(1);
                self.ime.reset();
                self.text_input = None;
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
                } else if self.vi.is_some() {
                    // Vi mode takes keys, not text, and the program isn't
                    // the one being typed to.
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
        // A password prompt (echo off) tells the input method to pass keys
        // through as typed. Inside a multiplexer this sees only its raw mode.
        // The search bar and vi mode take keys themselves, so they never are.
        let own_input = self.search.is_some() || self.vi.is_some();
        let content = Content::new(self.pty.password_input(), own_input);
        if !force
            && surrounding == self.ime.surrounding
            && rect == self.ime_cursor_rect
            && content == self.ime.content
        {
            return;
        }
        if force || content != self.ime.content {
            match content {
                Content::Password => text_input.set_content_type(
                    ContentHint::HiddenText | ContentHint::SensitiveData,
                    ContentPurpose::Password,
                ),
                Content::Terminal => {
                    text_input.set_content_type(ContentHint::None, ContentPurpose::Terminal)
                }
            }
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
        self.ime.content = content;
    }
}

impl Shared {
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

    /// Shows a notification for `window`, which clicking it focuses.
    fn send_notification(&mut self, window: WindowId, title: &str, body: &str) {
        let Some((bus, _)) = &mut self.bus else {
            return;
        };
        if let Err(e) = bus.notify(window.0, &self.config.window.app_id, title, body) {
            log::warn!("cannot show a notification: {e}");
            self.disconnect_bus();
            return;
        }
        self.watch_bus_writes();
    }

    /// Makes sure messages the bus couldn't take yet get written when it can.
    fn watch_bus_writes(&mut self) {
        let Some((bus, _)) = &self.bus else {
            return;
        };
        if !bus.has_outgoing() || self.notifier.write_queued {
            return;
        }
        let Ok(socket) = bus.socket() else {
            return;
        };
        let source = Generic::new(socket, Interest::WRITE, Mode::Level);
        let inserted = self.loop_handle.insert_source(source, |_, _, app| {
            let shared = &mut app.shared;
            let Some((bus, _)) = &mut shared.bus else {
                shared.notifier.write_queued = false;
                return Ok(PostAction::Remove);
            };
            if let Err(e) = bus.flush() {
                log::info!("lost the session bus: {e}");
                shared.disconnect_bus();
                shared.notifier.write_queued = false;
                return Ok(PostAction::Remove);
            }
            if bus.has_outgoing() {
                return Ok(PostAction::Continue);
            }
            shared.notifier.write_queued = false;
            Ok(PostAction::Remove)
        });
        self.notifier.write_queued = inserted.is_ok();
    }

    /// Starts connecting to the session bus on another thread, as it may
    /// take seconds. Returns whether a connection is on its way.
    fn connect_bus(&mut self) -> bool {
        if self.notifier.connecting {
            return true;
        }
        if self.notifier.retry_at.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        let (sender, receiver) = channel::channel();
        // The source goes away by itself once the thread drops the sender.
        let inserted = self.loop_handle.insert_source(receiver, |event, _, app| {
            if let channel::Event::Msg(result) = event {
                app.shared.bus_connected(result);
            }
        });
        if let Err(e) = inserted {
            log::warn!("cannot watch the session bus connection: {e}");
            return false;
        }
        let spawned = std::thread::Builder::new()
            .name("session-bus".into())
            .spawn(move || {
                // The window may be gone by now, and that's fine.
                let _ = sender.send(Bus::connect());
            });
        if let Err(e) = spawned {
            log::warn!("cannot connect to the session bus: {e}");
            self.notifier.retry_at = Some(Instant::now() + BUS_RETRY);
            return false;
        }
        self.notifier.connecting = true;
        true
    }

    /// The thread from `connect_bus` is done.
    fn bus_connected(&mut self, result: io::Result<Bus>) {
        self.notifier.connecting = false;
        let queued = std::mem::take(&mut self.notifier.queued);
        let registered = result.and_then(|bus| {
            let socket = bus.socket()?;
            let source = Generic::new(socket, Interest::READ, Mode::Level);
            let token = self
                .loop_handle
                .insert_source(source, |_, _, app| {
                    app.read_bus();
                    Ok(PostAction::Continue)
                })
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok((bus, token))
        });
        match registered {
            Ok(bus) => {
                self.notifier.retry_at = None;
                self.bus = Some(bus);
                self.watch_bus_writes();
                for (window, title, body) in queued {
                    self.send_notification(window, &title, &body);
                }
            }
            Err(e) => {
                log::warn!("cannot connect to the session bus for notifications: {e}");
                self.notifier.retry_at = Some(Instant::now() + BUS_RETRY);
            }
        }
    }

    fn disconnect_bus(&mut self) {
        if let Some((_, token)) = self.bus.take() {
            self.loop_handle.remove(token);
        }
    }
}

impl App {
    fn window_mut(&mut self, id: WindowId) -> Option<&mut TermWindow> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// A window and the shared state, to call the window's methods with.
    fn split(&mut self, id: WindowId) -> Option<(&mut Shared, &mut TermWindow)> {
        let window = self.windows.iter_mut().find(|w| w.id == id)?;
        Some((&mut self.shared, window))
    }

    /// The window of a surface and the shared state.
    fn split_by_surface(&mut self, surface: &WlSurface) -> Option<(&mut Shared, &mut TermWindow)> {
        let window = self
            .windows
            .iter_mut()
            .find(|w| w.window.wl_surface() == surface)?;
        Some((&mut self.shared, window))
    }

    /// The window with keyboard focus and the shared state.
    fn split_focused(&mut self) -> Option<(&mut Shared, &mut TermWindow)> {
        let id = self.shared.keyboard_focus?;
        self.split(id)
    }

    /// Opens a window running `spawn`'s command, or the shell.
    fn open_window(&mut self, spawn: Spawn) -> Result<WindowId, Box<dyn Error>> {
        let shared = &mut self.shared;
        let id = WindowId(shared.next_window);
        let window = TermWindow::open(shared, id, spawn)?;
        shared.next_window = shared.next_window.wrapping_add(1);
        self.windows.push(window);
        Ok(id)
    }

    /// Carries out what windows asked for, and closes the windows that are
    /// done.
    fn after_dispatch(&mut self) {
        for request in std::mem::take(&mut self.shared.requests) {
            match request {
                Request::ReloadConfig => self.reload_config(),
                Request::NewWindow(spawn) => {
                    if let Err(e) = self.open_window(spawn) {
                        log::warn!("cannot open a new window: {e}");
                    }
                }
            }
        }
        while let Some(i) = self.windows.iter().position(|w| w.exit) {
            let mut window = self.windows.remove(i);
            if self.windows.is_empty() {
                let status = window.child_status();
                if let Some(status) = status {
                    log::info!("child exited: {status}");
                }
                self.exit_code = status.map_or(0, exit_code);
            }
            self.close(window);
        }
    }

    /// Takes down a window that was removed from the list.
    fn close(&mut self, mut window: TermWindow) {
        let shared = &mut self.shared;
        for focus in [&mut shared.keyboard_focus, &mut shared.ime_focus] {
            if *focus == Some(window.id) {
                *focus = None;
            }
        }
        if let Some(token) = window.pty_source.take() {
            shared.loop_handle.remove(token);
        }
        if let Some(blur) = window.blur.take() {
            blur.destroy();
        }
        if let Some(viewport) = window.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional_scale) = window.fractional_scale.take() {
            fractional_scale.destroy();
        }
    }

    /// Opens the windows other jottys hand over to this one.
    fn serve(&mut self, server: &Server) {
        let listener = match server.listener() {
            Ok(listener) => listener,
            Err(e) => return log::warn!("cannot serve other jottys: {e}"),
        };
        let source = Generic::new(listener, Interest::READ, Mode::Level);
        let inserted = self
            .shared
            .loop_handle
            .insert_source(source, |_, listener, app| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => app.accept_client(stream),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            // Such as EMFILE; the connection waits in the
                            // backlog for the next try.
                            log::warn!("cannot accept a jotty: {e}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            });
        if let Err(e) = inserted {
            log::warn!("cannot serve other jottys: {e}");
        }
    }

    /// Reads the window a jotty asks for, opens it and answers.
    fn accept_client(&mut self, stream: std::os::unix::net::UnixStream) {
        let watched = Client::new(stream).and_then(|client| Ok((client.socket()?, client)));
        let (socket, mut client) = match watched {
            Ok(watched) => watched,
            Err(e) => return log::warn!("cannot read from a jotty: {e}"),
        };
        let source = Generic::new(socket, Interest::READ, Mode::Level);
        let inserted = self
            .shared
            .loop_handle
            .insert_source(source, move |_, _, app| {
                let reply = match client.read() {
                    Ok(Received::Incomplete) => return Ok(PostAction::Continue),
                    Ok(Received::Request(request)) => app.open_requested(request),
                    Ok(Received::Invalid(DecodeError::Version(_))) => Reply::VersionMismatch,
                    Ok(Received::Invalid(e)) => {
                        log::warn!("a jotty sent a bad request: {e}");
                        Reply::Failed(e.to_string())
                    }
                    Err(e) => {
                        log::debug!("a jotty went away before asking for a window: {e}");
                        return Ok(PostAction::Remove);
                    }
                };
                if let Err(e) = client.reply(&reply) {
                    log::debug!("cannot answer a jotty: {e}");
                }
                Ok(PostAction::Remove)
            });
        if let Err(e) = inserted {
            log::warn!("cannot read from a jotty: {e}");
        }
    }

    /// Opens the window another jotty asked for, with its directory and
    /// environment.
    fn open_requested(&mut self, request: instance::Request) -> Reply {
        let activation_token = request
            .env
            .iter()
            .find(|(name, _)| name == "XDG_ACTIVATION_TOKEN")
            .and_then(|(_, token)| token.to_str())
            .filter(|token| !token.is_empty())
            .map(String::from);
        let spawn = Spawn {
            command: request.command,
            cwd: Some(request.cwd),
            env: Some(request.env),
            activation_token,
        };
        match self.open_window(spawn) {
            Ok(_) => Reply::Opened,
            Err(e) => {
                log::warn!("cannot open the window a jotty asked for: {e}");
                Reply::Failed(e.to_string())
            }
        }
    }

    /// Hands a key to the window with keyboard focus.
    fn key(&mut self, event: KeyEvent, action: KeyAction) {
        if let Some((shared, window)) = self.split_focused() {
            window.handle_key(shared, event, action);
        }
    }

    /// Reloads the config when its file or the overrides change, or on
    /// SIGUSR1.
    fn watch_config(&mut self) {
        let path = self
            .shared
            .config_path
            .clone()
            .or_else(crate::config::default_path);
        let overrides = crate::config::overrides_path();
        // Created up front: inotify can't watch a directory that doesn't
        // exist yet, and `jotty config` would otherwise go unnoticed until
        // the next window.
        if let Some(dir) = overrides.as_deref().and_then(Path::parent)
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            log::info!("cannot create {}: {e}", dir.display());
        }
        for path in path.into_iter().chain(overrides) {
            self.watch_config_file(path);
        }
        match watch::sigusr1() {
            Ok(file) => {
                let source = Generic::new(file, Interest::READ, Mode::Level);
                let inserted = self
                    .shared
                    .loop_handle
                    .insert_source(source, |_, file, app| {
                        // SAFETY: the file is only read, never closed or replaced.
                        watch::drain(unsafe { file.get_mut() });
                        app.reload_config();
                        Ok(PostAction::Continue)
                    });
                if let Err(e) = inserted {
                    log::warn!("cannot handle SIGUSR1: {e}");
                }
            }
            Err(e) => log::warn!("cannot handle SIGUSR1: {e}"),
        }
    }

    /// Schedules a reload whenever the file at `path` changes.
    fn watch_config_file(&mut self, path: PathBuf) {
        if let Some(name) = path.file_name().map(|n| n.to_owned()) {
            match watch::watch_file(&path) {
                Ok(file) => {
                    let source = Generic::new(file, Interest::READ, Mode::Level);
                    let inserted =
                        self.shared
                            .loop_handle
                            .insert_source(source, move |_, file, app| {
                                // SAFETY: the file is only read, never closed or replaced.
                                if watch::changed(unsafe { file.get_mut() }, &name) {
                                    app.schedule_reload();
                                }
                                Ok(PostAction::Continue)
                            });
                    if let Err(e) = inserted {
                        log::warn!("cannot watch the config file: {e}");
                    }
                }
                // Typically: no config directory yet.
                Err(e) => log::info!("not watching {} for changes: {e}", path.display()),
            }
        }
    }

    /// Reloads shortly, once the burst of changes from saving a file is over.
    fn schedule_reload(&mut self) {
        if std::mem::replace(&mut self.shared.reload_pending, true) {
            return;
        }
        let timer = Timer::from_duration(Duration::from_millis(100));
        let inserted = self.shared.loop_handle.insert_source(timer, |_, _, app| {
            app.shared.reload_pending = false;
            app.reload_config();
            TimeoutAction::Drop
        });
        if let Err(e) = inserted {
            log::warn!("cannot schedule a config reload: {e}");
            self.shared.reload_pending = false;
        }
    }

    /// Reads the config file again and applies what can change in running
    /// windows. An invalid file is reported and the current config kept.
    fn reload_config(&mut self) {
        let shared = &mut self.shared;
        let mut new = match Config::load(shared.config_path.as_deref()) {
            Ok(config) => config,
            Err(e) => return log::error!("{e}; keeping the current configuration"),
        };
        if new == *shared.config {
            return;
        }
        log::info!("reloading the configuration");
        let old = Rc::clone(&shared.config);

        for (changed, what) in [
            (old.terminal.shell != new.terminal.shell, "terminal.shell"),
            (old.terminal.term != new.terminal.term, "terminal.term"),
            (
                old.terminal.scrollback != new.terminal.scrollback,
                "terminal.scrollback",
            ),
            (
                old.window.renderer != new.window.renderer,
                "window.renderer",
            ),
            (
                (old.window.columns, old.window.lines) != (new.window.columns, new.window.lines),
                "window.columns and window.lines",
            ),
            (
                (&old.window.title, &old.window.app_id) != (&new.window.title, &new.window.app_id),
                "window.title and window.app_id",
            ),
        ] {
            if changed {
                log::info!("{what} applies to new windows");
            }
        }
        if old.window.single_instance != new.window.single_instance {
            log::info!("window.single_instance applies when jotty starts");
        }

        // Every window's fonts first: a family that can't be loaded leaves
        // all of them as they are.
        let mut fonts: Vec<Option<Fonts>> = Vec::new();
        if (&old.font.family, &old.font.fallback) != (&new.font.family, &new.font.fallback) {
            let loaded: Result<Vec<Fonts>, _> = self
                .windows
                .iter()
                .map(|w| {
                    let px = font::points_to_px(w.font_points, w.scale as f32);
                    Fonts::try_new(&new.font.family, &new.font.fallback, px)
                })
                .collect();
            match loaded {
                Ok(loaded) => fonts = loaded.into_iter().map(Some).collect(),
                Err(e) => {
                    log::error!("{e}; keeping the current font");
                    // So that saving the file again tries again.
                    new.font.family = old.font.family.clone();
                    new.font.fallback = old.font.fallback.clone();
                }
            }
        }
        if (&old.url, &old.files) != (&new.url, &new.files) {
            let kinds: Vec<Vec<String>> =
                new.files.0.values().map(|k| k.extensions.clone()).collect();
            match Detector::try_new(&kinds, &new.url.regex) {
                Ok(links) => shared.links = links,
                Err(e) => log::error!("invalid url.regex, keeping the previous patterns: {e}"),
            }
        }
        let new = Rc::new(new);
        shared.config = Rc::clone(&new);
        fonts.resize_with(self.windows.len(), || None);
        for (window, fonts) in self.windows.iter_mut().zip(fonts) {
            window.apply_config(&shared.compositor, &old, &new, fonts);
        }
        if new.window.blur && !old.window.blur {
            self.update_capabilities();
        }
    }

    fn read_bus(&mut self) {
        let Some((bus, _)) = &mut self.shared.bus else {
            return;
        };
        let events = match bus.read() {
            Ok(events) => events,
            Err(e) => {
                // Connected again on the next notification.
                log::info!("lost the session bus: {e}");
                self.shared.disconnect_bus();
                return;
            }
        };
        for event in events {
            let notify::Event::Activate { tag, token } = event;
            let Some((shared, window)) = self.split(WindowId(tag)) else {
                continue;
            };
            match token {
                Some(token) => {
                    if let Some(activation) = &shared.activation {
                        activation.activate::<App>(window.window.wl_surface(), token);
                    }
                }
                // A daemon without activation tokens: only urgency is
                // possible.
                None => window.request_attention(shared),
            }
        }
    }

    fn text_input_event(&mut self, text_input: &ZwpTextInputV3, event: zwp_text_input_v3::Event) {
        use zwp_text_input_v3::Event;
        let target = match &event {
            Event::Enter { surface } => {
                let Some((shared, window)) = self.split_by_surface(surface) else {
                    return;
                };
                shared.ime_focus = Some(window.id);
                window.text_input = Some(text_input.clone());
                window.id
            }
            Event::Leave { .. } => match self.shared.ime_focus.take() {
                Some(id) => id,
                None => return,
            },
            _ => match self.shared.ime_focus {
                Some(id) => id,
                None => return,
            },
        };
        if let Some(window) = self.window_mut(target) {
            window.text_input_event(text_input, event);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        // wp-fractional-scale reports the exact scale instead.
        if let Some((_, window)) = self.split_by_surface(surface)
            && window.viewport.is_none()
        {
            window.set_scale(f64::from(factor.max(1)));
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        let Some((_, window)) = self.split_by_surface(surface) else {
            return;
        };
        window.frame_pending = false;
        if window.dirty {
            window.request_redraw();
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.shared.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window) {
        if let Some((_, window)) = self.split_by_surface(window.wl_surface()) {
            window.exit = true;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let Some((_, window)) = self.split_by_surface(window.wl_surface()) else {
            return;
        };
        let size_before = window.size;
        if let Some(w) = configure.new_size.0 {
            window.size.0 = w.get();
        }
        if let Some(h) = configure.new_size.1 {
            window.size.1 = h.get();
        }
        let unchanged = window.configured && window.size == size_before;
        window.configured = true;
        // Focus and state changes are configures too, with the same size.
        if unchanged {
            window.request_redraw();
        } else {
            window.apply_size();
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.shared.seat_state
    }

    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: WlSeat) {
        let shared = &mut self.shared;
        // Created before the window gets focus, so the compositor's focus
        // change reaches it as an `enter` event.
        if let (None, Some(manager)) = (&shared.text_input, &shared.text_input_manager) {
            shared.text_input = Some(manager.get_text_input(&seat, qh, TextInputData));
        }
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        let shared = &mut self.shared;
        match capability {
            Capability::Keyboard if shared.keyboard.is_none() => {
                let keyboard = shared.seat_state.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    shared.loop_handle.clone(),
                    Box::new(|app, _, event| app.key(event, KeyAction::Repeat)),
                );
                match keyboard {
                    Ok(keyboard) => shared.keyboard = Some(keyboard),
                    Err(e) => log::warn!("cannot use keyboard: {e}"),
                }
                if let (None, Some(manager)) = (&shared.data_device, &shared.data_device_manager) {
                    shared.data_device = Some(manager.get_data_device(qh, &seat));
                }
                if let (None, Some(manager)) =
                    (&shared.primary_device, &shared.primary_selection_manager)
                {
                    shared.primary_device = Some(manager.get_selection_device(qh, &seat));
                }
            }
            Capability::Pointer if shared.pointer.is_none() => {
                let surface = shared.compositor.create_surface(qh);
                let pointer = shared.seat_state.get_pointer_with_theme::<_, ()>(
                    qh,
                    &seat,
                    shared.shm.wl_shm(),
                    surface,
                    ThemeSpec::default(),
                );
                match pointer {
                    Ok(pointer) => shared.pointer = Some(pointer),
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
        _: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                if let Some(keyboard) = self.shared.keyboard.take() {
                    keyboard.release();
                }
                // No leave event comes for a keyboard that went away.
                if let Some((_, window)) = self.split_focused() {
                    window.focused = false;
                }
                let shared = &mut self.shared;
                shared.keyboard_focus = None;
                shared.modifiers = Modifiers::default();
                shared.sent_keys.clear();
            }
            Capability::Pointer => {
                if let Some(pointer) = self.shared.pointer.take() {
                    pointer.pointer().release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        surface: &WlSurface,
        serial: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        let Some((shared, window)) = self.split_by_surface(surface) else {
            return;
        };
        shared.keyboard_focus = Some(window.id);
        shared.last_serial = serial;
        window.focused = true;
        if window.term.screen().modes.focus_events {
            window.send(b"\x1b[I");
        }
        window.request_redraw();
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
    ) {
        let Some((shared, window)) = self.split_by_surface(surface) else {
            return;
        };
        if shared.keyboard_focus == Some(window.id) {
            shared.keyboard_focus = None;
        }
        window.focused = false;
        if window.term.screen().modes.focus_events {
            window.send(b"\x1b[O");
        }
        window.request_redraw();
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        self.shared.last_serial = serial;
        self.key(event, KeyAction::Press);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event, KeyAction::Repeat);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if let Some((shared, window)) = self.split_focused() {
            window.release(shared, event);
        }
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        layout: u32,
    ) {
        let shared = &mut self.shared;
        shared.modifiers = modifiers;
        shared.layout = layout;
        // Ctrl shows links in the window under the pointer.
        for window in &mut self.windows {
            window.update_hovered_link(shared);
        }
    }

    fn update_keymap(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        keymap: Keymap<'_>,
    ) {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        self.shared.keymap = xkb::Keymap::new_from_string(
            &context,
            keymap.as_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        );
        if self.shared.keymap.is_none() {
            log::warn!("cannot compile the keymap; key bindings match shifted symbols only");
        }
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let Some((shared, window)) = self.split_by_surface(&event.surface) {
                window.pointer_event(shared, conn, event);
            }
        }
    }
}

impl TermWindow {
    fn pointer_event(&mut self, shared: &mut Shared, conn: &Connection, event: &PointerEvent) {
        self.pointer_pos = event.position;
        if matches!(event.kind, PointerEventKind::Motion { .. })
            && (shared.modifiers.ctrl || self.hovered_link.is_some())
        {
            // Motion within a cell can't change the link under it.
            let cell = self.point_at(event.position);
            if self.hover_cell != Some(cell) {
                self.hover_cell = Some(cell);
                self.update_hovered_link(shared);
            }
        }
        match &event.kind {
            PointerEventKind::Enter { .. } => {
                self.pointer_inside = true;
                if let Some(pointer) = &shared.pointer
                    && let Err(e) = pointer.set_cursor(conn, CursorIcon::Text)
                {
                    log::debug!("cannot set cursor: {e}");
                }
                self.update_hovered_link(shared);
            }
            PointerEventKind::Leave { .. } => {
                self.pointer_inside = false;
                self.update_hovered_link(shared);
            }
            // A drag that started as a selection stays one, even if
            // Shift goes up on the way.
            PointerEventKind::Motion { .. } if self.selecting => {
                let point = self.point_at(event.position);
                if let Some(selection) = &mut self.selection {
                    selection.extend(self.term.screen(), point);
                    self.request_redraw();
                }
            }
            PointerEventKind::Motion { .. } if self.reporting_mouse(shared) => {
                let held = self.held_buttons.last().copied();
                self.report_mouse(shared, MouseAction::Motion(held), event.position);
            }
            PointerEventKind::Press { button, serial, .. }
                if self.reporting_mouse(shared) && mouse_button(*button).is_some() =>
            {
                shared.last_serial = *serial;
                let button = mouse_button(*button).expect("checked by the match guard");
                self.held_buttons.retain(|&held| held != button);
                self.held_buttons.push(button);
                self.mouse_cell = None;
                self.report_mouse(shared, MouseAction::Press(button), event.position);
            }
            // A release always follows a reported press, even if Shift
            // went down in between.
            PointerEventKind::Release { button, .. }
                if mouse_button(*button).is_some_and(|b| self.held_buttons.contains(&b)) =>
            {
                self.held_buttons
                    .retain(|&held| Some(held) != mouse_button(*button));
                self.mouse_cell = None;
                self.report_mouse(
                    shared,
                    MouseAction::Release(
                        mouse_button(*button).expect("checked by the match guard"),
                    ),
                    event.position,
                );
            }
            PointerEventKind::Axis {
                vertical,
                horizontal,
                ..
            } => self.wheel(shared, vertical, horizontal, event.position),
            PointerEventKind::Press {
                button: BTN_LEFT,
                serial,
                time,
            } if self.hovered_link.is_some() => {
                shared.last_serial = *serial;
                // A double click opens it once.
                let again = self
                    .last_link_click
                    .is_some_and(|last| time.wrapping_sub(last) <= DOUBLE_CLICK_MS);
                self.last_link_click = Some(*time);
                if !again && let Some(link) = &self.hovered_link {
                    self.open_link(link);
                }
            }
            // The second Shift+click: select from the marked cell.
            PointerEventKind::Press {
                button: BTN_LEFT,
                serial,
                ..
            } if shared.modifiers.shift && self.shift_mark.is_some() => {
                shared.last_serial = *serial;
                let mark = self.shift_mark.take().expect("checked by the match guard");
                let point = self.cell_start(self.point_at(event.position));
                let (start, end) = (mark.min(point), mark.max(point));
                self.selection = Some(Selection::span(start, end, Unit::Word));
                self.selecting = false;
                self.shift_press = false;
                self.last_click = None;
                self.request_redraw();
            }
            PointerEventKind::Press {
                button: BTN_LEFT,
                serial,
                time,
            } => {
                shared.last_serial = *serial;
                // A plain click cancels a Shift+click mark; a Shift+click
                // that isn't dragged sets one (on release).
                self.shift_press = shared.modifiers.shift;
                if !shared.modifiers.shift {
                    self.shift_mark = None;
                }
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
                    2 if shared.modifiers.ctrl => self
                        .output_selection(point)
                        .or_else(|| Selection::word(screen, point)),
                    2 => Selection::word(screen, point),
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
                if let Some(selection) = self.selection.filter(|s| s.is_empty()) {
                    self.selection = None;
                    if std::mem::take(&mut self.shift_press) {
                        self.shift_mark = Some(self.cell_start(selection.range().0));
                    }
                }
                self.set_primary(shared);
                self.request_redraw();
            }
            PointerEventKind::Press {
                button: BTN_MIDDLE,
                serial,
                ..
            } => {
                shared.last_serial = *serial;
                self.paste_primary(shared);
            }
            _ => {}
        }
    }
}

/// Calls `apply` with a region covering any surface size; compositors clip
/// it to the surface, so it never needs updating on resize.
fn set_whole_region(compositor: &CompositorState, apply: impl FnOnce(Option<&WlRegion>)) {
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
        &mut self.shared.background_effect
    }

    fn update_capabilities(&mut self) {
        let shared = &mut self.shared;
        if !shared.config.window.blur || self.windows.iter().all(|w| w.blur.is_some()) {
            return;
        }
        let supported = shared
            .background_effect
            .capabilities()
            .is_some_and(|c| c.contains(EffectCapability::Blur));
        if !supported {
            log::warn!("blur requested, but the compositor cannot blur");
            return;
        }
        for window in &mut self.windows {
            window.enable_blur(shared);
        }
    }
}

impl TermWindow {
    /// Asks the compositor to blur behind the window, once it can.
    fn enable_blur(&mut self, shared: &mut Shared) {
        if self.blur.is_some() {
            return;
        }
        let surface = self.window.wl_surface().clone();
        match shared
            .background_effect
            .get_background_effect(&surface, &shared.qh)
        {
            Ok(effect) => {
                set_whole_region(&shared.compositor, |region| effect.set_blur_region(region));
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
        &mut self.shared.shm
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
        _: &WlSurface,
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

    fn new_token(&mut self, token: String, data: &RequestData<()>) {
        if let (Some(activation), Some(surface)) = (&self.shared.activation, &data.surface) {
            activation.activate::<App>(surface, token);
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
            .shared
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
            .shared
            .primary_source
            .as_ref()
            .is_some_and(|(s, _)| s.inner() == source)
        {
            self.shared.primary_source = None;
        }
    }
}

/// Writes selection data to a reader, which may be slow (or be us), without
/// blocking the event loop.
fn write_in_background(mut pipe: WritePipe, text: String) {
    std::thread::spawn(move || {
        if let Err(e) = write_with_timeout(&mut pipe, text.as_bytes(), SELECTION_WRITE_TIMEOUT) {
            log::debug!("selection write: {e}");
        }
    });
}

/// Writes all of `data`, giving up once the reader has taken nothing for
/// `timeout`: a client that asks for the selection and never reads must not
/// keep a thread forever.
fn write_with_timeout<W: Write + AsRawFd>(
    pipe: &mut W,
    mut data: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: `fd` is open for as long as `pipe` is borrowed; these calls
    // read and set its status flags, and poll reads the one pollfd.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    while !data.is_empty() {
        match pipe.write(data) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
                // SAFETY: as above.
                match unsafe { libc::poll(&mut poll, 1, millis) } {
                    0 => return Err(io::ErrorKind::TimedOut.into()),
                    n if n < 0 => {
                        let e = io::Error::last_os_error();
                        if e.kind() != io::ErrorKind::Interrupted {
                            return Err(e);
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
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
            .shared
            .copy_source
            .as_ref()
            .filter(|(s, _)| s.inner() == source)
        {
            write_in_background(pipe, text.clone());
        }
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self
            .shared
            .copy_source
            .as_ref()
            .is_some_and(|(s, _)| s.inner() == source)
        {
            self.shared.copy_source = None;
        }
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

/// User data for the viewporter, the viewports and the fractional-scale
/// manager.
struct ScaleData;

/// User data for a window's fractional-scale object.
struct FractionalScaleData(WindowId);

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

impl Dispatch2<WpFractionalScaleV1, App> for FractionalScaleData {
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
            if let Some(window) = app.window_mut(self.0) {
                window.set_scale(f64::from(scale) / 120.0);
            }
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
        &mut self.shared.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
delegate_dispatch2!(App);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_that_never_reads_times_the_write_out() {
        let (_reader, mut writer) = io::pipe().unwrap();
        let data = vec![0; 1 << 20];
        let result = write_with_timeout(&mut writer, &data, Duration::from_millis(50));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_reader_that_keeps_reading_gets_everything() {
        use std::io::Read;
        let (mut reader, mut writer) = io::pipe().unwrap();
        let data = vec![7; 1 << 20];
        let thread = std::thread::spawn(move || {
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            out
        });
        write_with_timeout(&mut writer, &data, Duration::from_secs(10)).unwrap();
        drop(writer);
        assert_eq!(thread.join().unwrap(), data);
    }

    #[test]
    fn exit_codes_follow_the_shell() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        // Killed by SIGTERM.
        assert_eq!(exit_code(ExitStatus::from_raw(15)), 143);
    }
}
