//! Touchpad pinch gestures (`zwp_pointer_gestures_v1`).
//!
//! Patched in by Lumen: winit 0.30 only reports pinch gestures on macOS and iOS. The
//! compositor sends the total scale of the pinch since it began; winit's `PinchGesture`
//! carries the change since the last event, as on macOS.

use std::sync::Mutex;

use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_pointer::WlPointer;
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
};

use crate::event::{TouchPhase, WindowEvent};
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::{self, DeviceId, WindowId};

/// The compositor's gesture manager.
pub struct PointerGesturesState {
    manager: ZwpPointerGesturesV1,
}

impl PointerGesturesState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=3, GlobalData)?;
        Ok(Self { manager })
    }

    /// Pinch gestures made with `pointer`.
    pub fn get_pinch(
        &self,
        pointer: &WlPointer,
        queue_handle: &QueueHandle<WinitState>,
    ) -> ZwpPointerGesturePinchV1 {
        self.manager.get_pinch_gesture(pointer, queue_handle, PinchData::default())
    }
}

/// The window a pinch happens over, and its scale so far.
#[derive(Default)]
pub struct PinchData {
    inner: Mutex<(Option<WindowId>, f64)>,
}

impl Dispatch<ZwpPointerGesturesV1, GlobalData, WinitState> for PointerGesturesState {
    fn event(
        _state: &mut WinitState,
        _proxy: &ZwpPointerGesturesV1,
        _event: <ZwpPointerGesturesV1 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, PinchData, WinitState> for PointerGesturesState {
    fn event(
        state: &mut WinitState,
        _proxy: &ZwpPointerGesturePinchV1,
        event: <ZwpPointerGesturePinchV1 as Proxy>::Event,
        data: &PinchData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        let mut inner = data.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (window_id, delta, phase) = match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { surface, .. } => {
                let window_id = wayland::make_wid(&surface);
                *inner = (Some(window_id), 1.0);
                (window_id, 0.0, TouchPhase::Started)
            },
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                let Some(window_id) = inner.0 else { return };
                let delta = scale - inner.1;
                inner.1 = scale;
                (window_id, delta, TouchPhase::Moved)
            },
            zwp_pointer_gesture_pinch_v1::Event::End { cancelled, .. } => {
                let Some(window_id) = inner.0.take() else { return };
                let phase = if cancelled != 0 { TouchPhase::Cancelled } else { TouchPhase::Ended };
                (window_id, 0.0, phase)
            },
            _ => return,
        };
        drop(inner);
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        state
            .events_sink
            .push_window_event(WindowEvent::PinchGesture { device_id, delta, phase }, window_id);
    }
}

delegate_dispatch!(WinitState: [ZwpPointerGesturesV1: GlobalData] => PointerGesturesState);
delegate_dispatch!(WinitState: [ZwpPointerGesturePinchV1: PinchData] => PointerGesturesState);
