//! The surface everything is drawn on: shows a texture at an arbitrary zoom.
//!
//! The canvas implements `gtk::Scrollable`, so a `gtk::ScrolledWindow` drives
//! its adjustments. Content smaller than the viewport is centred; larger
//! content is shifted by the adjustment values. In fit mode the zoom follows
//! the allocation; any manual zoom leaves fit mode until `best_fit()`.

use relm4::gtk::{self, gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};

use crate::image::{ImageDocument, Orientation};

const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 32.0;
/// Above this zoom, pixels are drawn as crisp squares instead of smoothed.
const NEAREST_ABOVE: f64 = 2.0;
const KEY_ZOOM_STEP: f64 = 1.25;
const WHEEL_ZOOM_STEP: f64 = 0.1;

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::LumenCanvas)]
    pub struct LumenCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub orientation: Cell<Orientation>,
        pub zoom: Cell<f64>,
        pub fit: Cell<bool>,
        pub pointer: Cell<Option<(f64, f64)>>,
        pub pinch_start_zoom: Cell<f64>,

        #[property(override_interface = gtk::Scrollable, get, set = Self::set_hadjustment, explicit_notify, nullable)]
        pub hadjustment: RefCell<Option<gtk::Adjustment>>,
        #[property(override_interface = gtk::Scrollable, get, set = Self::set_vadjustment, explicit_notify, nullable)]
        pub vadjustment: RefCell<Option<gtk::Adjustment>>,
        #[property(override_interface = gtk::Scrollable, get, set, explicit_notify)]
        pub hscroll_policy: Cell<gtk::ScrollablePolicy>,
        #[property(override_interface = gtk::Scrollable, get, set, explicit_notify)]
        pub vscroll_policy: Cell<gtk::ScrollablePolicy>,
    }

    impl Default for LumenCanvas {
        fn default() -> Self {
            Self {
                texture: RefCell::default(),
                orientation: Cell::new(Orientation::NORMAL),
                zoom: Cell::new(1.0),
                fit: Cell::new(true),
                pointer: Cell::new(None),
                pinch_start_zoom: Cell::new(1.0),
                hadjustment: RefCell::default(),
                vadjustment: RefCell::default(),
                hscroll_policy: Cell::new(gtk::ScrollablePolicy::Minimum),
                vscroll_policy: Cell::new(gtk::ScrollablePolicy::Minimum),
            }
        }
    }

    impl LumenCanvas {
        fn set_hadjustment(&self, adjustment: Option<gtk::Adjustment>) {
            self.install_adjustment(&self.hadjustment, adjustment);
            self.obj().notify("hadjustment");
        }

        fn set_vadjustment(&self, adjustment: Option<gtk::Adjustment>) {
            self.install_adjustment(&self.vadjustment, adjustment);
            self.obj().notify("vadjustment");
        }

        fn install_adjustment(
            &self,
            slot: &RefCell<Option<gtk::Adjustment>>,
            adjustment: Option<gtk::Adjustment>,
        ) {
            if let Some(adj) = &adjustment {
                let canvas = self.obj();
                adj.connect_value_changed(glib::clone!(
                    #[weak]
                    canvas,
                    move |_| canvas.queue_draw()
                ));
            }
            *slot.borrow_mut() = adjustment;
            self.obj().update_adjustments();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LumenCanvas {
        const NAME: &'static str = "LumenCanvas";
        type Type = super::LumenCanvas;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::Scrollable,);
    }

    #[glib::derived_properties]
    impl ObjectImpl for LumenCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            let canvas = self.obj();
            canvas.set_overflow(gtk::Overflow::Hidden);
            canvas.set_hexpand(true);
            canvas.set_vexpand(true);

            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(glib::clone!(
                #[weak]
                canvas,
                move |_, x, y| canvas.imp().pointer.set(Some((x, y)))
            ));
            motion.connect_leave(glib::clone!(
                #[weak]
                canvas,
                move |_| canvas.imp().pointer.set(None)
            ));
            canvas.add_controller(motion);

            // Ctrl+scroll zooms; plain scroll falls through to the ScrolledWindow.
            let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            scroll.connect_scroll(glib::clone!(
                #[weak]
                canvas,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |ctrl, _, dy| {
                    if !ctrl
                        .current_event_state()
                        .contains(gdk::ModifierType::CONTROL_MASK)
                    {
                        return glib::Propagation::Proceed;
                    }
                    let factor = (-dy * WHEEL_ZOOM_STEP).exp();
                    let anchor = canvas.imp().pointer.get();
                    canvas.zoom_by(factor, anchor);
                    glib::Propagation::Stop
                }
            ));
            canvas.add_controller(scroll);

            // Drag to pan.
            let drag = gtk::GestureDrag::new();
            let start = Cell::new((0.0, 0.0));
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                canvas,
                #[strong]
                start,
                move |_, _, _| {
                    start.set((
                        canvas.adjustment_value(gtk::Orientation::Horizontal),
                        canvas.adjustment_value(gtk::Orientation::Vertical),
                    ));
                    if canvas.can_pan() {
                        canvas.set_cursor_from_name(Some("grabbing"));
                    }
                }
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                canvas,
                #[strong]
                start,
                move |_, dx, dy| {
                    let (sx, sy) = start.get();
                    canvas.set_adjustment_value(gtk::Orientation::Horizontal, sx - dx);
                    canvas.set_adjustment_value(gtk::Orientation::Vertical, sy - dy);
                }
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                canvas,
                move |_, _, _| canvas.update_cursor()
            ));
            canvas.add_controller(drag);

            // Touchpad / touchscreen pinch.
            let pinch = gtk::GestureZoom::new();
            pinch.connect_begin(glib::clone!(
                #[weak]
                canvas,
                move |_, _| {
                    let imp = canvas.imp();
                    imp.pinch_start_zoom.set(canvas.effective_zoom());
                }
            ));
            pinch.connect_scale_changed(glib::clone!(
                #[weak]
                canvas,
                move |gesture, scale| {
                    let anchor = gesture.bounding_box_center();
                    let start = canvas.imp().pinch_start_zoom.get();
                    canvas.set_zoom(start * scale, anchor);
                }
            ));
            canvas.add_controller(pinch);
        }
    }

    impl WidgetImpl for LumenCanvas {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (64, 256, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            self.obj().update_adjustments_for(width as f64, height as f64);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let canvas = self.obj();
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let zoom = canvas.effective_zoom();
            let (x, y) = canvas.origin();
            // Draw the stored pixels centred on the origin, then rotate/flip
            // them into place inside the (oriented) bounds.
            let (w, h) = (texture.width() as f64 * zoom, texture.height() as f64 * zoom);
            let (shown_w, shown_h) = if self.orientation.get().swaps_axes() { (h, w) } else { (w, h) };
            let bounds = graphene::Rect::new((-w / 2.0) as f32, (-h / 2.0) as f32, w as f32, h as f32);
            let filter = if zoom > NEAREST_ABOVE {
                gsk::ScalingFilter::Nearest
            } else {
                gsk::ScalingFilter::Trilinear
            };
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (x + shown_w / 2.0) as f32,
                (y + shown_h / 2.0) as f32,
            ));
            snapshot.transform(Some(&self.orientation.get().transform()));
            snapshot.append_scaled_texture(&texture, filter, &bounds);
            snapshot.restore();
        }
    }

    impl ScrollableImpl for LumenCanvas {}
}

glib::wrapper! {
    pub struct LumenCanvas(ObjectSubclass<imp::LumenCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl Default for LumenCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LumenCanvas {
    pub fn set_image(&self, image: Option<&ImageDocument>) {
        let imp = self.imp();
        let texture = image.map(|i| &i.texture);
        let orientation = image.map_or(Orientation::NORMAL, |i| i.orientation);
        if imp.texture.borrow().as_ref() != texture || imp.orientation.get() != orientation {
            *imp.texture.borrow_mut() = texture.cloned();
            imp.orientation.set(orientation);
            self.best_fit();
        }
    }

    /// The zoom actually used for drawing (the fit zoom while in fit mode).
    pub fn effective_zoom(&self) -> f64 {
        self.effective_zoom_for(self.width() as f64, self.height() as f64)
    }

    fn effective_zoom_for(&self, width: f64, height: f64) -> f64 {
        let imp = self.imp();
        match (imp.fit.get(), imp.texture.borrow().as_ref()) {
            (true, Some(_)) => {
                let (iw, ih) = self.texture_size().unwrap_or((1.0, 1.0));
                // Never upscale small images past 100 % just to fill the window.
                (width / iw).min(height / ih).min(1.0)
            }
            _ => imp.zoom.get(),
        }
    }

    pub fn best_fit(&self) {
        self.imp().fit.set(true);
        self.update_adjustments();
        self.queue_draw();
    }

    /// Zoom to `zoom`, keeping the image point under `anchor` (widget
    /// coordinates; defaults to the centre) where it is.
    pub fn set_zoom(&self, zoom: f64, anchor: Option<(f64, f64)>) {
        let imp = self.imp();
        if self.texture_size().is_none() {
            return;
        }
        let old = self.effective_zoom();
        let new = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let (ax, ay) = anchor.unwrap_or((self.width() as f64 / 2.0, self.height() as f64 / 2.0));

        // The image point currently under the anchor.
        let (ox, oy) = self.origin();
        let (px, py) = ((ax - ox) / old, (ay - oy) / old);

        imp.zoom.set(new);
        imp.fit.set(false);
        self.update_adjustments();

        // Scroll so that point lands back under the anchor. Axes where the
        // content fits are centred and ignore the adjustment.
        self.set_adjustment_value(gtk::Orientation::Horizontal, px * new - ax);
        self.set_adjustment_value(gtk::Orientation::Vertical, py * new - ay);
        self.queue_draw();
    }

    pub fn zoom_by(&self, factor: f64, anchor: Option<(f64, f64)>) {
        self.set_zoom(self.effective_zoom() * factor, anchor);
    }

    pub fn zoom_in(&self) {
        self.zoom_by(KEY_ZOOM_STEP, None);
    }

    pub fn zoom_out(&self) {
        self.zoom_by(1.0 / KEY_ZOOM_STEP, None);
    }

    pub fn actual_size(&self) {
        self.set_zoom(1.0, None);
    }

    fn texture_size(&self) -> Option<(f64, f64)> {
        let imp = self.imp();
        imp.texture.borrow().as_ref().map(|t| {
            let (w, h) = (t.width() as f64, t.height() as f64);
            if imp.orientation.get().swaps_axes() { (h, w) } else { (w, h) }
        })
    }

    fn adjustment(&self, orientation: gtk::Orientation) -> Option<gtk::Adjustment> {
        let imp = self.imp();
        match orientation {
            gtk::Orientation::Horizontal => imp.hadjustment.borrow().clone(),
            _ => imp.vadjustment.borrow().clone(),
        }
    }

    fn adjustment_value(&self, orientation: gtk::Orientation) -> f64 {
        self.adjustment(orientation).map_or(0.0, |a| a.value())
    }

    /// Set an adjustment value (the adjustment clamps it to its range).
    fn set_adjustment_value(&self, orientation: gtk::Orientation, value: f64) {
        if let Some(adj) = self.adjustment(orientation) {
            adj.set_value(value);
        }
    }

    /// Top-left corner of the image in widget coordinates.
    fn origin(&self) -> (f64, f64) {
        let zoom = self.effective_zoom();
        let (tw, th) = self.texture_size().unwrap_or((0.0, 0.0));
        let axis = |content: f64, view: f64, orientation| {
            if content < view {
                (view - content) / 2.0
            } else {
                -self.adjustment_value(orientation)
            }
        };
        (
            axis(tw * zoom, self.width() as f64, gtk::Orientation::Horizontal),
            axis(th * zoom, self.height() as f64, gtk::Orientation::Vertical),
        )
    }

    fn can_pan(&self) -> bool {
        let zoom = self.effective_zoom();
        self.texture_size().is_some_and(|(tw, th)| {
            tw * zoom > self.width() as f64 + 0.5 || th * zoom > self.height() as f64 + 0.5
        })
    }

    fn update_cursor(&self) {
        self.set_cursor_from_name(self.can_pan().then_some("grab"));
    }

    fn update_adjustments(&self) {
        self.update_adjustments_for(self.width() as f64, self.height() as f64);
    }

    /// Resize both adjustments to match the content and the viewport.
    fn update_adjustments_for(&self, width: f64, height: f64) {
        let zoom = self.effective_zoom_for(width, height);
        let (tw, th) = self.texture_size().unwrap_or((0.0, 0.0));
        for (orientation, content, view) in [
            (gtk::Orientation::Horizontal, tw * zoom, width),
            (gtk::Orientation::Vertical, th * zoom, height),
        ] {
            if let Some(adj) = self.adjustment(orientation) {
                let upper = content.max(view);
                let value = adj.value().clamp(0.0, upper - view);
                adj.configure(value, 0.0, upper, view * 0.1, view * 0.9, view);
            }
        }
        self.update_cursor();
        self.queue_draw();
    }
}
