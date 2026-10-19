//! The surface everything is drawn on: shows a texture at an arbitrary zoom.
//!
//! The image centre sits at the widget centre plus `offset`. In fit mode the
//! zoom follows the allocation and the offset is zero; any manual zoom leaves
//! fit mode until `best_fit()` is called again.

use relm4::gtk::{self, gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};

const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 32.0;
/// Above this zoom, pixels are drawn as crisp squares instead of smoothed.
const NEAREST_ABOVE: f64 = 2.0;
const KEY_ZOOM_STEP: f64 = 1.25;
const WHEEL_ZOOM_STEP: f64 = 0.1;

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    pub struct LumenCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub zoom: Cell<f64>,
        pub fit: Cell<bool>,
        pub offset: Cell<(f64, f64)>,
        pub pointer: Cell<Option<(f64, f64)>>,
    }

    impl Default for LumenCanvas {
        fn default() -> Self {
            Self {
                texture: RefCell::default(),
                zoom: Cell::new(1.0),
                fit: Cell::new(true),
                offset: Cell::new((0.0, 0.0)),
                pointer: Cell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LumenCanvas {
        const NAME: &'static str = "LumenCanvas";
        type Type = super::LumenCanvas;
        type ParentType = gtk::Widget;
    }

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
        }
    }

    impl WidgetImpl for LumenCanvas {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (64, 256, -1, -1)
        }

        fn size_allocate(&self, _width: i32, _height: i32, _baseline: i32) {
            if self.fit.get() {
                self.obj().queue_draw();
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let canvas = self.obj();
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let (w, h) = (canvas.width() as f64, canvas.height() as f64);
            let zoom = canvas.effective_zoom();
            let (ox, oy) = if self.fit.get() { (0.0, 0.0) } else { self.offset.get() };

            let (iw, ih) = (texture.width() as f64 * zoom, texture.height() as f64 * zoom);
            let bounds = graphene::Rect::new(
                (w / 2.0 + ox - iw / 2.0) as f32,
                (h / 2.0 + oy - ih / 2.0) as f32,
                iw as f32,
                ih as f32,
            );
            let filter = if zoom > NEAREST_ABOVE {
                gsk::ScalingFilter::Nearest
            } else {
                gsk::ScalingFilter::Trilinear
            };
            snapshot.append_scaled_texture(&texture, filter, &bounds);
        }
    }
}

glib::wrapper! {
    pub struct LumenCanvas(ObjectSubclass<imp::LumenCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for LumenCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LumenCanvas {
    pub fn set_texture(&self, texture: Option<&gdk::Texture>) {
        let imp = self.imp();
        let changed = imp.texture.borrow().as_ref() != texture;
        if changed {
            *imp.texture.borrow_mut() = texture.cloned();
            self.best_fit();
        }
    }

    /// The zoom actually used for drawing (the fit zoom while in fit mode).
    pub fn effective_zoom(&self) -> f64 {
        let imp = self.imp();
        match (imp.fit.get(), imp.texture.borrow().as_ref()) {
            (true, Some(t)) => {
                let (w, h) = (self.width() as f64, self.height() as f64);
                // Never upscale small images past 100 % just to fill the window.
                (w / t.width() as f64).min(h / t.height() as f64).min(1.0)
            }
            _ => imp.zoom.get(),
        }
    }

    pub fn best_fit(&self) {
        let imp = self.imp();
        imp.fit.set(true);
        imp.offset.set((0.0, 0.0));
        self.queue_draw();
    }

    pub fn set_zoom(&self, zoom: f64, anchor: Option<(f64, f64)>) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let old = self.effective_zoom();
        let new = zoom.clamp(MIN_ZOOM, MAX_ZOOM);

        // Keep the image point under `anchor` (default: widget centre) fixed.
        let (cx, cy) = (self.width() as f64 / 2.0, self.height() as f64 / 2.0);
        let (ax, ay) = anchor.unwrap_or((cx, cy));
        let (ox, oy) = if imp.fit.get() { (0.0, 0.0) } else { imp.offset.get() };
        let ratio = new / old;
        imp.offset.set((
            ax - cx + (ox + cx - ax) * ratio,
            ay - cy + (oy + cy - ay) * ratio,
        ));
        imp.zoom.set(new);
        imp.fit.set(false);
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
}
