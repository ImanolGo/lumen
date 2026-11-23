# Patched dependencies

Lumen supports pinch-to-zoom on touchpads. Upstream, that is not possible yet on
Linux: winit 0.30 reports pinch gestures only on macOS and iOS, and iced 0.14 does
not pass them on at all. These three crates are the released versions with small
patches (the exact changes are in `patches/`), used through `[patch.crates-io]` in
`Cargo.toml`:

| Crate | Version | Change |
|---|---|---|
| `winit` | 0.30.13 | Wayland: binds `zwp_pointer_gestures_v1` and reports pinches as `WindowEvent::PinchGesture` (new `seat/pointer/gestures.rs`, a few lines in `seat/mod.rs` and `state.rs`) |
| `iced_core` | 0.14.0 | `mouse::Event::Pinched { delta }` |
| `iced_winit` | 0.14.1 | Converts `WindowEvent::PinchGesture` into `mouse::Event::Pinched`; silences two upstream deprecation warnings |

Everything else is untouched. When upstream gains the same support, drop the
`[patch.crates-io]` section and this folder. To move to a newer release, copy it
from `~/.cargo/registry/src/` and apply the patches again.

X11 has no pinch support here (it would need XInput 2.4 gesture events in winit's
X11 backend).
