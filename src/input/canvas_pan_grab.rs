//! rice: pointer grab that pans a canvas workspace by direct manipulation.
//!
//! Started by Mod+MMB anywhere on a canvas workspace and by Mod+LMB on the empty plane. The plane
//! follows the pointer 1:1 (plane delta = view delta / zoom); on release the view glides on with
//! the release velocity (`Workspace::canvas_pan_gesture_end`). Modelled on
//! `SpatialMovementGrab`: relative motion takes precedence over absolute motion, so that the
//! pointer can wrap around the output edges while panning.

use std::time::Duration;

use smithay::backend::input::InputTime;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
    MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
};
use smithay::input::SeatHandler;
use smithay::output::Output;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::layout::workspace::WorkspaceId;
use crate::niri::State;

pub struct CanvasPanGrab {
    start_data: PointerGrabStartData<State>,
    output: Output,
    last_location: Point<f64, Logical>,

    // Accumulated and applied in frame().
    new_location: Point<f64, Logical>,
    event_timestamp: Option<Duration>,
    relative_delta: Option<Point<f64, Logical>>,
}

impl CanvasPanGrab {
    /// Starts panning the canvas workspace `ws_id`. Returns `None` if it is not a canvas.
    pub fn new(
        state: &mut State,
        start_data: PointerGrabStartData<State>,
        output: Output,
        ws_id: WorkspaceId,
    ) -> Option<Self> {
        if !state.niri.layout.canvas_pan_gesture_begin(ws_id, false) {
            return None;
        }

        let location = start_data.location;
        Some(Self {
            start_data,
            output,
            last_location: location,
            new_location: location,
            event_timestamp: None,
            relative_delta: None,
        })
    }

    /// Output whose edges the pointer wraps around while panning.
    pub fn output(&self) -> &Output {
        &self.output
    }

    fn on_frame(&mut self, data: &mut State) -> bool {
        let Some(timestamp) = self.event_timestamp.take() else {
            return true;
        };

        let delta = self
            .relative_delta
            .take()
            .unwrap_or(self.new_location - self.last_location);
        self.last_location = self.new_location;

        // Dragging moves the content with the pointer, so the view moves the other way.
        match data
            .niri
            .layout
            .canvas_pan_gesture_update(delta.upscale(-1.), timestamp, false)
        {
            Some(output) => {
                if let Some(output) = output {
                    data.niri.queue_redraw(&output);
                }
                true
            }
            None => false,
        }
    }

    fn on_ungrab(&mut self, state: &mut State, time: Option<Duration>) {
        if let Some(Some(output)) = state.niri.layout.canvas_pan_gesture_end(time, Some(false)) {
            state.niri.queue_redraw(&output);
        }

        state
            .niri
            .cursor_manager
            .set_cursor_image(CursorImageStatus::default_named());
    }
}

impl PointerGrab<State> for CanvasPanGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus.
        handle.motion(data, None, event);

        self.new_location = event.location;

        // Relative motion takes precedence over normal motion.
        if self.relative_delta.is_none() {
            self.event_timestamp = Some(Duration::from_micros(event.time.micros()));
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        // While the grab is active, no client has pointer focus.
        handle.relative_motion(data, None, event);

        *self.relative_delta.get_or_insert_default() += event.delta;
        self.event_timestamp = Some(Duration::from_micros(event.time.micros()));
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        if handle.current_pressed().is_empty() {
            // Apply any motion of this frame before measuring the release velocity.
            self.on_frame(data);

            // No more buttons are pressed, release the grab and let the view glide.
            let time = Duration::from_micros(event.time.micros());
            self.on_ungrab(data, Some(time));
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);

        if !self.on_frame(data) {
            // The gesture is no longer ongoing (e.g. the workspace left canvas mode).
            handle.unset_grab(
                self,
                data,
                SERIAL_COUNTER.next_serial(),
                InputTime::now(),
                true,
            );
        }
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut State) {
        // Released through a button: already ended with a glide. Otherwise (grab replaced or
        // cancelled): stop in place.
        self.on_ungrab(data, None);
    }
}
