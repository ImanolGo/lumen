//! The surface everything is drawn on: shows an image or a PDF page at an arbitrary zoom.
//!
//! The canvas implements `gtk::Scrollable`, so a `gtk::ScrolledWindow` drives
//! its adjustments. Content smaller than the viewport is centred; larger
//! content is shifted by the adjustment values. In fit mode the zoom follows
//! the allocation; any manual zoom leaves fit mode until `best_fit()`.
//!
//! Content has a *logical* size (image pixels, or PDF points) that zoom is
//! applied to. A PDF page's texture may have more pixels than that, rendered
//! for the current zoom; it is simply stretched over the logical rectangle, so
//! an outdated texture still shows (blurry) until the fresh one arrives.

use relm4::{
    adw::{self, prelude::*},
    gtk::{self, gdk, glib, graphene, gsk, subclass::prelude::*},
};

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use crate::{
    annotate::{self, Store, Tool, render as annot_render},
    edit::{
        CropRect,
        crop::{self, Grab, Rect},
    },
    image::{Animation, ImageDocument, Orientation},
    pdf::{FieldKind, FormField, HitRect, Link, LinkTarget, PageLayout, PagePoint},
    settings::Background,
};

const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 32.0;
/// Above this zoom, pixels are drawn as crisp squares instead of smoothed.
const NEAREST_ABOVE: f64 = 2.0;
/// What the canvas shows.
pub enum CanvasContent<'a> {
    None,
    Image(&'a ImageDocument),
    /// All pages of a PDF stacked vertically. `key` identifies the document;
    /// textures arrive later through `set_page_texture`.
    Pages {
        key: u64,
        layout: &'a Rc<PageLayout>,
    },
}

/// What the user is doing with the pointer on a PDF page.
#[derive(Debug, Clone)]
pub enum SelectGesture {
    /// Pressed on a page; `clicks` is 1 for a single click, 2 for a double click, 3 for a triple.
    Start { point: PagePoint, clicks: u32 },
    /// The pointer moved while pressed.
    Extend(PagePoint),
    /// Released; `moved` is false for a plain click.
    End { moved: bool },
    /// A plain click on a link.
    OpenLink(LinkTarget),
    /// A plain click on a form field that can be filled in.
    Form { page: usize, field: FormField },
}

/// How the pointer is used on the canvas with regard to markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkupMode {
    /// Markup is closed.
    #[default]
    Off,
    /// Pointer events draw and edit annotations with this tool.
    Draw(Tool),
    /// Text is selected as usual and the selection becomes a highlight (PDF pages).
    TextSelect,
}

/// A pointer event on the canvas in markup mode, in document coordinates: pixels of
/// the displayed image, or points on a PDF page.
#[derive(Debug, Clone, Copy)]
pub enum MarkupEvent {
    Press {
        page: usize,
        point: annotate::Point,
        /// Reach for hitting annotations, in document units.
        tolerance: f64,
        clicks: u32,
    },
    Drag(annotate::Point),
    Release(annotate::Point),
}

/// Screen pixels within which a click hits an annotation or one of its handles.
const MARKUP_REACH: f64 = 7.0;

/// What is on screen, reported to the owner so it can render what is needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewInfo {
    pub zoom: f64,
    pub scale_factor: i32,
    /// Pages touching the viewport (always 0 for images).
    pub first_visible: usize,
    pub last_visible: usize,
    /// The page filling most of the viewport.
    pub current_page: usize,
    /// Where a picture is scrolled to, in steps of `PAN_STEP` pixels (always 0 for PDFs), so
    /// a drawing shown sharp can be rendered again where the view has moved.
    pub pan: (i64, i64),
}

const PAN_STEP: f64 = 128.0;

impl ViewInfo {
    fn same_as(&self, other: &Self) -> bool {
        (self.zoom - other.zoom).abs() < 1e-4
            && self.scale_factor == other.scale_factor
            && self.first_visible == other.first_visible
            && self.last_visible == other.last_visible
            && self.current_page == other.current_page
            && self.pan == other.pan
    }
}

const SELECTION_COLOR: gdk::RGBA = gdk::RGBA::new(0.2, 0.5, 1.0, 0.35);
const HIT_OTHER: gdk::RGBA = gdk::RGBA::new(1.0, 0.85, 0.0, 0.45);
const HIT_CURRENT: gdk::RGBA = gdk::RGBA::new(1.0, 0.45, 0.0, 0.6);
/// The tint and edge of form fields that can be filled in.
const FORM_FILL: gdk::RGBA = gdk::RGBA::new(0.21, 0.52, 0.89, 0.10);
const FORM_EDGE: gdk::RGBA = gdk::RGBA::new(0.21, 0.52, 0.89, 0.45);
/// The soft shadow under each PDF page, so pages read as paper on any background.
const PAGE_SHADOW: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.22);

/// Fit-to-width never zooms a document past this.
const MAX_PAGES_FIT_ZOOM: f64 = 3.0;

const ZOOM_ANIMATION_MS: u32 = 130;
const KEY_ZOOM_STEP: f64 = 1.25;
const WHEEL_ZOOM_STEP: f64 = 0.1;

type ViewCallback = Box<dyn Fn(ViewInfo)>;
type SelectCallback = Box<dyn Fn(SelectGesture)>;
type MarkupCallback = Box<dyn Fn(MarkupEvent)>;

/// The crop rectangle being edited, in pixels of the image as displayed.
pub struct CropState {
    rect: Rect,
    aspect: Option<f64>,
    /// The drag in progress: what is grabbed and the rectangle when it began.
    drag: Option<(Grab, Rect)>,
}

/// How close (in screen pixels) the pointer must be to grab a crop edge or corner.
const CROP_REACH: f64 = 12.0;
const CROP_HANDLE: f32 = 10.0;

pub struct PagesState {
    layout: Rc<PageLayout>,
    textures: HashMap<usize, gdk::Texture>,
    /// Search hits per page.
    highlights: HashMap<usize, Vec<HitRect>>,
    /// The hit that is selected: page and index within that page's hits.
    current_hit: Option<(usize, usize)>,
    /// Selected text rectangles per page.
    selection: HashMap<usize, Vec<HitRect>>,
    /// Clickable links per page.
    links: HashMap<usize, Vec<Link>>,
    /// Form fields per page.
    forms: HashMap<usize, Vec<FormField>>,
}

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::LumenCanvas)]
    pub struct LumenCanvas {
        /// What is drawn: the picture, or the current frame of an animation.
        pub texture: RefCell<Option<gdk::Texture>>,
        /// The picture being shown (its first frame), to tell a new picture from the next frame.
        pub source: RefCell<Option<gdk::Texture>>,
        pub animation: RefCell<Option<Animation>>,
        /// A drawing rendered again, sharp, for part of the picture: the texture and the area
        /// (x, y, w, h) it covers, in the picture's own pixels.
        pub sharp: RefCell<Option<(gdk::Texture, [f64; 4])>>,
        pub frame: Cell<usize>,
        /// Bumped whenever the content changes; timers of an older animation stop.
        pub animation_generation: Cell<u64>,
        pub orientation: Cell<Orientation>,
        /// Unoriented size the zoom applies to; `None` when there is no content.
        pub logical: Cell<Option<(f64, f64)>>,
        /// Draw pixels as hard squares when zoomed in (images, not PDF pages).
        pub crisp: Cell<bool>,
        pub content_key: Cell<u64>,
        pub pages: RefCell<Option<PagesState>>,
        pub last_view: Cell<Option<ViewInfo>>,
        pub view_callback: RefCell<Option<ViewCallback>>,
        pub select_callback: RefCell<Option<SelectCallback>>,
        pub crop: RefCell<Option<CropState>>,
        pub background: Cell<Background>,
        /// Show new images at 100 % instead of fitted to the window.
        pub start_actual: Cell<bool>,
        pub smooth_zoom: Cell<bool>,
        pub zoom_animation: RefCell<Option<adw::TimedAnimation>>,
        /// Where a running zoom animation is heading, so quick repeats add up.
        pub zoom_target: Cell<f64>,
        /// Hide the pointer over the canvas (fullscreen, when it has been still for a while).
        pub cursor_hidden: Cell<bool>,
        pub markup_store: RefCell<Option<Rc<RefCell<Store>>>>,
        pub markup_mode: Cell<MarkupMode>,
        pub markup_callback: RefCell<Option<MarkupCallback>>,
        /// A pan drag is in progress (the hover cursor must not override "grabbing").
        pub dragging: Cell<bool>,
        /// Time (ms), position and count of the last press, to detect double and triple clicks.
        pub last_press: Cell<Option<(u32, f64, f64, u32)>>,
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
                source: RefCell::default(),
                animation: RefCell::default(),
                sharp: RefCell::default(),
                frame: Cell::new(0),
                animation_generation: Cell::new(0),
                orientation: Cell::new(Orientation::NORMAL),
                logical: Cell::new(None),
                crisp: Cell::new(true),
                content_key: Cell::new(0),
                pages: RefCell::default(),
                last_view: Cell::new(None),
                view_callback: RefCell::default(),
                select_callback: RefCell::default(),
                crop: RefCell::default(),
                background: Cell::new(Background::System),
                start_actual: Cell::new(false),
                smooth_zoom: Cell::new(true),
                zoom_animation: RefCell::default(),
                zoom_target: Cell::new(1.0),
                cursor_hidden: Cell::new(false),
                markup_store: RefCell::default(),
                markup_mode: Cell::new(MarkupMode::Off),
                markup_callback: RefCell::default(),
                dragging: Cell::new(false),
                last_press: Cell::new(None),
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
                    move |_| {
                        canvas.queue_draw();
                        canvas.notify_view();
                    }
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
                move |_, x, y| {
                    canvas.imp().pointer.set(Some((x, y)));
                    canvas.update_hover_cursor(x, y);
                }
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

            // Drag to pan: the primary button on images, the middle button anywhere.
            canvas.add_pan_gesture(gdk::BUTTON_PRIMARY, false);
            canvas.add_pan_gesture(gdk::BUTTON_MIDDLE, true);
            canvas.add_select_gesture();
            canvas.add_crop_gesture();
            canvas.add_markup_gesture();
            canvas.set_focusable(true);

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
            self.obj()
                .update_adjustments_for(width as f64, height as f64);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let canvas = self.obj();
            let Some((logical_w, logical_h)) = self.logical.get() else {
                return;
            };
            canvas.snapshot_background(snapshot);
            let zoom = canvas.effective_zoom();
            let (x, y) = canvas.origin();
            if let Some(pages) = self.pages.borrow().as_ref() {
                canvas.snapshot_pages(snapshot, pages, zoom, (x, y));
                return;
            }
            // Draw the stored pixels centred on the origin, then rotate/flip
            // them into place inside the (oriented) bounds.
            let (w, h) = (logical_w * zoom, logical_h * zoom);
            let (shown_w, shown_h) = if self.orientation.get().swaps_axes() {
                (h, w)
            } else {
                (w, h)
            };
            let bounds =
                graphene::Rect::new((-w / 2.0) as f32, (-h / 2.0) as f32, w as f32, h as f32);

            if self.background.get() == Background::Checkerboard {
                canvas.snapshot_checkerboard(
                    snapshot,
                    &graphene::Rect::new(x as f32, y as f32, shown_w as f32, shown_h as f32),
                );
            }
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (x + shown_w / 2.0) as f32,
                (y + shown_h / 2.0) as f32,
            ));
            snapshot.transform(Some(&self.orientation.get().transform()));
            match self.texture.borrow().as_ref() {
                Some(texture) => {
                    let filter = if self.crisp.get() && zoom > NEAREST_ABOVE {
                        gsk::ScalingFilter::Nearest
                    } else {
                        gsk::ScalingFilter::Trilinear
                    };
                    snapshot.append_scaled_texture(texture, filter, &bounds);
                    // A drawing: the part in view, rendered again at this zoom, over the pixels.
                    if let Some((tile, [tx, ty, tw, th])) = self.sharp.borrow().as_ref() {
                        let area = graphene::Rect::new(
                            (bounds.x() as f64 + tx * zoom) as f32,
                            (bounds.y() as f64 + ty * zoom) as f32,
                            (tw * zoom) as f32,
                            (th * zoom) as f32,
                        );
                        snapshot.append_scaled_texture(tile, gsk::ScalingFilter::Linear, &area);
                    }
                }
                // A page that has not been rendered yet: blank paper.
                None => snapshot.append_color(&gdk::RGBA::WHITE, &bounds),
            }
            snapshot.restore();
            canvas.snapshot_crop(snapshot, zoom, (x, y));
            canvas.snapshot_markup(snapshot, 0, (x, y), zoom);
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
    pub fn set_content(&self, content: CanvasContent) {
        let imp = self.imp();
        match content {
            CanvasContent::None => {
                self.stop_animation();
                if imp.logical.get().is_some() {
                    *imp.pages.borrow_mut() = None;
                    *imp.texture.borrow_mut() = None;
                    imp.logical.set(None);
                    self.best_fit();
                }
            }
            CanvasContent::Image(image) => {
                let unchanged = imp.crisp.get()
                    && imp.logical.get().is_some()
                    && imp.source.borrow().as_ref() == Some(&image.texture)
                    && imp.orientation.get() == image.orientation;
                if !unchanged {
                    self.stop_animation();
                    *imp.pages.borrow_mut() = None;
                    *imp.texture.borrow_mut() = Some(image.texture.clone());
                    *imp.source.borrow_mut() = Some(image.texture.clone());
                    imp.sharp.replace(None);
                    *imp.animation.borrow_mut() = image.animation.clone();
                    self.next_frame_later();
                    imp.orientation.set(image.orientation);
                    imp.logical.set(Some((
                        image.texture.width() as f64,
                        image.texture.height() as f64,
                    )));
                    imp.crisp.set(true);
                    self.new_content();
                }
            }
            CanvasContent::Pages { key, layout } => {
                let same = imp.pages.borrow().is_some() && imp.content_key.get() == key;
                if !same {
                    // A new version of the same pages (a form filled in, an undo) keeps the
                    // zoom and scroll position; anything else starts fitted, at the top.
                    let keep_view = imp
                        .pages
                        .borrow()
                        .as_ref()
                        .is_some_and(|old| old.layout.rects == layout.rects);
                    // The old pictures of the pages stand in until the new ones are rendered,
                    // so a form filled in or an undo does not flash white pages.
                    let stand_ins = if keep_view {
                        imp.pages
                            .borrow_mut()
                            .as_mut()
                            .map(|old| std::mem::take(&mut old.textures))
                            .unwrap_or_default()
                    } else {
                        HashMap::new()
                    };
                    self.stop_animation();
                    *imp.texture.borrow_mut() = None;
                    *imp.source.borrow_mut() = None;
                    imp.orientation.set(Orientation::NORMAL);
                    imp.logical.set(Some(layout.size));
                    imp.crisp.set(false);
                    imp.content_key.set(key);
                    *imp.pages.borrow_mut() = Some(PagesState {
                        layout: Rc::clone(layout),
                        textures: stand_ins,
                        highlights: HashMap::new(),
                        current_hit: None,
                        selection: HashMap::new(),
                        links: HashMap::new(),
                        forms: HashMap::new(),
                    });
                    if keep_view {
                        // Report the view again, so the new document's pages get rendered.
                        imp.last_view.set(None);
                        self.notify_view();
                        self.queue_draw();
                    } else {
                        self.new_content();
                    }
                }
            }
        }
    }

    /// Show `tile`, the drawing rendered again for `area` (x, y, w, h in the picture's pixels).
    pub fn set_sharp(&self, tile: Option<(gdk::Texture, [f64; 4])>) {
        self.imp().sharp.replace(tile);
        self.queue_draw();
    }

    /// The part of the picture in view, with `margin` (a fraction of the view) around it, in
    /// the picture's own pixels; `None` unless a picture is shown upright.
    pub fn visible_image_area(&self, margin: f64) -> Option<[f64; 4]> {
        let imp = self.imp();
        if imp.pages.borrow().is_some() || imp.orientation.get() != Orientation::NORMAL {
            return None;
        }
        let (w, h) = imp.logical.get()?;
        let zoom = self.effective_zoom();
        let (ox, oy) = self.origin();
        let (vw, vh) = (self.width() as f64 / zoom, self.height() as f64 / zoom);
        let x1 = ((-ox / zoom) - vw * margin).max(0.0);
        let y1 = ((-oy / zoom) - vh * margin).max(0.0);
        let x2 = ((-ox / zoom) + vw * (1.0 + margin)).min(w);
        let y2 = ((-oy / zoom) + vh * (1.0 + margin)).min(h);
        (x2 > x1 && y2 > y1).then_some([x1, y1, x2 - x1, y2 - y1])
    }

    /// Forget the animation being played, if any; its pending timer becomes a no-op.
    fn stop_animation(&self) {
        let imp = self.imp();
        imp.animation_generation
            .set(imp.animation_generation.get() + 1);
        imp.animation.replace(None);
        imp.frame.set(0);
    }

    /// Show the next frame of the animation once the current one has had its time.
    fn next_frame_later(&self) {
        let imp = self.imp();
        let Some(animation) = imp.animation.borrow().clone() else {
            return;
        };
        let generation = imp.animation_generation.get();
        // Like browsers, treat delays of 10 ms or less as 100 ms: old GIFs rely on it.
        let delay = animation.frames[imp.frame.get()].1;
        let delay = if delay <= std::time::Duration::from_millis(10) {
            std::time::Duration::from_millis(100)
        } else {
            delay
        };
        let canvas = self.downgrade();
        glib::timeout_add_local_once(delay, move || {
            let Some(canvas) = canvas.upgrade() else {
                return;
            };
            let imp = canvas.imp();
            if imp.animation_generation.get() != generation {
                return;
            }
            let next = (imp.frame.get() + 1) % animation.frames.len();
            imp.frame.set(next);
            imp.texture.replace(Some(animation.frames[next].0.clone()));
            canvas.queue_draw();
            canvas.next_frame_later();
        });
    }

    /// Show `texture` for page `index` (or blank paper for `None`).
    pub fn set_page_texture(&self, index: usize, texture: Option<gdk::Texture>) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            match texture {
                Some(t) => pages.textures.insert(index, t),
                None => pages.textures.remove(&index),
            };
            self.queue_draw();
        }
    }

    /// Scroll so that page `index` starts at the top of the viewport.
    pub fn scroll_to_page(&self, index: usize) {
        self.scroll_to_point(index, None);
    }

    /// Scroll down (`direction` 1) or up (-1) by nearly one screen. Returns whether the view
    /// moved, so the caller can go on to the next file at the end.
    pub fn scroll_screen(&self, direction: i32) -> bool {
        let Some(adj) = self.adjustment(gtk::Orientation::Vertical) else {
            return false;
        };
        let before = adj.value();
        adj.set_value(before + direction as f64 * adj.page_increment());
        (adj.value() - before).abs() > 0.5
    }

    /// Scroll to `y` points below the top of page `index` (the page top if `None`).
    pub fn scroll_to_point(&self, index: usize, y: Option<f64>) {
        let top = {
            let pages = self.imp().pages.borrow();
            pages
                .as_ref()
                .and_then(|p| {
                    p.layout
                        .rects
                        .get(index.min(p.layout.rects.len().saturating_sub(1)))
                })
                .map(|r| r.y)
        };
        if let Some(top) = top {
            // Leave a sliver above the target visible.
            let margin = crate::pdf::layout::PAGE_GAP / 2.0;
            let value = (top + y.unwrap_or(0.0) - margin) * self.effective_zoom();
            self.set_adjustment_value(gtk::Orientation::Vertical, value);
        }
    }

    fn add_pan_gesture(&self, button: u32, also_on_pages: bool) {
        let drag = gtk::GestureDrag::builder().button(button).build();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            start,
            move |gesture, _, _| {
                // On PDF pages the primary button selects text instead.
                if (canvas.has_pages() || canvas.in_crop_mode() || canvas.markup_draws())
                    && !also_on_pages
                {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                start.set((
                    canvas.adjustment_value(gtk::Orientation::Horizontal),
                    canvas.adjustment_value(gtk::Orientation::Vertical),
                ));
                canvas.imp().dragging.set(true);
                if canvas.can_pan() {
                    canvas.set_cursor_from_name(Some("grabbing"));
                }
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            start,
            move |_, dx, dy| {
                let (sx, sy) = start.get();
                canvas.set_adjustment_value(gtk::Orientation::Horizontal, sx - dx);
                canvas.set_adjustment_value(gtk::Orientation::Vertical, sy - dy);
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _, _| {
                canvas.imp().dragging.set(false);
                canvas.update_cursor();
            }
        ));
        self.add_controller(drag);
    }

    /// Primary-button drags on PDF pages select text.
    fn add_select_gesture(&self) {
        let drag = gtk::GestureDrag::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        let origin = Rc::new(Cell::new((0.0, 0.0)));
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            origin,
            move |gesture, x, y| {
                if !canvas.has_pages() || canvas.markup_draws() {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                canvas.grab_focus();
                origin.set((x, y));
                let clicks = canvas.count_click(gesture.current_event_time(), x, y);
                if let Some(point) = canvas.page_point_at(x, y) {
                    canvas.emit_select(SelectGesture::Start { point, clicks });
                }
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            origin,
            move |_, dx, dy| {
                let (x, y) = origin.get();
                if let Some(point) = canvas.page_point_at(x + dx, y + dy) {
                    canvas.emit_select(SelectGesture::Extend(point));
                }
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            origin,
            move |_, dx, dy| {
                let moved = dx.hypot(dy) > 3.0;
                let single_click = canvas
                    .imp()
                    .last_press
                    .get()
                    .is_some_and(|(_, _, _, n)| n == 1);
                let (x, y) = origin.get();
                let click = !moved && single_click;
                if let Some((page, field)) = canvas.form_at(x, y).filter(|_| click) {
                    canvas.emit_select(SelectGesture::Form { page, field });
                } else if let Some(target) = canvas.link_at(x, y).filter(|_| click) {
                    canvas.emit_select(SelectGesture::OpenLink(target));
                } else {
                    canvas.emit_select(SelectGesture::End { moved });
                }
            }
        ));
        self.add_controller(drag);
    }

    /// 1 for a fresh click, 2 or 3 when it follows the previous one closely enough.
    fn count_click(&self, time: u32, x: f64, y: f64) -> u32 {
        let imp = self.imp();
        let interval = self.settings().gtk_double_click_time() as u32;
        let count = match imp.last_press.get() {
            Some((t, px, py, n))
                if time.wrapping_sub(t) < interval && (x - px).hypot(y - py) < 6.0 =>
            {
                n % 3 + 1
            }
            _ => 1,
        };
        imp.last_press.set(Some((time, x, y, count)));
        count
    }

    fn emit_select(&self, gesture: SelectGesture) {
        if let Some(callback) = self.imp().select_callback.borrow().as_ref() {
            callback(gesture);
        }
    }

    /// The page point under widget coordinates (`x`, `y`); the nearest page if between pages.
    fn page_point_at(&self, x: f64, y: f64) -> Option<PagePoint> {
        let zoom = self.effective_zoom();
        let (ox, oy) = self.origin();
        let pages = self.imp().pages.borrow();
        let (page, px, py) = pages
            .as_ref()?
            .layout
            .locate((x - ox) / zoom, (y - oy) / zoom)?;
        Some(PagePoint { page, x: px, y: py })
    }

    /// Links found on `page`, for hit testing.
    pub fn set_links(&self, page: usize, links: Vec<Link>) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            pages.links.insert(page, links);
        }
    }

    /// Show the form fields of every page that has some.
    pub fn set_forms(&self, forms: &std::collections::BTreeMap<usize, Vec<FormField>>) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            pages.forms = forms.iter().map(|(p, f)| (*p, f.clone())).collect();
        }
        self.queue_draw();
    }

    /// The form fields of every page that has some.
    pub fn forms(&self) -> std::collections::BTreeMap<usize, Vec<FormField>> {
        self.imp()
            .pages
            .borrow()
            .as_ref()
            .map(|p| p.forms.iter().map(|(k, v)| (*k, v.clone())).collect())
            .unwrap_or_default()
    }

    /// The point of `page` at the centre of the view, in document units.
    pub fn view_center(&self, page: usize) -> Option<annotate::Point> {
        self.doc_point(page, self.width() as f64 / 2.0, self.height() as f64 / 2.0)
    }

    /// The editable form field under widget coordinates (`x`, `y`), if any.
    fn form_at(&self, x: f64, y: f64) -> Option<(usize, FormField)> {
        let point = self.page_point_at(x, y)?;
        let pages = self.imp().pages.borrow();
        let fields = pages.as_ref()?.forms.get(&point.page)?;
        fields
            .iter()
            .find(|f| f.editable() && f.contains(point.x, point.y))
            .map(|f| (point.page, f.clone()))
    }

    /// A popover pointing at `area` (points) of `page`, removed again once closed.
    pub fn popup_at(&self, page: usize, area: &HitRect) -> Option<gtk::Popover> {
        let (ox, oy) = self.page_origin(page)?;
        let zoom = self.effective_zoom();
        let target = gdk::Rectangle::new(
            (ox + area.x1 * zoom) as i32,
            (oy + area.y1 * zoom) as i32,
            ((area.x2 - area.x1) * zoom).max(8.0) as i32,
            ((area.y2 - area.y1) * zoom).max(8.0) as i32,
        );
        let popover = gtk::Popover::new();
        popover.set_parent(self);
        popover.set_pointing_to(Some(&target));
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        Some(popover)
    }

    /// The link under widget coordinates (`x`, `y`), if any.
    fn link_at(&self, x: f64, y: f64) -> Option<LinkTarget> {
        let point = self.page_point_at(x, y)?;
        let pages = self.imp().pages.borrow();
        let links = pages.as_ref()?.links.get(&point.page)?;
        links
            .iter()
            .find(|l| l.contains(point.x, point.y))
            .map(|l| l.target.clone())
    }

    fn update_hover_cursor(&self, x: f64, y: f64) {
        if self.imp().cursor_hidden.get() {
            return;
        }
        if self.in_crop_mode() {
            if self
                .imp()
                .crop
                .borrow()
                .as_ref()
                .is_some_and(|c| c.drag.is_none())
            {
                let name = self.crop_grab_at(x, y).map_or("default", Grab::cursor);
                self.set_cursor_from_name(Some(name));
            }
            return;
        }
        if let MarkupMode::Draw(tool) = self.imp().markup_mode.get() {
            self.set_cursor_from_name(Some(markup_cursor(tool)));
            return;
        }
        if self.has_pages() && !self.imp().dragging.get() {
            let name = match self.form_at(x, y) {
                Some((_, field)) if matches!(field.kind, FieldKind::Text { .. }) => "text",
                Some(_) => "pointer",
                None if self.link_at(x, y).is_some() => "pointer",
                None => "text",
            };
            self.set_cursor_from_name(Some(name));
        }
    }

    // --- appearance ---------------------------------------------------------------

    pub fn set_background(&self, background: Background) {
        self.imp().background.set(background);
        self.queue_draw();
    }

    pub fn set_start_actual_size(&self, actual: bool) {
        self.imp().start_actual.set(actual);
    }

    pub fn set_smooth_zoom(&self, smooth: bool) {
        self.imp().smooth_zoom.set(smooth);
    }

    /// Fill the widget behind the content, for the backgrounds that are not the theme's.
    fn snapshot_background(&self, snapshot: &gtk::Snapshot) {
        let color = match self.imp().background.get() {
            Background::Dark => gdk::RGBA::new(0.12, 0.12, 0.12, 1.0),
            Background::Light => gdk::RGBA::new(0.96, 0.96, 0.95, 1.0),
            Background::System | Background::Checkerboard => return,
        };
        snapshot.append_color(
            &color,
            &graphene::Rect::new(0.0, 0.0, self.width() as f32, self.height() as f32),
        );
    }

    /// Grey squares behind a (possibly transparent) image, over `area` in widget coordinates.
    fn snapshot_checkerboard(&self, snapshot: &gtk::Snapshot, area: &graphene::Rect) {
        const TILE: f32 = 14.0;
        let light = gdk::RGBA::new(0.80, 0.80, 0.80, 1.0);
        let dark = gdk::RGBA::new(0.62, 0.62, 0.62, 1.0);
        // Only the part of the image that is on screen needs squares.
        let screen = graphene::Rect::new(0.0, 0.0, self.width() as f32, self.height() as f32);
        let Some(visible) = area.intersection(&screen) else {
            return;
        };
        snapshot.append_color(&light, &visible);

        let first_col = ((visible.x() - area.x()) / TILE).floor() as i32;
        let first_row = ((visible.y() - area.y()) / TILE).floor() as i32;
        let last_col = ((visible.x() + visible.width() - area.x()) / TILE).ceil() as i32;
        let last_row = ((visible.y() + visible.height() - area.y()) / TILE).ceil() as i32;
        for row in first_row..last_row {
            for col in first_col..last_col {
                if (row + col) % 2 != 0 {
                    continue;
                }
                let square = graphene::Rect::new(
                    area.x() + col as f32 * TILE,
                    area.y() + row as f32 * TILE,
                    TILE,
                    TILE,
                );
                if let Some(clipped) = square.intersection(&visible) {
                    snapshot.append_color(&dark, &clipped);
                }
            }
        }
    }

    // --- markup -------------------------------------------------------------------

    /// The annotations to draw, shared with whoever edits them.
    pub fn set_markup_store(&self, store: Rc<RefCell<Store>>) {
        *self.imp().markup_store.borrow_mut() = Some(store);
        self.queue_draw();
    }

    pub fn set_markup_mode(&self, mode: MarkupMode) {
        if self.imp().markup_mode.replace(mode) != mode {
            self.update_cursor();
        }
        self.queue_draw();
    }

    pub fn connect_markup(&self, callback: impl Fn(MarkupEvent) + 'static) {
        *self.imp().markup_callback.borrow_mut() = Some(Box::new(callback));
    }

    fn markup_draws(&self) -> bool {
        matches!(self.imp().markup_mode.get(), MarkupMode::Draw(_))
    }

    fn emit_markup(&self, event: MarkupEvent) {
        if let Some(callback) = self.imp().markup_callback.borrow().as_ref() {
            callback(event);
        }
    }

    /// Where the document origin of `page` is in widget coordinates (images: page 0).
    fn page_origin(&self, page: usize) -> Option<(f64, f64)> {
        let zoom = self.effective_zoom();
        let (ox, oy) = self.origin();
        match self.imp().pages.borrow().as_ref() {
            Some(pages) => {
                let r = pages.layout.rects.get(page)?;
                Some((ox + r.x * zoom, oy + r.y * zoom))
            }
            None => self.imp().logical.get().map(|_| (ox, oy)),
        }
    }

    /// The point of `page` under widget coordinates (`x`, `y`), possibly outside the page.
    fn doc_point(&self, page: usize, x: f64, y: f64) -> Option<annotate::Point> {
        let zoom = self.effective_zoom();
        let (px, py) = self.page_origin(page)?;
        Some(annotate::Point::new((x - px) / zoom, (y - py) / zoom))
    }

    /// The page and point under widget coordinates (`x`, `y`): the nearest page for PDFs.
    fn doc_point_at(&self, x: f64, y: f64) -> Option<(usize, annotate::Point)> {
        if self.has_pages() {
            let hit = self.page_point_at(x, y)?;
            Some((hit.page, annotate::Point::new(hit.x, hit.y)))
        } else {
            self.doc_point(0, x, y).map(|p| (0, p))
        }
    }

    fn add_markup_gesture(&self) {
        let drag = gtk::GestureDrag::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        // The page the drag began on and where, so later points are relative to that page.
        let start = Rc::new(Cell::new((0usize, 0.0f64, 0.0f64)));
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            start,
            move |gesture, x, y| {
                let hit = canvas
                    .markup_draws()
                    .then(|| canvas.doc_point_at(x, y))
                    .flatten();
                let Some((page, point)) = hit else {
                    // Denying ends the gesture at once; the `drag-end` that follows is not a release.
                    start.set((usize::MAX, 0.0, 0.0));
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                };
                canvas.grab_focus();
                start.set((page, x, y));
                let clicks = canvas.count_click(gesture.current_event_time(), x, y);
                let tolerance = MARKUP_REACH / canvas.effective_zoom();
                canvas.emit_markup(MarkupEvent::Press {
                    page,
                    point,
                    tolerance,
                    clicks,
                });
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            start,
            move |_, dx, dy| {
                let (page, x, y) = start.get();
                if let Some(point) = canvas.doc_point(page, x + dx, y + dy) {
                    canvas.emit_markup(MarkupEvent::Drag(point));
                }
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[strong]
            start,
            move |_, dx, dy| {
                let (page, x, y) = start.get();
                if page == usize::MAX {
                    return;
                }
                if let Some(point) = canvas.doc_point(page, x + dx, y + dy) {
                    canvas.emit_markup(MarkupEvent::Release(point));
                }
            }
        ));
        self.add_controller(drag);
    }

    /// Draw the annotations of `page` (document origin at `origin` on screen).
    fn snapshot_markup(
        &self,
        snapshot: &gtk::Snapshot,
        page: usize,
        (ox, oy): (f64, f64),
        zoom: f64,
    ) {
        let Some(store) = self.imp().markup_store.borrow().clone() else {
            return;
        };
        let store = store.borrow();
        let layer = store.layer(page).filter(|l| !l.is_empty());
        let draft = store
            .draft
            .as_ref()
            .filter(|(p, _)| *p == page)
            .map(|(_, a)| a);
        let selected = store
            .selection
            .filter(|s| s.page == page)
            .and(store.selected());
        if layer.is_none() && draft.is_none() && selected.is_none() {
            return;
        }
        let bounds = graphene::Rect::new(0.0, 0.0, self.width() as f32, self.height() as f32);
        let cr = snapshot.append_cairo(&bounds);
        cr.translate(ox, oy);
        cr.scale(zoom, zoom);
        if let Some(layer) = layer {
            annot_render::draw_layer(&cr, layer);
        }
        if let Some(draft) = draft {
            annot_render::draw_annotation(&cr, draft);
        }
        if let Some(selected) = selected {
            annot_render::draw_selection(&cr, selected, zoom);
        }
    }

    /// Edit the text of an annotation in a small popover over it. `on_done` gets the
    /// text when the popover closes.
    pub fn edit_text(
        &self,
        page: usize,
        bounds: annotate::Rect,
        initial: &str,
        on_done: impl FnOnce(String) + 'static,
    ) {
        let Some((ox, oy)) = self.page_origin(page) else {
            return;
        };
        let zoom = self.effective_zoom();
        let target = gdk::Rectangle::new(
            (ox + bounds.x * zoom) as i32,
            (oy + bounds.y * zoom) as i32,
            (bounds.w * zoom).max(8.0) as i32,
            (bounds.h * zoom).max(8.0) as i32,
        );

        let popover = gtk::Popover::new();
        popover.set_parent(self);
        popover.set_pointing_to(Some(&target));
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        view.buffer().set_text(initial);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .min_content_width(260)
            .min_content_height(90)
            .build();
        popover.set_child(Some(&scroller));

        // Enter finishes, as do Escape and a click elsewhere; Shift+Enter starts a new line.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak]
            popover,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| {
                let enter = key == gdk::Key::Return || key == gdk::Key::KP_Enter;
                if enter && !state.contains(gdk::ModifierType::SHIFT_MASK) {
                    popover.popdown();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        view.add_controller(keys);

        let on_done = Cell::new(Some(on_done));
        let buffer_view = view.clone();
        popover.connect_closed(move |popover| {
            let buffer = buffer_view.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            if let Some(done) = on_done.take() {
                done(text);
            }
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
        view.grab_focus();
    }

    // --- crop mode ---------------------------------------------------------------

    pub fn in_crop_mode(&self) -> bool {
        self.imp().crop.borrow().is_some()
    }

    /// Start cropping the image on screen, with a centred rectangle.
    pub fn begin_crop(&self, aspect: Option<f64>) {
        let Some(bounds) = self.texture_size().filter(|_| !self.has_pages()) else {
            return;
        };
        *self.imp().crop.borrow_mut() = Some(CropState {
            rect: crop::initial(bounds, aspect, 0.8),
            aspect,
            drag: None,
        });
        self.grab_focus();
        self.queue_draw();
        self.update_cursor();
    }

    /// Change the aspect ratio of the crop rectangle (`None` for free), resetting its size.
    pub fn set_crop_aspect(&self, aspect: Option<f64>) {
        let Some(bounds) = self.texture_size() else {
            return;
        };
        if let Some(state) = self.imp().crop.borrow_mut().as_mut() {
            state.aspect = aspect;
            state.rect = crop::initial(bounds, aspect, 0.8);
        }
        self.queue_draw();
    }

    pub fn end_crop(&self) {
        *self.imp().crop.borrow_mut() = None;
        self.queue_draw();
        self.update_cursor();
    }

    /// The chosen crop in whole pixels of the displayed image.
    pub fn crop_rect(&self) -> Option<CropRect> {
        let (bw, bh) = self.texture_size()?;
        let rect = self.imp().crop.borrow().as_ref()?.rect;
        let x = rect.x.floor().clamp(0.0, bw - 1.0);
        let y = rect.y.floor().clamp(0.0, bh - 1.0);
        let w = (rect.right().round() - x).clamp(1.0, bw - x);
        let h = (rect.bottom().round() - y).clamp(1.0, bh - y);
        Some(CropRect {
            x: x as u32,
            y: y as u32,
            w: w as u32,
            h: h as u32,
        })
    }

    /// What the point (`x`, `y`) in widget coordinates would grab of the crop rectangle.
    fn crop_grab_at(&self, x: f64, y: f64) -> Option<Grab> {
        let zoom = self.effective_zoom();
        let (ox, oy) = self.origin();
        let rect = self.imp().crop.borrow().as_ref()?.rect;
        crop::grab_at(rect, (x - ox) / zoom, (y - oy) / zoom, CROP_REACH / zoom)
    }

    fn add_crop_gesture(&self) {
        let drag = gtk::GestureDrag::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |gesture, x, y| {
                let grab = canvas.crop_grab_at(x, y);
                let grabbed = match (canvas.imp().crop.borrow_mut().as_mut(), grab) {
                    (Some(state), Some(grab)) => {
                        state.drag = Some((grab, state.rect));
                        true
                    }
                    _ => false,
                };
                // Denying ends the gesture there and then, and `drag-end` borrows the crop
                // state again: the borrow above must be over by now, or the app aborts.
                if !grabbed {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                }
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, dx, dy| {
                let Some(bounds) = canvas.texture_size() else {
                    return;
                };
                let zoom = canvas.effective_zoom();
                if let Some(state) = canvas.imp().crop.borrow_mut().as_mut()
                    && let Some((grab, start)) = state.drag
                {
                    state.rect =
                        crop::drag(start, grab, dx / zoom, dy / zoom, bounds, state.aspect);
                }
                canvas.queue_draw();
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _, _| {
                if let Some(state) = canvas.imp().crop.borrow_mut().as_mut() {
                    state.drag = None;
                }
            }
        ));
        self.add_controller(drag);
    }

    fn snapshot_crop(&self, snapshot: &gtk::Snapshot, zoom: f64, (ox, oy): (f64, f64)) {
        let crop = self.imp().crop.borrow();
        let (Some(state), Some((bw, bh))) = (crop.as_ref(), self.texture_size()) else {
            return;
        };
        let to_widget = |r: Rect| {
            graphene::Rect::new(
                (ox + r.x * zoom) as f32,
                (oy + r.y * zoom) as f32,
                (r.w * zoom) as f32,
                (r.h * zoom) as f32,
            )
        };
        let r = state.rect;

        // Dim everything outside the crop, inside the image.
        let dim = gdk::RGBA::new(0.0, 0.0, 0.0, 0.55);
        for outside in [
            Rect {
                x: 0.0,
                y: 0.0,
                w: bw,
                h: r.y,
            },
            Rect {
                x: 0.0,
                y: r.bottom(),
                w: bw,
                h: bh - r.bottom(),
            },
            Rect {
                x: 0.0,
                y: r.y,
                w: r.x,
                h: r.h,
            },
            Rect {
                x: r.right(),
                y: r.y,
                w: bw - r.right(),
                h: r.h,
            },
        ] {
            snapshot.append_color(&dim, &to_widget(outside));
        }

        // Frame and rule-of-thirds guides.
        let white = gdk::RGBA::new(1.0, 1.0, 1.0, 0.95);
        let guide = gdk::RGBA::new(1.0, 1.0, 1.0, 0.4);
        let area = to_widget(r);
        let (x, y, w, h) = (area.x(), area.y(), area.width(), area.height());
        for (rect, color) in [
            (graphene::Rect::new(x, y, w, 2.0), &white),
            (graphene::Rect::new(x, y + h - 2.0, w, 2.0), &white),
            (graphene::Rect::new(x, y, 2.0, h), &white),
            (graphene::Rect::new(x + w - 2.0, y, 2.0, h), &white),
            (graphene::Rect::new(x + w / 3.0, y, 1.0, h), &guide),
            (graphene::Rect::new(x + 2.0 * w / 3.0, y, 1.0, h), &guide),
            (graphene::Rect::new(x, y + h / 3.0, w, 1.0), &guide),
            (graphene::Rect::new(x, y + 2.0 * h / 3.0, w, 1.0), &guide),
        ] {
            snapshot.append_color(color, &rect);
        }

        // Handles at the corners and the middle of the sides.
        let outline = gdk::RGBA::new(0.0, 0.0, 0.0, 0.7);
        for (hx, hy) in [
            (x, y),
            (x + w / 2.0, y),
            (x + w, y),
            (x, y + h / 2.0),
            (x + w, y + h / 2.0),
            (x, y + h),
            (x + w / 2.0, y + h),
            (x + w, y + h),
        ] {
            let half = CROP_HANDLE / 2.0;
            snapshot.append_color(
                &outline,
                &graphene::Rect::new(
                    hx - half - 1.0,
                    hy - half - 1.0,
                    CROP_HANDLE + 2.0,
                    CROP_HANDLE + 2.0,
                ),
            );
            snapshot.append_color(
                &white,
                &graphene::Rect::new(hx - half, hy - half, CROP_HANDLE, CROP_HANDLE),
            );
        }
    }

    pub fn connect_select(&self, callback: impl Fn(SelectGesture) + 'static) {
        *self.imp().select_callback.borrow_mut() = Some(Box::new(callback));
    }

    /// Replace the selection rectangles drawn on the pages.
    pub fn set_selection(&self, rects: Vec<(usize, Vec<HitRect>)>) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            pages.selection = rects.into_iter().collect();
        }
        self.queue_draw();
    }

    /// Whether a PDF is being shown (as opposed to an image or nothing).
    pub fn has_pages(&self) -> bool {
        self.imp().pages.borrow().is_some()
    }

    pub fn clear_highlights(&self) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            pages.highlights.clear();
            pages.current_hit = None;
        }
        self.queue_draw();
    }

    pub fn add_highlights(&self, page: usize, hits: Vec<HitRect>) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            pages.highlights.insert(page, hits);
        }
        self.queue_draw();
    }

    pub fn set_current_hit(&self, hit: Option<(usize, usize)>) {
        if let Some(pages) = self.imp().pages.borrow_mut().as_mut() {
            pages.current_hit = hit;
        }
        self.queue_draw();
    }

    /// Scroll just enough to bring `hit` on `page` into view, centring it if it is not.
    pub fn reveal(&self, page: usize, hit: HitRect) {
        let rect = {
            let pages = self.imp().pages.borrow();
            pages
                .as_ref()
                .and_then(|p| p.layout.rects.get(page).copied())
        };
        let Some(rect) = rect else { return };
        let zoom = self.effective_zoom();
        let (ox, oy) = self.origin();
        // The hit in widget coordinates.
        let (left, right) = (ox + (rect.x + hit.x1) * zoom, ox + (rect.x + hit.x2) * zoom);
        let (top, bottom) = (oy + (rect.y + hit.y1) * zoom, oy + (rect.y + hit.y2) * zoom);
        let (width, height) = (self.width() as f64, self.height() as f64);

        let margin = 24.0;
        for (orientation, start, end, extent) in [
            (gtk::Orientation::Horizontal, left, right, width),
            (gtk::Orientation::Vertical, top, bottom, height),
        ] {
            if start < margin || end > extent - margin {
                let shift = (start + end) / 2.0 - extent / 2.0;
                self.set_adjustment_value(orientation, self.adjustment_value(orientation) + shift);
            }
        }
    }

    fn snapshot_pages(
        &self,
        snapshot: &gtk::Snapshot,
        pages: &PagesState,
        zoom: f64,
        (ox, oy): (f64, f64),
    ) {
        let (top, bottom) = (-oy / zoom, (self.height() as f64 - oy) / zoom);
        let Some((first, last)) = pages.layout.visible(top, bottom) else {
            return;
        };
        for index in first..=last {
            let r = pages.layout.rects[index];
            let bounds = graphene::Rect::new(
                (ox + r.x * zoom) as f32,
                (oy + r.y * zoom) as f32,
                (r.w * zoom) as f32,
                (r.h * zoom) as f32,
            );
            snapshot.append_outset_shadow(
                &gsk::RoundedRect::from_rect(bounds, 0.0),
                &PAGE_SHADOW,
                0.0,
                1.0,
                0.0,
                6.0,
            );
            match pages.textures.get(&index) {
                Some(texture) => {
                    snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Trilinear, &bounds)
                }
                None => snapshot.append_color(&gdk::RGBA::WHITE, &bounds),
            }
            // Fields that can be filled in get a light tint and edge, as in other readers.
            for field in pages.forms.get(&index).into_iter().flatten() {
                if !field.editable() {
                    continue;
                }
                let a = &field.area;
                let rect = graphene::Rect::new(
                    (ox + (r.x + a.x1) * zoom) as f32,
                    (oy + (r.y + a.y1) * zoom) as f32,
                    ((a.x2 - a.x1) * zoom) as f32,
                    ((a.y2 - a.y1) * zoom) as f32,
                );
                snapshot.append_color(&FORM_FILL, &rect);
                let edge = gsk::RoundedRect::from_rect(rect, 0.0);
                snapshot.append_border(&edge, &[1.0; 4], &[FORM_EDGE; 4]);
            }
            self.snapshot_markup(snapshot, index, (ox + r.x * zoom, oy + r.y * zoom), zoom);
            if let Some(rects) = pages.selection.get(&index) {
                for hit in rects {
                    let rect = graphene::Rect::new(
                        (ox + (r.x + hit.x1) * zoom) as f32,
                        (oy + (r.y + hit.y1) * zoom) as f32,
                        ((hit.x2 - hit.x1) * zoom) as f32,
                        ((hit.y2 - hit.y1) * zoom) as f32,
                    );
                    snapshot.append_color(&SELECTION_COLOR, &rect);
                }
            }
            if let Some(hits) = pages.highlights.get(&index) {
                for (i, hit) in hits.iter().enumerate() {
                    let current = pages.current_hit == Some((index, i));
                    let color = if current { HIT_CURRENT } else { HIT_OTHER };
                    let rect = graphene::Rect::new(
                        (ox + (r.x + hit.x1) * zoom) as f32,
                        (oy + (r.y + hit.y1) * zoom) as f32,
                        ((hit.x2 - hit.x1) * zoom) as f32,
                        ((hit.y2 - hit.y1) * zoom) as f32,
                    );
                    snapshot.append_color(&color, &rect);
                }
            }
        }
    }

    fn new_content(&self) {
        // Report the view again even if the zoom happens to be unchanged.
        let imp = self.imp();
        imp.last_view.set(None);
        *imp.crop.borrow_mut() = None;
        self.best_fit();
        if imp.start_actual.get() && imp.pages.borrow().is_none() {
            self.set_zoom(1.0, None);
        }
    }

    /// Called whenever the zoom, the visible pages or the scale factor change,
    /// and when new content appears.
    pub fn connect_view_changed(&self, callback: impl Fn(ViewInfo) + 'static) {
        *self.imp().view_callback.borrow_mut() = Some(Box::new(callback));
    }

    /// The zoom actually used for drawing (the fit zoom while in fit mode).
    pub fn effective_zoom(&self) -> f64 {
        self.effective_zoom_for(self.width() as f64, self.height() as f64)
    }

    fn effective_zoom_for(&self, width: f64, height: f64) -> f64 {
        let imp = self.imp();
        match (imp.fit.get(), imp.logical.get()) {
            (true, Some((lw, _))) if imp.pages.borrow().is_some() => {
                // Documents fit to width; the height scrolls.
                (width / lw).min(MAX_PAGES_FIT_ZOOM)
            }
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
        let imp = self.imp();
        if !imp.smooth_zoom.get() || self.texture_size().is_none() {
            self.set_zoom(self.effective_zoom() * factor, anchor);
            return;
        }
        // Quick repeats (a spinning wheel) build on where the last animation was going.
        let running = imp
            .zoom_animation
            .borrow()
            .as_ref()
            .is_some_and(|a| a.state() == adw::AnimationState::Playing);
        let base = if running {
            imp.zoom_target.get()
        } else {
            self.effective_zoom()
        };
        let target = (base * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        imp.zoom_target.set(target);
        // The old animation must stop, or the two would fight over the zoom.
        if let Some(old) = imp.zoom_animation.borrow_mut().take() {
            old.pause();
        }

        let canvas = self.downgrade();
        let animation = adw::TimedAnimation::new(
            self,
            self.effective_zoom(),
            target,
            ZOOM_ANIMATION_MS,
            adw::CallbackAnimationTarget::new(move |zoom| {
                if let Some(canvas) = canvas.upgrade() {
                    canvas.set_zoom(zoom, anchor);
                }
            }),
        );
        animation.set_easing(adw::Easing::EaseOutCubic);
        animation.play();
        *imp.zoom_animation.borrow_mut() = Some(animation);
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

    /// Size of the content as displayed (after orientation), at 100 % zoom.
    fn texture_size(&self) -> Option<(f64, f64)> {
        let imp = self.imp();
        imp.logical.get().map(|(w, h)| {
            if imp.orientation.get().swaps_axes() {
                (h, w)
            } else {
                (w, h)
            }
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

    pub fn set_cursor_hidden(&self, hidden: bool) {
        if self.imp().cursor_hidden.replace(hidden) != hidden {
            self.update_cursor();
        }
    }

    fn update_cursor(&self) {
        if self.imp().cursor_hidden.get() {
            self.set_cursor_from_name(Some("none"));
            return;
        }
        let name = if let MarkupMode::Draw(tool) = self.imp().markup_mode.get() {
            Some(markup_cursor(tool))
        } else if self.has_pages() {
            Some("text")
        } else {
            self.can_pan().then_some("grab")
        };
        self.set_cursor_from_name(name);
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
        self.notify_view();
    }

    /// Report the current view to the owner if it changed since last time.
    fn notify_view(&self) {
        let imp = self.imp();
        let (width, height) = (self.width() as f64, self.height() as f64);
        if imp.logical.get().is_none() || width < 1.0 || height < 1.0 {
            return;
        }
        let zoom = self.effective_zoom();
        let (first_visible, last_visible, current_page) = match imp.pages.borrow().as_ref() {
            Some(pages) => {
                let top = self.adjustment_value(gtk::Orientation::Vertical) / zoom;
                let bottom = top + height / zoom;
                let (first, last) = pages.layout.visible(top, bottom).unwrap_or((0, 0));
                let current = match self.adjustment(gtk::Orientation::Vertical) {
                    // Exactly at an end the first/last page is current, however short.
                    Some(adj) if adj.value() <= 0.5 => first,
                    Some(adj) if adj.value() >= adj.upper() - adj.page_size() - 0.5 => {
                        pages.layout.rects.len().saturating_sub(1)
                    }
                    _ => pages.layout.current(top, bottom).unwrap_or(0),
                };
                (first, last, current)
            }
            None => (0, 0, 0),
        };
        let pan = if imp.pages.borrow().is_some() {
            (0, 0)
        } else {
            let (x, y) = self.origin();
            ((x / PAN_STEP).floor() as i64, (y / PAN_STEP).floor() as i64)
        };
        let info = ViewInfo {
            zoom,
            scale_factor: self.scale_factor(),
            first_visible,
            last_visible,
            current_page,
            pan,
        };
        if imp.last_view.get().is_some_and(|last| last.same_as(&info)) {
            return;
        }
        imp.last_view.set(Some(info));
        if let Some(callback) = imp.view_callback.borrow().as_ref() {
            callback(info);
        }
    }
}

fn markup_cursor(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "default",
        Tool::Text => "text",
        _ => "crosshair",
    }
}
