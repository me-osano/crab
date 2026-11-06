//! Two-output tests driven through a real Wayland client.
//!
//! The compositor side is the same [`Crab`] state the backends use; the client
//! side is a small `wayland-client` connection over a `UnixStream` pair. Driving
//! the protocol instead of poking `ManagedWindow`s directly is what proves the
//! per-output fixes: a commit on an unfocused output still reaches
//! `on_commit`/`on_layer_commit`, an app-id event re-evaluates rules with the
//! window's owning output, and destroying a toplevel removes exactly that
//! window.
//!
//! Both ends share a thread and are pumped explicitly: flush the client, let
//! the server dispatch, flush the server, then read whatever the client can
//! read. No background thread, socket path, or `XDG_RUNTIME_DIR` is needed.

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crab::core::{ColumnWidth, LayoutCommand, SpaceSet, WindowHeight};
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as ServerResizeEdge;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use super::*;
use crate::compositor::transaction::TIME_LIMIT;

/// Client-side state: the bound globals and the last configure serial per
/// `xdg_surface`, so configures can be acknowledged.
#[derive(Default)]
struct TestClient {
    #[allow(dead_code)]
    registry: Option<wl_registry::WlRegistry>,
    compositor: Option<wl_compositor::WlCompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    /// Configure serials arriving before the client's dispatch for the
    /// `xdg_surface` that must acknowledge them.
    configures: HashMap<u32, u32>,
}

/// A toplevel the test client owns. Dropping it does not close it; call
/// [`ClientToplevel::close`] to send the protocol destructors.
struct ClientToplevel {
    surface: wl_surface::WlSurface,
    xdg_surface: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
}

impl ClientToplevel {
    /// Destroys the role, the `xdg_surface`, and the `wl_surface`, which is
    /// what a client does when it closes a window.
    fn close(self) {
        self.toplevel.destroy();
        self.xdg_surface.destroy();
        self.surface.destroy();
    }
}

/// A layer-shell surface the test client owns.
struct ClientLayer {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

impl ClientLayer {
    fn close(self) {
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// A two-output compositor, its in-process client, and the pump that keeps
/// both protocols moving.
struct Harness {
    display: Display<Crab>,
    handle: DisplayHandle,
    state: Crab,
    conn: Connection,
    queue: EventQueue<TestClient>,
    client: TestClient,
    outputs: [OutputId; 2],
    /// A clock ahead of wall time, so a test can flush the configure throttle
    /// without sleeping.
    test_clock: Instant,
}

impl Harness {
    /// Creates the compositor with a left 1920×1080 monitor and a right
    /// 2560×1440 one (`DP-1` and `DP-2`), then connects a client.
    fn new(config: Config) -> Self {
        let display = Display::<Crab>::new().expect("the test display is created");
        let mut handle = display.handle();
        let mut state = Crab::new_without_outputs(&handle, &config);

        let outputs = [
            add_test_output(
                &mut state,
                &handle,
                OutputId::new(0),
                "DP-1",
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                &config,
            ),
            add_test_output(
                &mut state,
                &handle,
                OutputId::new(1),
                "DP-2",
                Rect::new(1920.0, 0.0, 2560.0, 1440.0),
                &config,
            ),
        ];

        let (server_stream, client_stream) =
            UnixStream::pair().expect("the in-process socket pair is created");
        handle
            .insert_client(server_stream, Arc::new(ClientState::default()))
            .expect("the server accepts the test client");
        let conn = Connection::from_socket(client_stream).expect("the client connects");
        let queue = conn.new_event_queue::<TestClient>();
        let registry = conn.display().get_registry(&queue.handle(), ());

        let mut harness = Self {
            display,
            handle,
            state,
            conn,
            queue,
            client: TestClient {
                registry: Some(registry),
                ..TestClient::default()
            },
            outputs,
            test_clock: Instant::now(),
        };
        // The registry round trip binds the globals; the second pump lets the
        // server process those bind requests.
        harness.pump();
        harness.pump();
        assert!(
            harness.client.compositor.is_some(),
            "the client bound wl_compositor"
        );
        assert!(
            harness.client.wm_base.is_some(),
            "the client bound xdg_wm_base"
        );
        assert!(
            harness.client.layer_shell.is_some(),
            "the client bound zwlr_layer_shell_v1"
        );
        harness
    }

    /// Moves messages in both directions until neither side has more work.
    fn pump(&mut self) {
        for _ in 0..8 {
            self.conn.flush().expect("client requests flush");
            let dispatched = self
                .display
                .dispatch_clients(&mut self.state)
                .expect("the server dispatches client requests");
            self.display
                .flush_clients()
                .expect("server events flush to the client");
            if let Some(guard) = self.queue.prepare_read() {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(err) => panic!("the client failed to read events: {err}"),
                }
            }
            let events = self
                .queue
                .dispatch_pending(&mut self.client)
                .expect("the client dispatches server events");
            if dispatched == 0 && events == 0 {
                break;
            }
        }
    }

    fn focus(&mut self, output: OutputId) {
        self.state.focused_output = output;
    }

    /// Creates a bare toplevel on the focused output. The app id is set
    /// separately so a rule that needs it can be observed arriving late.
    fn create_toplevel(&mut self) -> (WindowId, ClientToplevel) {
        let before = self.state.windows.len();
        let qh = self.queue.handle();
        let compositor = self
            .client
            .compositor
            .clone()
            .expect("wl_compositor was bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base was bound");
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        surface.commit();
        self.pump();
        assert_eq!(
            self.state.windows.len(),
            before + 1,
            "the server registered the new toplevel"
        );
        let id = self.state.windows.last().expect("a window exists").id();
        (
            id,
            ClientToplevel {
                surface,
                xdg_surface,
                toplevel,
            },
        )
    }

    fn set_app_id(&mut self, toplevel: &ClientToplevel, app_id: &str) {
        toplevel.toplevel.set_app_id(app_id.to_owned());
        self.pump();
    }

    /// Creates a top-anchored, 300 px exclusive layer strip on the focused
    /// output. `None` makes the server use the focused output.
    fn create_exclusive_layer(&mut self) -> ClientLayer {
        let qh = self.queue.handle();
        let compositor = self
            .client
            .compositor
            .clone()
            .expect("wl_compositor was bound");
        let layer_shell = self
            .client
            .layer_shell
            .clone()
            .expect("zwlr_layer_shell_v1 was bound");
        let surface = compositor.create_surface(&qh, ());
        let layer = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "crab-test-bar".to_owned(),
            &qh,
            (),
        );
        layer.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        layer.set_exclusive_zone(300);
        layer.set_size(0, 300);
        surface.commit();
        self.pump();
        self.pump();
        ClientLayer { surface, layer }
    }

    fn add_output(&mut self, id: OutputId, name: &str, geometry: Rect) -> OutputId {
        let config = self.state.config.clone();
        add_test_output(&mut self.state, &self.handle, id, name, geometry, &config)
    }

    /// Marks a toplevel mapped and arranges it, so hit-testing and interaction
    /// see the rectangle the layout chose. The tests that use this disable
    /// animations so `visual_rect` equals the target immediately.
    fn map_toplevel(&mut self, id: WindowId) {
        if let Some(window) = self.state.window_mut(id) {
            window.mapped = true;
        }
        self.state.relayout();
    }

    /// The layout rectangle of a window (location and configured size).
    fn window_rect(&self, id: WindowId) -> Rect {
        self.state
            .window(id)
            .expect("the scenario window exists")
            .rect()
    }

    /// Creates an shm buffer of `width`x`height` for a client commit.
    fn create_buffer(&mut self, width: i32, height: i32) -> wl_buffer::WlBuffer {
        let shm = self.client.shm.clone().expect("wl_shm was bound");
        let qh = self.queue.handle();
        let width = width.max(1);
        let height = height.max(1);
        let stride = width * 4;
        let size = stride * height;
        // Tests run in parallel, so the name must be unique across the whole
        // process, not just this harness.
        static NEXT_SHM: AtomicU32 = AtomicU32::new(0);
        let path = format!(
            "/dev/shm/crab-test-{}-{}",
            std::process::id(),
            NEXT_SHM.fetch_add(1, Ordering::Relaxed)
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create the test shm file");
        file.set_len(u64::try_from(size).expect("a positive shm size"))
            .expect("size the test shm file");
        let pool = shm.create_pool(file.as_fd(), size, &qh, ());
        let buffer =
            pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
        let _ = std::fs::remove_file(&path);
        buffer
    }

    /// Sends every throttled configure by advancing a test-only clock.
    ///
    /// The clock is anchored to wall time so a configure that production code
    /// timestamped with `Instant::now()` is always older than it, then moved
    /// past the throttle window. Without that, the first flush could land
    /// before a configure sent during test setup and leave it pending.
    fn flush_configures(&mut self) {
        self.test_clock =
            self.test_clock.max(Instant::now()) + MIN_CONFIGURE_INTERVAL + Duration::from_millis(1);
        let now = self.test_clock;
        for window in &mut self.state.windows {
            window.flush_configure(now);
        }
        self.pump();
    }

    /// Commits a buffer at the size the layout asked for, as a real client
    /// does after acknowledging a configure.
    fn commit_configure(&mut self, id: WindowId, toplevel: &ClientToplevel) {
        // A configure inside the throttle window is still pending; a real
        // client would only see it after the frame boundary.
        self.flush_configures();
        let size = self
            .state
            .window(id)
            .and_then(|window| window.configure.asked())
            .expect("the window has been configured");
        let buffer = self.create_buffer(size.w, size.h);
        toplevel.surface.attach(Some(&buffer), 0, 0);
        toplevel.surface.damage_buffer(0, 0, size.w, size.h);
        toplevel.surface.commit();
        self.pump();
    }
}

/// Registers one output with a mode and a logical model rectangle.
fn add_test_output(
    state: &mut Crab,
    handle: &DisplayHandle,
    id: OutputId,
    name: &str,
    geometry: Rect,
    config: &Config,
) -> OutputId {
    let output = Output::new(
        name.to_owned(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "crab".to_owned(),
            model: "test".to_owned(),
        },
    );
    output.create_global::<Crab>(handle);
    let model = OutputModel::new(id, name, geometry, 1.0, Crab::initial_space_state(config));
    let id = state.add_output(output, model, Transform::Normal);
    let physical = (geometry.w.round() as i32, geometry.h.round() as i32).into();
    state.set_output_size(id, physical);
    id
}

impl Dispatch<wl_registry::WlRegistry, ()> for TestClient {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(6), qh, ()));
            }
            "xdg_wm_base" => {
                state.wm_base = Some(registry.bind(name, version.min(6), qh, ()));
            }
            "wl_shm" => {
                state.shm = Some(registry.bind(name, version.min(1), qh, ()));
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for TestClient {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for TestClient {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for TestClient {
    fn event(
        _state: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for TestClient {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.configures.insert(surface.id().protocol_id(), serial);
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for TestClient {
    fn event(
        _state: &mut Self,
        _proxy: &xdg_toplevel::XdgToplevel,
        _event: xdg_toplevel::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for TestClient {
    fn event(
        _state: &mut Self,
        _proxy: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for TestClient {
    fn event(
        _state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer.ack_configure(serial);
        }
    }
}

// The shm objects have no events the test cares about. The compositor still
// needs the buffer commit to reach `on_commit`, which is what the transaction
// tests watch.
delegate_noop!(TestClient: ignore wl_shm::WlShm);
delegate_noop!(TestClient: ignore wl_shm_pool::WlShmPool);
delegate_noop!(TestClient: ignore wl_buffer::WlBuffer);

#[test]
fn closing_a_window_on_a_background_output_compacts_only_that_output() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;

    // Two windows on the background output in spaces 1 and 3, and one on the
    // focused output in space 1. The gaps make the remap observable.
    h.focus(right);
    let (background_first, first) = h.create_toplevel();
    let (background_last, last) = h.create_toplevel();
    h.state
        .window_mut(background_first)
        .expect("the first background window exists")
        .window
        .spaces = SpaceSet::SPACE_1;
    h.state
        .window_mut(background_last)
        .expect("the last background window exists")
        .window
        .spaces = SpaceSet::SPACE_3;

    h.focus(left);
    let (foreground, foreground_client) = h.create_toplevel();
    h.state.focused_output = left;
    h.state.focus_window(Some(foreground));
    assert_eq!(h.state.focused_output, left);

    // Closing the background window must not move focus or renumber the
    // focused output's spaces.
    first.close();
    h.pump();

    assert!(h.state.window(background_first).is_none());
    assert_eq!(h.state.focused_output, left);
    assert_eq!(
        h.state
            .window(foreground)
            .map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1),
        "the focused output keeps its own space numbering"
    );
    assert_eq!(
        h.state
            .window(background_last)
            .map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1),
        "space 3 on the background output moved down to 1"
    );
    assert!(h.state.window(background_last).is_some());

    drop(foreground_client);
    drop(last);
}

#[test]
fn an_exclusive_zone_on_a_secondary_output_keeps_its_global_origin() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;

    h.focus(right);
    let layer = h.create_exclusive_layer();

    let left_area = h.state.outputs[&left].model.usable_area;
    let right_area = h.state.outputs[&right].model.usable_area;
    assert_eq!(
        left_area,
        Rect::new(0.0, 0.0, 1920.0, 1080.0),
        "the other output keeps its whole area"
    );
    assert_eq!(
        right_area,
        Rect::new(1920.0, 300.0, 2560.0, 1140.0),
        "the area starts at the secondary output's origin, not at (0, 0)"
    );

    // Removing the layer restores the full area at the same origin.
    layer.close();
    h.pump();
    assert_eq!(
        h.state.outputs[&right].model.usable_area,
        Rect::new(1920.0, 0.0, 2560.0, 1440.0)
    );
}

#[test]
fn a_rule_uses_the_owning_output_when_that_output_is_not_focused() {
    let mut config = Config::default();
    config.window_rules.push(crab::config::WindowRule {
        app_id: vec!["background".to_owned()],
        output: vec!["DP-2".to_owned()],
        open_floating: true,
        size: Some((400, 300)),
        ..crab::config::WindowRule::default()
    });
    let mut h = Harness::new(config);
    let [left, right] = h.outputs;

    // The window is created on the right output, but its app id (the matcher
    // the rule waits for) arrives after focus has moved left.
    h.focus(right);
    let (id, toplevel) = h.create_toplevel();
    // Move focus away without focusing the window: `focus_window` would move
    // the focused output back to the window's owner, which is what this test
    // deliberately avoids.
    h.state.focused_output = left;
    assert_eq!(h.state.focused_output, left);

    h.set_app_id(&toplevel, "background");

    let window = h.state.window(id).expect("the window still exists");
    assert_eq!(window.window.output, right);
    assert!(window.window.floating, "the rule floated the window");
    assert_eq!(
        window.location,
        (3000, 570).into(),
        "the rule centred it in DP-2, not in the focused DP-1"
    );
    assert_eq!(
        h.state.outputs[&left].model.usable_area,
        Rect::new(0.0, 0.0, 1920.0, 1080.0),
        "the focused output was not used as the rule's area"
    );
}

#[test]
fn removing_and_re_adding_an_output_keeps_focus_on_a_valid_output() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;

    h.focus(right);
    let (id, toplevel) = h.create_toplevel();
    h.state.focused_output = left;
    h.state.focus_window(Some(id));

    assert!(h.state.remove_output(right));
    assert_eq!(h.state.focused_output, left);
    assert_eq!(h.state.fallback_output, left);
    assert_eq!(
        h.state.window(id).map(|window| window.window.output),
        Some(left),
        "the removed output's window moved to the fallback"
    );

    // Re-adding the connector must not steal focus from the surviving output.
    let right = h.add_output(right, "DP-2", Rect::new(1920.0, 0.0, 2560.0, 1440.0));
    assert_eq!(h.state.focused_output, left);
    assert_eq!(h.state.fallback_output, left);
    assert!(h.state.outputs.contains_key(&right));

    // A click on the new monitor moves focus there.
    h.state.focus_at((2000.0, 500.0).into());
    assert_eq!(h.state.focused_output, right);

    drop(toplevel);
}

#[test]
fn removing_the_fallback_output_migrates_windows_to_the_lowest_survivor() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;
    let third = h.add_output(
        OutputId::new(2),
        "DP-3",
        Rect::new(4480.0, 0.0, 1920.0, 1080.0),
    );

    // `left` is the session fallback. Put a window there, then focus the new
    // monitor so neither focus nor the fallback is the output being removed.
    h.focus(left);
    let (id, toplevel) = h.create_toplevel();
    h.state.focused_output = third;

    assert!(h.state.remove_output(left));
    assert_eq!(
        h.state.window(id).map(|window| window.window.output),
        Some(right),
        "the fallback's windows move to the lowest surviving output"
    );
    assert_eq!(
        h.state.focused_output, third,
        "focus stays on the third output"
    );
    assert_eq!(
        h.state.fallback_output, third,
        "the focused output becomes the new fallback"
    );

    drop(toplevel);
}

#[test]
fn clicking_empty_space_moves_focus_to_that_output() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;
    assert_eq!(h.state.focused_output, left);

    h.state.focus_at((2000.0, 500.0).into());
    assert_eq!(h.state.focused_output, right);
    assert_eq!(h.state.focus, FocusTarget::None);
}

#[test]
fn moving_a_window_to_another_space_leaves_the_other_output_untouched() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;

    // Two windows on the left in space 1, one on the right.
    h.focus(left);
    let (first, _first_client) = h.create_toplevel();
    let (moved, moved_client) = h.create_toplevel();
    h.focus(right);
    let (peer, _peer_client) = h.create_toplevel();
    h.state.focus_window(Some(moved));
    assert_eq!(h.state.focused_output, left);

    h.state.move_to_space(2);
    assert_eq!(
        h.state.window(moved).map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_2)
    );
    assert_eq!(
        h.state.window(first).map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1),
        "the source space keeps its window"
    );
    assert_eq!(
        h.state.output().model.view.visible,
        SpaceSet::SPACE_2,
        "moving a window shows its destination"
    );
    assert_eq!(
        h.state.outputs[&right].model.view.visible,
        SpaceSet::SPACE_1,
        "the other output's view did not follow"
    );
    assert_eq!(
        h.state.window(peer).map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1)
    );

    // Closing the only window on the viewed space keeps the empty space past
    // the end (niri's dynamic-space rule), without renumbering the other one.
    moved_client.close();
    h.pump();
    assert!(h.state.window(moved).is_none());
    assert_eq!(h.state.output().model.view.visible, SpaceSet::SPACE_2);
    assert_eq!(
        h.state.window(first).map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1)
    );
}

#[test]
fn a_move_drag_snaps_to_the_output_edge_and_keeps_its_output() {
    let mut config = Config::default();
    config.animations.enabled = false;
    let mut h = Harness::new(config);
    let [left, _right] = h.outputs;

    h.focus(left);
    let (first, _first_client) = h.create_toplevel();
    let (second, _second_client) = h.create_toplevel();
    // Map both so the fair grid gives each a half-width column.
    h.map_toplevel(first);
    h.map_toplevel(second);
    assert_eq!(h.window_rect(first), Rect::new(8.0, 8.0, 948.0, 1064.0));

    assert!(
        h.state
            .begin_interaction(GrabMode::Move, (100.0, 100.0).into(), 1)
    );
    // Drag right until the far edge is within the snap distance of DP-1's
    // usable edge: the window lands flush at 972 + 948 = 1920, and the top
    // edge snaps to the usable area's origin rather than the gap inset.
    assert!(h.state.update_interaction((1050.0, 100.0).into()));
    assert_eq!(h.window_rect(first), Rect::new(972.0, 0.0, 948.0, 1064.0));
    assert_eq!(
        h.state.window(first).map(|window| window.window.output),
        Some(left),
        "the drag did not hand the window to the other output"
    );
    assert!(
        h.state
            .window(first)
            .is_some_and(|window| window.window.floating),
        "moving a tiled window floats it"
    );
    assert!(h.state.end_interaction(1).is_some());
    assert!(h.state.interaction.is_none());
}

#[test]
fn a_resize_drag_pins_the_opposite_edges_and_keeps_its_output() {
    let mut config = Config::default();
    config.animations.enabled = false;
    let mut h = Harness::new(config);
    let [left, _right] = h.outputs;

    h.focus(left);
    let (first, _first_client) = h.create_toplevel();
    let (second, _second_client) = h.create_toplevel();
    h.map_toplevel(first);
    h.map_toplevel(second);
    assert_eq!(h.window_rect(first), Rect::new(8.0, 8.0, 948.0, 1064.0));
    assert_eq!(h.window_rect(second), Rect::new(964.0, 8.0, 948.0, 1064.0));

    // Bottom-right follows the pointer; the top-left corner stays put.
    assert!(h.state.begin_interaction(
        GrabMode::Resize {
            edges: ServerResizeEdge::BottomRight,
        },
        (100.0, 100.0).into(),
        1,
    ));
    assert!(h.state.update_interaction((150.0, 150.0).into()));
    assert_eq!(h.window_rect(first), Rect::new(8.0, 8.0, 998.0, 1114.0));
    assert!(h.state.end_interaction(1).is_some());

    // Top-left follows; the opposite corner is pinned, so the size shrinks by
    // exactly the distance the pointer travelled. Floating the first window
    // left the second as the only tile, so it now fills the usable area.
    assert!(h.state.begin_interaction(
        GrabMode::Resize {
            edges: ServerResizeEdge::TopLeft,
        },
        (1200.0, 500.0).into(),
        1,
    ));
    assert!(h.state.update_interaction((1250.0, 550.0).into()));
    let resized = h.window_rect(second);
    assert_eq!(resized, Rect::new(58.0, 58.0, 1854.0, 1014.0));
    assert_eq!(resized.right(), 1912.0, "the right edge did not move");
    assert_eq!(resized.bottom(), 1072.0, "the bottom edge did not move");
    assert_eq!(
        h.state.window(second).map(|window| window.window.output),
        Some(left)
    );
    assert!(h.state.end_interaction(1).is_some());
}

#[test]
fn column_actions_reorder_and_resize_the_scroll_strip() {
    let mut config = Config {
        layout: "scroll".to_owned(),
        gaps: 0,
        ..Config::default()
    };
    config.animations.enabled = false;
    let mut h = Harness::new(config);
    let [left, _right] = h.outputs;

    h.focus(left);
    let (first, _first_client) = h.create_toplevel();
    let (second, _second_client) = h.create_toplevel();
    let (third, _third_client) = h.create_toplevel();
    h.map_toplevel(first);
    h.map_toplevel(second);
    h.map_toplevel(third);

    // Three half-view columns; the focused third one is scrolled to the right
    // edge, so the first column starts one view to the left.
    assert_eq!(h.window_rect(first), Rect::new(-960.0, 0.0, 960.0, 1080.0));
    assert_eq!(h.window_rect(second), Rect::new(0.0, 0.0, 960.0, 1080.0));
    assert_eq!(h.window_rect(third), Rect::new(960.0, 0.0, 960.0, 1080.0));
    assert_eq!(h.state.focused_id(), Some(third));

    // Moving the focused column left swaps it with the middle column.
    h.state.layout_command(LayoutCommand::MoveColumnLeft);
    assert_eq!(h.window_rect(first), Rect::new(0.0, 0.0, 960.0, 1080.0));
    assert_eq!(h.window_rect(third), Rect::new(960.0, 0.0, 960.0, 1080.0));
    assert_eq!(
        h.window_rect(second),
        Rect::new(1920.0, 0.0, 960.0, 1080.0),
        "the middle column moved to the end"
    );

    // The width preset widens only the focused column (half → two thirds).
    // The view scrolls left so the wider column still fits entirely on screen.
    h.state.layout_command(LayoutCommand::SwitchPresetWidth);
    assert_eq!(h.window_rect(third), Rect::new(640.0, 0.0, 1280.0, 1080.0));
    assert_eq!(
        h.window_rect(first).w,
        960.0,
        "the neighbour kept its width"
    );

    // An explicit width replaces the preset without touching the neighbour.
    h.state
        .layout_command(LayoutCommand::SetColumnWidth(ColumnWidth::Fixed(400.0)));
    assert_eq!(h.window_rect(third), Rect::new(640.0, 0.0, 400.0, 1080.0));
    assert_eq!(h.window_rect(first).w, 960.0);

    // Window heights are per window: the column stays put while the window
    // inside it becomes shorter.
    h.state
        .layout_command(LayoutCommand::SetWindowHeight(WindowHeight::Fixed(500.0)));
    assert_eq!(h.window_rect(third), Rect::new(640.0, 0.0, 400.0, 500.0));
    assert_eq!(
        h.window_rect(first).h,
        1080.0,
        "other columns keep their height"
    );
}

#[test]
fn closing_the_focused_window_hands_focus_to_a_survivor_on_its_output() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;

    h.focus(left);
    let (first, first_client) = h.create_toplevel();
    let (second, _second_client) = h.create_toplevel();
    h.focus(right);
    let (peer, _peer_client) = h.create_toplevel();
    h.map_toplevel(first);
    h.map_toplevel(second);

    h.state.focus_window(Some(first));
    assert_eq!(h.state.focused_output, left);
    assert_eq!(h.state.focused_id(), Some(first));

    first_client.close();
    h.pump();

    assert!(h.state.window(first).is_none());
    assert_eq!(h.state.focused_output, left);
    assert_eq!(
        h.state.focused_id(),
        Some(second),
        "the surviving window on the same output takes focus"
    );
    assert_eq!(
        h.state.window(second).map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1)
    );
    assert_eq!(
        h.state.window(peer).map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1),
        "the other output's window was not renumbered"
    );
    assert_eq!(
        h.state.outputs[&right].model.view.visible,
        SpaceSet::SPACE_1
    );
}

#[test]
fn two_clients_commit_before_a_resize_is_presented() {
    let mut config = Config::default();
    config.animations.enabled = false;
    let mut h = Harness::new(config);
    let [left, _right] = h.outputs;

    h.focus(left);
    let (first, first_client) = h.create_toplevel();
    let (second, second_client) = h.create_toplevel();
    h.map_toplevel(first);
    h.map_toplevel(second);

    // Let the initial layout settle: both clients commit the size the layout
    // asked for, as a real client does after acknowledging its configure.
    h.commit_configure(first, &first_client);
    h.commit_configure(second, &second_client);
    assert!(h.state.can_present_frame(), "the initial layout is settled");

    // A resize changes both windows. Presenting now would mix their old and
    // new buffers, so the frame is held until both have committed.
    h.state.set_output_size(left, (1600, 900).into());
    assert!(
        !h.state.can_present_frame(),
        "the resize is not presented before the clients answer"
    );
    assert!(
        h.state.is_animating(),
        "the redraw loop keeps running so the commits and deadline can arrive"
    );

    h.commit_configure(first, &first_client);
    assert!(
        !h.state.can_present_frame(),
        "one client is still on its old buffer"
    );

    h.commit_configure(second, &second_client);
    assert!(
        h.state.can_present_frame(),
        "both commits released the frame"
    );
}

#[test]
fn a_client_that_ignores_configure_does_not_hold_the_frame_past_the_deadline() {
    let mut config = Config::default();
    config.animations.enabled = false;
    let mut h = Harness::new(config);
    let [left, _right] = h.outputs;

    h.focus(left);
    let (first, first_client) = h.create_toplevel();
    let (second, second_client) = h.create_toplevel();
    h.map_toplevel(first);
    h.map_toplevel(second);
    h.commit_configure(first, &first_client);
    h.commit_configure(second, &second_client);

    h.state.set_output_size(left, (1600, 900).into());
    h.commit_configure(first, &first_client);
    assert!(
        !h.state.can_present_frame(),
        "the second client has not committed yet"
    );

    // The deadline is what keeps one unresponsive client from freezing the
    // session: the frame presents the state that has been committed.
    let expired = h.state.transactions.expire(Instant::now() + TIME_LIMIT);
    assert_eq!(expired, 1, "the laggard's transaction was released");
    assert!(
        h.state.can_present_frame(),
        "the committed state is presented at the deadline"
    );
    assert!(
        h.state
            .window(second)
            .is_some_and(ManagedWindow::awaits_configure),
        "the laggard is still behind and the WindowFit fallback draws it"
    );
}

#[test]
fn unplugging_an_output_parks_its_spaces_and_replug_restores_them() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;
    // A third output takes focus, so migration must follow the fallback rather
    // than dumping the windows on whatever output is focused.
    let third = h.add_output(
        OutputId::new(2),
        "DP-3",
        Rect::new(4480.0, 0.0, 1920.0, 1080.0),
    );

    // The fallback output keeps a window of its own in space 1.
    h.focus(left);
    let (left_window, _left_client) = h.create_toplevel();
    h.map_toplevel(left_window);

    // The unplugged output has two spaces with distinct layout, scroll, and
    // focus state. That state must travel with the windows.
    h.focus(right);
    let (right_first, _first_client) = h.create_toplevel();
    let (right_second, _second_client) = h.create_toplevel();
    {
        let runtime = h.state.outputs.get_mut(&right).expect("the output exists");
        runtime.model.space_states[0].layout = LayoutKind::Scroll;
        runtime.model.space_states[0].scroll.offset = 120.0;
        runtime.model.space_states[0].params.gaps = 4.0;
        runtime.model.space_states[0].focus_history = vec![right_first];
        runtime.model.space_states[1].layout = LayoutKind::Floating;
        runtime.model.space_states[1].scroll.offset = 45.0;
        runtime.model.space_states[1].params.gaps = 12.0;
        runtime.model.space_states[1].focus_history = vec![right_second];
    }
    h.state
        .window_mut(right_first)
        .expect("the window exists")
        .window
        .spaces = SpaceSet::SPACE_1;
    h.state
        .window_mut(right_second)
        .expect("the window exists")
        .window
        .spaces = SpaceSet::SPACE_2;
    h.map_toplevel(right_first);
    h.map_toplevel(right_second);

    // Unplug: the two spaces move to the fallback output as units, keeping
    // their order and staying distinct from the fallback's own space 1.
    h.focus(third);
    assert!(h.state.remove_output(right));
    assert_eq!(
        h.state
            .window(right_first)
            .map(|window| window.window.output),
        Some(left),
        "the spaces follow the fallback, not the focused output"
    );
    assert_eq!(
        h.state
            .window(right_second)
            .map(|window| window.window.output),
        Some(left)
    );
    let first_slot = h
        .state
        .window(right_first)
        .expect("the window exists")
        .window
        .spaces;
    let second_slot = h
        .state
        .window(right_second)
        .expect("the window exists")
        .window
        .spaces;
    assert_eq!(first_slot.iter_indices().count(), 1);
    assert_eq!(second_slot.iter_indices().count(), 1);
    assert_ne!(first_slot, second_slot, "the two spaces stay distinct");
    assert_ne!(
        first_slot,
        h.state
            .window(left_window)
            .expect("the window exists")
            .window
            .spaces,
        "the fallback's own space is not reused"
    );

    let first_index = usize::from(first_slot.iter_indices().next().expect("one space") - 1);
    let second_index = usize::from(second_slot.iter_indices().next().expect("one space") - 1);
    let left_model = &h.state.outputs[&left].model;
    assert_eq!(
        left_model.space_states[first_index].layout,
        LayoutKind::Scroll
    );
    assert_eq!(left_model.space_states[first_index].scroll.offset, 120.0);
    assert_eq!(left_model.space_states[first_index].params.gaps, 4.0);
    assert_eq!(
        left_model.space_states[first_index].focus_history,
        vec![right_first]
    );
    assert_eq!(
        left_model.space_states[second_index].layout,
        LayoutKind::Floating
    );
    assert_eq!(left_model.space_states[second_index].scroll.offset, 45.0);
    assert_eq!(left_model.space_states[second_index].params.gaps, 12.0);
    assert_eq!(
        left_model.space_states[second_index].focus_history,
        vec![right_second]
    );
    assert_eq!(
        left_model.space_states[first_index].home,
        Some(SpaceHome {
            output: right,
            slot: 1
        }),
        "the space remembers where it came from"
    );

    // Replug: the spaces and their windows move back to their old slots, and
    // the fallback returns to its own view.
    let right = h.add_output(right, "DP-2", Rect::new(1920.0, 0.0, 2560.0, 1440.0));
    assert_eq!(
        h.state
            .window(right_first)
            .map(|window| window.window.output),
        Some(right)
    );
    assert_eq!(
        h.state
            .window(right_first)
            .map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_1)
    );
    assert_eq!(
        h.state
            .window(right_second)
            .map(|window| window.window.spaces),
        Some(SpaceSet::SPACE_2)
    );
    let right_model = &h.state.outputs[&right].model;
    assert_eq!(right_model.space_states[0].layout, LayoutKind::Scroll);
    assert_eq!(right_model.space_states[0].scroll.offset, 120.0);
    assert_eq!(right_model.space_states[0].params.gaps, 4.0);
    assert_eq!(right_model.space_states[1].layout, LayoutKind::Floating);
    assert_eq!(right_model.space_states[1].scroll.offset, 45.0);
    assert_eq!(right_model.view.visible, SpaceSet::SPACE_1);
    assert_eq!(
        h.state
            .window(left_window)
            .map(|window| window.window.output),
        Some(left),
        "the fallback's own window did not move"
    );
    assert_eq!(
        h.state.outputs[&left].model.view.visible,
        SpaceSet::SPACE_1,
        "the fallback's view is its own again"
    );
}

#[test]
fn a_parked_space_takes_a_free_slot_and_returns_to_its_home_slot() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;

    // Both outputs use space 1, so parking the right output's space must pick
    // another slot on the fallback without disturbing the fallback's own space.
    h.focus(left);
    let (left_window, _left_client) = h.create_toplevel();
    h.map_toplevel(left_window);
    h.focus(right);
    let (right_window, _right_client) = h.create_toplevel();
    h.map_toplevel(right_window);
    assert_eq!(
        h.state
            .window(right_window)
            .expect("the window exists")
            .window
            .spaces,
        SpaceSet::SPACE_1
    );

    assert!(h.state.remove_output(right));
    let parked = h
        .state
        .window(right_window)
        .expect("the window exists")
        .window
        .spaces;
    assert_eq!(parked, SpaceSet::SPACE_2, "the occupied slot is skipped");
    assert_eq!(
        h.state.outputs[&left].model.space_states[1].home,
        Some(SpaceHome {
            output: right,
            slot: 1
        })
    );
    assert_eq!(
        h.state
            .window(left_window)
            .expect("the window exists")
            .window
            .spaces,
        SpaceSet::SPACE_1,
        "the fallback's own window stays in its space"
    );

    let right = h.add_output(right, "DP-2", Rect::new(1920.0, 0.0, 2560.0, 1440.0));
    assert_eq!(
        h.state
            .window(right_window)
            .expect("the window exists")
            .window
            .spaces,
        SpaceSet::SPACE_1,
        "replug restores the home slot, not the parked one"
    );
    assert_eq!(
        h.state
            .window(right_window)
            .expect("the window exists")
            .window
            .output,
        right
    );
    assert_eq!(h.state.outputs[&right].model.space_states[0].home, None);
}

#[test]
fn parked_spaces_survive_their_host_output_being_unplugged_too() {
    let mut h = Harness::new(Config::default());
    let [left, right] = h.outputs;
    let third = h.add_output(
        OutputId::new(2),
        "DP-3",
        Rect::new(4480.0, 0.0, 1920.0, 1080.0),
    );

    h.focus(right);
    let (window, _client) = h.create_toplevel();
    {
        let runtime = h.state.outputs.get_mut(&right).expect("the output exists");
        runtime.model.space_states[0].layout = LayoutKind::Scroll;
        runtime.model.space_states[0].params.gaps = 3.0;
    }
    h.map_toplevel(window);

    // Right parks on the fallback (left), then left is unplugged too. The
    // window's home is still right, not its current host.
    assert!(h.state.remove_output(right));
    assert_eq!(
        h.state
            .window(window)
            .expect("the window exists")
            .window
            .output,
        left
    );
    assert!(h.state.remove_output(left));
    assert_eq!(
        h.state
            .window(window)
            .expect("the window exists")
            .window
            .output,
        third
    );

    // Replugging right pulls it back off the second host, with its state.
    let right = h.add_output(right, "DP-2", Rect::new(1920.0, 0.0, 2560.0, 1440.0));
    let window = h.state.window(window).expect("the window exists");
    assert_eq!(window.window.output, right);
    assert_eq!(window.window.spaces, SpaceSet::SPACE_1);
    assert_eq!(
        h.state.outputs[&right].model.space_states[0].layout,
        LayoutKind::Scroll
    );
    assert_eq!(
        h.state.outputs[&right].model.space_states[0].params.gaps,
        3.0
    );
    assert_eq!(h.state.outputs[&right].model.space_states[0].home, None);
}
