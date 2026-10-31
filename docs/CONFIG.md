# Configuration

crab uses TOML configuration with mango-style sections. The shipped example is [`resources/default-config.toml`](../resources/default-config.toml).

## Location and loading

- **Default path**: `$XDG_CONFIG_HOME/crab/config.toml`, falling back to `$HOME/.config/crab/config.toml` when `XDG_CONFIG_HOME` is unset or not absolute. If `--config` is given it is used instead.
- **Validate without running**: `crab --validate --config <path>`. Also `crab-cli config-validate` against a running compositor's active config file.
- **Hot reload**: if `notify` watches the config's parent directory, changes are debounced (~250ms). Parsed and validated atomically; on failure the last good config remains, a banner is shown, and `ConfigError`/`config-error` is emitted over IPC. Use `crab-cli config-reload` or bind `reload-config`/`Config::ReloadConfig`.
- **Unknown keys** are ignored. A key that is **present but wrong type** is an error. A pre-`[layout]` flat form (`gaps`, `border_width`, ...) is rejected with a pointer to move keys under `[layout]`.

### Includes

A config may pull in other files:

```toml
include = ["base.toml", "theme.toml", "?local.toml"]
```

- Paths are relative to the **including file**; absolute paths are accepted.
- A leading `?` marks an optional entry: a missing file is skipped instead of failing the load.
- Includes are resolved depth-first in order, and included files may include others. Cycles are rejected.
- Merging is per key: tables merge recursively, arrays concatenate, and a later value overrides an earlier one. The including file therefore overrides its includes, and later entries override earlier ones.
- Each included file's syntax errors name that file. `parse()` (text-only) rejects `include`; use `load()` / `--config` so the base directory is known.
- Hot reload watches the **main** config's directory. Files included from other directories are re-read on reload but editing them alone does not trigger it.

## Conventions

- **Booleans/flags**: accept `true`/`false` or `0`/`1` (mango style). Other values are errors.
- **Colors**: `#rrggbb` hex strings (e.g. `#7aa2f7`).
- **Numbers**: integers/floats as appropriate; validated (e.g. ratios/clamps).
- **Arrays of tables**: rules may be written as `[window_rule]` (single) or `[[window_rule]]` (multiple). Action maps are flat tables under `[binds.*]`.
- **Sections parse early**: features not yet applied still parse and validate so configs can be written ahead of milestones (see "Status" below).

## Sections

### `[input.keyboard]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `repeat_rate` | int (keys/s) | `25` | `0` disables repeat. Applied to Smithay keyboard repeat. |
| `repeat_delay` | int (ms) | `600` | Must be >= 0. |
| `numlock_on` | bool/0-1 | `false` | Parsed; not yet applied by current backend. |
| `xkb_rules_layout` | string | `"us"` | xkb layout(s), e.g. `"us,de"`. Applied at startup and on reload (new keymap compiled before swap; invalid layouts fail reload and keep previous). Runtime switch via `SwitchKeyboardLayout` / built-ins `SUPER+ALT+space`, `SUPER+ALT+SHIFT+space`. |

### `[input.touchpad]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `disable` | bool/0-1 | `false` | Parsed (M4). |
| `tap_to_click` | bool/0-1 | `true` | Parsed (M4). |
| `tap_and_drag` | bool/0-1 | `true` | Parsed (M4). |
| `drag_lock` | bool/0-1 | `true` | Parsed (M4). |
| `natural_scrolling` | bool/0-1 | `true` | Parsed (M4). |
| `disable_while_typing` | bool/0-1 | `true` | Parsed (M4). |
| `swipe_min_threshold` | float | `1.0` | Parsed (M4). Must be >= 0. |

### `[input.mouse]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `natural_scrolling` | bool/0-1 | `false` | Parsed (M4). |

### `[output.<connector>]`
One table per connector (e.g. `[output.eDP-1]`, `[output.DP-1]`). `<connector>` must be non-empty.

| Key | Type | Default | Notes |
|---|---|---|---|
| `mode` | string | - | e.g. `"1920x1080@120.030"`. Parsed/validated; applied by DRM backend (M4). |
| `scale` | float | - | Logical scale factor. Applied on registration/reload where model exists. Must be > 0. |
| `transform` | string | - | `normal`, `90`, `180`, `270`, `flipped`, `flipped-90`, `flipped-180`, `flipped-270`. Parsed/validated; applied by backend (M4). |
| `position` | table `{ x=.., y=.. }` | - | Logical position; applied if output at origin. |

### `[layout]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `gaps` | int | `8` | Outer/inner gaps in logical pixels. Must be >= 0. Relayout on reload. |
| `border_width` | int | `2` | `0` disables borders. Must be >= 0. Relayout on reload. |
| `border_color` | `#rrggbb` | `#7aa2f7` | Focused window border. |
| `unfocused_border_color` | `#rrggbb` | `#3b4261` | Unfocused window border. |
| `client_side_decorations` | bool/0-1 | `false` | Parsed, not applied (xdg-decoration M6). |
| `focus_follows_cursor` | bool/0-1 | `true` | Applied on pointer motion in compositor input handling. |
| `default_layout` | string | `"tile"` | Mode or family name for new spaces. Must be a valid layout/mode (see below). |

**Renames**: `[layout] layout` was renamed to `default_layout` — using old key is an error with a pointer.

#### `[layout.stack]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `next_space_direction` | `horizontal` \| `vertical` | `horizontal` | Parsed, not applied yet (space creation policy). |

#### `[layout.tile]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `next_space_direction` | `horizontal` \| `vertical` | `horizontal` | Parsed, not applied yet. |

#### `[layout.scroll]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `center_focused_column` | `never` \| `always` \| `on-overflow` | `never` | View centering behavior. |
| `always_center_single_column` | bool/0-1 | `true` | Parsed, not applied yet. |
| `default_column_ratio` | float | `0.5` | Width for new column (fraction of view). Must be > 0 and finite. |
| `next_space_direction` | `horizontal` \| `vertical` | `vertical` | Parsed, not applied yet. |

**Layout kinds/modes/families** (from core):
- Families (cycle order): `stack`, `tile`, `scroll`
- Modes (all): `fair` (`tile`), `floating` (`stack`), `scroll` (`scroll`)
- Config/IPC accept mode names or family names; a family name is an alias for that family's default mode (`stack` → `floating`, `tile` → `fair`, `scroll` → `scroll`). `free` may be accepted in some places as alias for floating. `LayoutKind::from_name` validates `default_layout` and `set-layout`. The removed modes `monocle` and `vertical-scroll` are rejected.

### `[animations]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `enabled` | bool/0-1 | `true` | Global toggle; also togglable at runtime via IPC. |
| `animation_type_open` | `slide` \| `zoom` | `slide` | Open animation style for windows/layers where used. |
| `animation_type_close` | `slide` \| `zoom` | `slide` | Close animation style. |

### `[autostart]`
| Key | Type | Default | Notes |
|---|---|---|---|
| `exec_once` | list of strings | `[]` | Commands spawned once at startup. Spawn uses shell-style quoting but **does not invoke a shell** (parsed via `spawn::parse_command`). Autostarted/spawned processes receive `WAYLAND_DISPLAY` and `CRAB_SOCKET` when IPC is enabled. |

### Window rules: `[window_rule]` or `[[window_rule]]`
Rules are scanned in config order; **first matching rule wins**. A rule is applied **once at map time**. If a rule names `app_id` or `title`, it **waits** until that info arrives (late application still triggers compact/relayout). Dynamic re-evaluation of rules after title/app-id changes is not yet implemented.

**Matchers** (omitted = wildcard):
| Key | Type | Notes |
|---|---|---|
| `app_id` | string or list of strings | Must match exactly (case-sensitive). Stable Wayland/X11 identifier. |
| `title` | string or list of strings | Matches if title contains any fragment, case-insensitively. |
| `is_floating` | bool/0-1 | Require window floating state at match time. |
| `is_fullscreen` | bool/0-1 | Require window fullscreen state at match time. |

**Actions**:
| Key | Type | Notes |
|---|---|---|
| `open_floating` | bool/0-1 | Float the window on open. |
| `size` | `[width, height]` | Initial size in logical pixels; asked via configure (clamped to client hints). |
| `offset_x` | int | Offset from centered position (x). |
| `offset_y` | int | Offset from centered position (y). |
| `space` | int (1-based) | Move window to a 1-based **dynamic** space index (lands on next empty if past last occupied, like a bind). Must be in `1..=32`. |
| `pinned` | bool/0-1 | Show on every space. |
| `always_on_top` | bool/0-1 | Draw above ordinary floating windows; fullscreen/maximized still cover. |
| `no_focus` | bool/0-1 | Never take keyboard focus (clicking won't move focus away). |

### Layer rules: `[layer_rule]`
Matched by namespace when a layer-shell surface maps. **First matching rule wins** (empty `layer_name` is wildcard).

| Key | Type | Notes |
|---|---|---|
| `animation_type_open` | `slide` \| `zoom` | Open animation for matching layer namespace; if absent/no rule, appears immediately. Validated if present. |
| `layer_name` | string or list of strings | Namespaces to match (exact string equality). |

### Binds

**Tables**:
- `[binds.default]` — normal session binds
- `[binds.locked]` — active always for now (lock screen not yet implemented; useful for volume/brightness)

**Compilation**: built-ins are added first, then `default` and `locked` in order (config replaces built-in with same chord). Invalid/unimplemented binds produce startup warnings but do not prevent startup or `--validate`.

**Chord syntax**: `MOD+MOD+KEY`. Modifiers: `SUPER`/`LOGO`/`WIN`/`MOD4`, `CTRL`/`CONTROL`, `ALT`/`MOD1`, `SHIFT`. Use `NONE+...` for no modifiers. Lock-state modifiers (CapsLock/NumLock) are ignored. Keys are looked up by xkb name; case-insensitive fallback applied. Mouse chords end with `btn_left`/`btn_right`/`btn_middle`/`btn_side`/`btn_extra`/`btn_forward`/`btn_back`/`btn_task`. Axis chords end with `UP`/`DOWN`/`LEFT`/`RIGHT` (or `WHEEL_*`).

**Bind flags**: append `:flag,flag` to a keyboard chord. Unknown flags are reported as bind issues (the bind is skipped), so typos do not pass silently.

```toml
[binds.default]
"SUPER+q:release" = "killclient"         # run on key-up, not key-down
"SUPER+x:pass-through" = "spawn:notify-send hi"  # also send the key to the client
"SUPER+Left:repeat" = "viewprev_have_client"     # re-fire on backend repeat presses
```

| Flag | Effect |
|---|---|
| `release` | Run the action on key release; press and release are swallowed unless `pass-through`. |
| `pass-through` | Run the action *and* forward the key to the focused client. |
| `repeat` | Re-run the action on repeat presses the backend reports. crab does not synthesize key repeat yet, so this is currently a no-op with most backends. |
| `allow-when-locked` / `locked` | Parsed and reserved for the lock screen (M6); no effect yet. |
| `allow-inhibiting` | Parsed and reserved for keyboard inhibitors (M6); no effect yet. |

**Key modes (submaps)**: `[modes.<name>]` tables add binds that are active only while that mode is on top of the stack. A mode table can contain `binds`, `mousebinds`, `axisbinds`, and `gesturebinds`. Binds are looked up from the top mode down, then fall back to `[binds.*]`/built-ins, so default binds (including `quit`/`reload-config`) keep working inside a mode. The name `default` is reserved.

```toml
[binds.default]
"SUPER+ALT+s" = "enter_mode:scroll"

[modes.scroll.binds]
"r" = "preset_width"
"c" = "center_column"
"Escape" = "leave_mode"     # back to default
```

Each mode may also have `[modes.<name>.mousebinds]`, `[modes.<name>.axisbinds]`, and `[modes.<name>.gesturebinds]`, using the same chord syntax as their top-level counterparts.

Actions: `enter_mode:<name>` (aliases `enter-mode:`, `mode:`), `leave_mode`/`exit_mode` (reset to default), `pop_mode` (pop one mode). Entering an unknown mode is ignored with a warning, so a stale bind cannot break input. Reload keeps the stack in order and drops modes that no longer exist.

**Actions**: see "Action reference" below. Config accepts mango-style aliases; wire/IPC uses canonical `crab_ipc::Action` names.

**Mouse/axis/gestures**:
- `[mousebinds]` compiled over built-ins `SUPER+btn_left` = `moveresize,curmove`, `SUPER+btn_right` = `moveresize,curresize`. Config replaces by chord. Bare left/right `moveresize` without modifier is refused (would swallow clicks). Mouse actions can be `moveresize,curmove|move` or `moveresize,curresize|resize` or any normal action.
- `[axisbinds]` for scroll directions with modifiers. Used for actions like `viewtoleft_have_client`/`viewtoright_have_client`.
- `[gesturebinds]` for libinput swipe gestures. Chords combine an optional direction, finger count, and modifiers: `left+3`, `right+4`, `none+up+3`. The gesture fires when the swipe ends if it moved at least 30 logical pixels along its dominant axis; the finger count and current modifiers must match.

### Built-in keyboard bindings (defaults)
| Chord | Action | Notes |
|---|---|---|
| `SUPER+1..9` | `view_space:N` | View space N |
| `SUPER+SHIFT+1..9` | `move_client_space:N` | Move focused window to space N |
| `SUPER+CTRL+1..9` | `toggle_space_view:N` | Toggle space visibility |
| `SUPER+CTRL+SHIFT+1..9` | `toggle_space:N` | Toggle space on window |
| `SUPER+Tab` | `toggle_back` | Swap to previous view set |
| `SUPER+Left/Right/Up/Down` | `focusdir:*` | Directional focus |
| `SUPER+j/k` | `focus_cycle_next/prev` | Cycle focus |
| `SUPER+SHIFT+space` | `toggle_floating` | Toggle floating |
| `SUPER+CTRL+space` | `switch_layout` / `cycle_layout_family` | Cycle layout family |
| `SUPER+space` | `cycle_layout_mode` / `cycle_layout` | Cycle layout within family |
| `SUPER+ALT+space` | `next_keyboard_layout` | Switch keyboard layout forward |
| `SUPER+ALT+SHIFT+space` | `prev_keyboard_layout` | Switch keyboard layout backward |
| `SUPER+f` | `toggle_fullscreen` | Toggle fullscreen |
| `SUPER+r` | `preset_width` | Cycle scroll column width preset |
| `SUPER+c` | `center_column` | Center focused scroll column |
| `SUPER+q` | `killclient` / `close-window` | Close focused window |

Config binds override built-ins for the same chord.

## Action reference (canonical)

Canonical actions match [`crab_ipc::Action`](../crab-ipc/src/protocol.rs). Config strings accept aliases below.

| Action (canonical/wire) | Config aliases | Args | Notes |
|---|---|---|---|
| `quit` | `quit`, `exit` | - | Exit compositor |
| `spawn` | `spawn:<command>` | argv (shell-style quoting, no shell) | Spawn command |
| `close-window` | `killclient`, `close`, `closewindow`, `close_window`, `close-window` | - | Close focused window |
| `focus-window` | `focus_window:<id>`, `focus-window:<id>` | `id` (u64>=1) | Focus window by id |
| `focus-direction` | `focusdir:<dir>`, `focus-direction:<dir>` | `left\|right\|up\|down` | Directional focus |
| `move-window` | `move_client:<dir>`, `move-window:<dir>` | `left\|right\|up\|down` | Move focused window |
| `focus-cycle` | `focus_cycle_next`/`focusnext` (forward), `focus_cycle_prev`/`focusprev` (backward) | `forward: bool` | Cycle focus |
| `switch-keyboard-layout` | `switch_keyboard_layout`, `next_keyboard_layout`, `switch-keyboard-layout` (forward); `switch_keyboard_layout_prev`, `prev_keyboard_layout`, `switch-keyboard-layout-prev` (backward) | `forward: bool` | Cycle through layouts in `xkb_rules_layout` |
| `view-space` | `view_space:<n>`, `view-space:<n>` | `index` 1-based (1..=32) | Show a space (dynamic space semantics apply) |
| `move-to-space` | `move_client_space:<n>`, `move-to-space:<n>` | `index` 1-based | Move focused window to space |
| `toggle-space-view` | `toggle_space_view:<n>`, `toggle_view_space:<n>`, `toggle-space-view:<n>` | `index` 1-based | Toggle space visibility |
| `toggle-space` | `toggle_space:<n>`, `toggle-space:<n>` | `index` 1-based | Toggle space on focused window |
| `view-space-step` | `view_space_left`/`view_space_prev`/`viewprev` (-1), `view_space_right`/`view_space_next`/`viewnext` (1) | `step: -1\|1` | Step view |
| `view-space-have-client` | `viewprev_have_client`/`viewtoleft_have_client` (-1), `viewnext_have_client`/`viewtoright_have_client` (1) | `step: -1\|1` | Step to next occupied space in direction |
| `toggle-back` | `toggle_back`, `view_back`, `toggle-back` | - | Swap to previous view set |
| `cycle-layout` | `cycle_layout`, `cycle_layout_mode`, `cycle-layout` | - | Cycle layout within family |
| `cycle-layout-family` | `switch_layout`, `cycle_layout_family`, `cycle-layout-family` | - | Cycle to next family default |
| `set-layout` | `set_layout:<name>`, `set-layout:<name>` | mode or family name | Set layout for focused space |
| `toggle-floating` | `toogle_window_floating`, `toggle_window_floating`, `toggle_floating`, `togglefloating`, `toggle-floating` | - | Toggle floating |
| `toggle-fullscreen` | `toggle_fullscreen`, `togglefullscreen`, `toggle-fullscreen` | - | Toggle fullscreen |
| `toggle-maximized` | `toggle_maximize_window`, `toggle_maximized`, `togglemaximizescreen`, `toggle-maximized` | - | Toggle maximized |
| `preset-width` | `preset_width`, `switch_preset_width`, `preset-width` | - | Cycle scroll column width preset |
| `center-column` | `center_column`, `center-column` | - | Center focused scroll column |
| `reload-config` | `reload_config`, `reload-config` | - | Reload config file (hot reload) |
| `enter-mode` | `enter_mode:<name>`, `enter-mode:<name>`, `mode:<name>` | mode name | Enter a key mode |
| `leave-mode` | `leave_mode`, `leave-mode`, `exit_mode` | - | Reset to the default mode |
| `pop-mode` | `pop_mode`, `pop-mode` | - | Pop one mode off the stack |

**Unimplemented (accepted with warnings, skipped):** `toggle_overview`/`enteroverview`/`leaveoverview` (overview), `minimized`/`minimize`/`restore_minimized` (minimize), `toggle_scratchpad` (scratchpad).

## Spaces model

- Crab has **32 spaces per output** (fixed `SpaceSet::COUNT = 32`).
- **Dynamic spaces** (niri-style): a space exists if occupied or is the empty space past the last occupied one. Indices shown via IPC are **1-based dynamic indices** (dense). `SpaceRemap` renumbers on compaction; closing last window of focused space moves view to the space that takes its place. `spaces` IPC returns existing spaces only (union of occupied/visible), not all 32 slots.
- Binds/actions use 1-based dynamic indices (`1..=32` range enforced).

## Hot reload behavior

- Watched via `notify` on config's parent directory, debounced (~250ms).
- On change: read → parse → validate → apply atomically. 
- **Keyboard layout**: if `input.keyboard.xkb_rules_layout` changed, new keymap is compiled **before** swapping config; invalid keymap fails reload and keeps previous config/keymap.
- **Applied on reload**: binds/mouse/axis/gesture binds, per-mode tables, keyboard repeat, xkb layout (if changed), output positions (if model exists) and scales, config epoch/error state. Active modes that no longer exist are dropped. Triggers relayout.
- **Not re-evaluated automatically**: existing window rules (applied once at map time), output mode/transform in all backends, animations settings affect future events.
- **On failure**: error recorded, banner shown, IPC `ConfigError` event emitted, last good config remains in effect.
- **Manual**: `crab-cli config-reload`, IPC `{"type":"config-reload"}`, or bind `reload-config`.

## Status (what's applied vs parsed)

**Applied**: `[layout]` (gaps/borders/colors/`focus_follows_cursor`/`default_layout`/stack/tile/scroll params), `[animations]` (enabled/open/close), `[binds.*]` (including bind flags), `[modes.*]`, `[mousebinds]`, `[axisbinds]`, `[gesturebinds]`, `[autostart] exec_once`, `[[window_rule]]` matching/apply (app_id/title/is_floating/is_fullscreen + floating/size/offset/space/pinned/always_on_top/no_focus, plus dynamic re-evaluation of those state fields for rules with `dynamic`), `[layer_rule]` open animation by namespace, keyboard repeat/layout, output position/scale where applicable.

**Parsed/validated but not yet applied**: `[input.keyboard] numlock_on`, `[input.touchpad]` and `[input.mouse]` details (M4), `[output.*]` mode/transform (M4), `[layout] client_side_decorations`, `space_directions` (`*_next_space_direction`), `scroll.always_center_single_column`, some advanced rule fields (`opacity`, `blur`, `shadow`, `corner_radius`, `block_out_from`, `default_column_width`, etc.), the `x11 = true` rule matcher (no separate X11 surface tracking yet), and the lock/inhibitor bind flags. Startup logs these via `Config::pending_sections()`, so a non-empty list always means some configured behavior is missing.

## Defaults

From `Config::default()` / shipped `resources/default-config.toml`:
- Gaps 8, borders 2 (#7aa2f7 focused, #3b4261 unfocused)
- Layout tile (`fair` balanced grid)
- Animations enabled, slide/slide
- Focus follows cursor true, client side decorations false
- Scroll: center_focused_column never, always_center_single_column true, default_column_ratio 0.5; scroll family extends vertically by default
- Input: repeat_rate 25, repeat_delay 600, numlock_off, xkb us, touchpad natural scrolling/tap/drag on, mouse natural scrolling off
- No autostart/rules/binds beyond built-ins

## The shipped default config

`resources/default-config.toml` is a commented example (mango-style). Notable points:
- Uses `0`/`1` flags alongside comments; both accepted.
- Sets `[output.monitor]` as a placeholder (replace with actual connector like `eDP-1`/`DP-1`).
- Overrides `SUPER+space` to spawn `rofi` (replaces built-in cycle layout); built-in layout cycling remains on `SUPER+CTRL+space`/`SUPER+space` family depending on built-ins? Built-ins: `SUPER+CTRL+space` = cycle_layout_family, `SUPER+space` = cycle_layout_mode. Config overriding `SUPER+space` to spawn rofi takes precedence. Keyboard layout switching is still available via built-ins `SUPER+ALT+space` / `SUPER+ALT+SHIFT+space`.
- Includes window rule examples (floating with size/offset for Nautilus/Brave/Obsidian), a layer rule for rofi/alacritty zoom, autostart waybar, mouse/axis/gesture binds (all applied).
- Uses action aliases like `killclient`, `focusdir`, `move_client`, `view_space_left`, `toggle_window_floating`, `toggle_maximize_window` — all accepted.
- Shows bind flags and a `[modes.scroll.binds]` submap; `SUPER+ALT+s` enters it and `Escape` leaves it.
- Validate it: `cargo run --quiet -- --validate --config resources/default-config.toml` (should report "config valid").

To use: copy to `~/.config/crab/config.toml` and edit for your setup.
