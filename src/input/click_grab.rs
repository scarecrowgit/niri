use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::input::SeatHandler;
use smithay::utils::{Logical, Point};

// ClickGrab from Smithay, but without the grab location updates.
//
// When clicking on a partially off-screen window, such that it will scroll into the view, updating
// the focus location will cause a move event, essentially a short DnD, which might result in some
// unintended action.
//
// Not updating in this case avoids any unintentional DnD. It does cause offset cursor movement if
// you hold and keep moving, but that can't cause mistakes as easily, so it's the lesser evil.
pub struct ClickGrab<D: SeatHandler> {
    start_data: GrabStartData<D>,
    focus: Option<(D::PointerFocus, Point<f64, Logical>)>,
    /// rice: origin (global, as drawn) and zoom of a focused surface drawn scaled (zoomed
    /// canvas).
    scaled: Option<(Point<f64, Logical>, f64)>,
}

impl<D: SeatHandler> ClickGrab<D> {
    /// Creates the grab; `zoom` is the scale the focused surface is drawn at, if it is drawn
    /// scaled.
    ///
    /// The focus location of a scaled surface is chosen such that smithay's `pos - location`
    /// gives the scaled-down local position (see `niri::scaled_surface_under()`), so it has to
    /// follow the pointer: keeping it fixed would move the pointer within the surface at the
    /// unscaled speed while dragging.
    pub fn new(start_data: GrabStartData<D>, zoom: Option<f64>) -> Self {
        let scaled = zoom
            .filter(|zoom| *zoom > 0. && zoom.is_finite())
            .and_then(|zoom| {
                let (_, loc) = start_data.focus.as_ref()?;
                let pos = start_data.location;
                Some((pos - (pos - *loc).upscale(zoom), zoom))
            });
        Self {
            focus: start_data.focus.clone(),
            start_data,
            scaled,
        }
    }

    fn focus_at(&self, pos: Point<f64, Logical>) -> Option<(D::PointerFocus, Point<f64, Logical>)> {
        let (focus, loc) = self.focus.clone()?;
        let loc = match self.scaled {
            Some((origin, zoom)) => pos - (pos - origin).downscale(zoom),
            None => loc,
        };
        Some((focus, loc))
    }
}

impl<D: SeatHandler + 'static> PointerGrab<D> for ClickGrab<D> {
    fn motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        _focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, self.focus_at(event.location), event);
    }

    fn relative_motion(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        focus: Option<(<D as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // no more buttons are pressed, release the grab
            handle.unset_grab(self, data, event.serial, event.time, false);
        }
    }

    fn axis(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut D, handle: &mut PointerInnerHandle<'_, D>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut D,
        handle: &mut PointerInnerHandle<'_, D>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<D> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut D) {}
}
