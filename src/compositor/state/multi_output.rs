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
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use crab::core::{ColumnWidth, LayoutCommand, SpaceSet, WindowHeight};
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as ServerResizeEdge;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use super::*;

/// Client-side state: the bound globals and the last configure serial per
/// `xdg_surface`, so configures can be acknowledged.
#[derive(Default)]
struct TestClient {
    #[allow(dead_code)]
    registry: Option<wl_registry::WlRegistry>,
    compositor: Option<wl_compositor::WlCompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
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
