//! Printing images and PDFs through the native GTK print dialog.
//!
//! Both kinds of document are drawn onto one cairo page per sheet, scaled and
//! centred inside the printable area. Orientation follows the content, so a
//! landscape photo or page uses a landscape sheet.

use std::{cell::{Cell, RefCell}, path::Path, rc::Rc};

use anyhow::{Context, Result};
use relm4::gtk::{self, cairo, prelude::*};

use crate::{
    annotate::{AnnotationLayer, render as annot_render},
    image::ImageDocument,
    pdf::{PdfSource, PrintDocument},
};

/// 1 pixel on screen is 1/96 inch, i.e. 0.75 points.
const PIXEL_TO_POINT: f64 = 72.0 / 96.0;

/// What to print.
pub enum Job {
    Pdf {
        source: PdfSource,
        /// Page sizes in points, to choose the sheet orientation.
        page_sizes: Vec<(f64, f64)>,
        /// Annotations per page, drawn over the pages.
        layers: std::collections::BTreeMap<usize, AnnotationLayer>,
    },
    Image {
        image: ImageDocument,
        /// Annotations in the displayed image's pixels.
        layer: AnnotationLayer,
    },
}

/// How the content is sized on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scaling {
    /// Scaled up or down to fill the printable area, keeping the aspect ratio.
    FitToPage,
    /// The size it has on screen at 100 % (images) or its own size (PDF pages).
    ActualSize,
}

/// Settings kept for the rest of the session, so the next print starts where the last ended.
struct Session {
    settings: gtk::PrintSettings,
    page_setup: Option<gtk::PageSetup>,
    scaling: Scaling,
}

thread_local! {
    static SESSION: RefCell<Session> = RefCell::new(Session {
        settings: gtk::PrintSettings::new(),
        page_setup: None,
        scaling: Scaling::FitToPage,
    });
}

/// Choose between "Fit to page" (false) and "Actual size" (true) for the next print.
/// The print dialog's own "Scaling" tab does the same where GTK can show it, but
/// the desktop portal's dialog cannot, so the window offers it in a menu too.
pub fn set_actual_size(actual: bool) {
    SESSION.with(|s| s.borrow_mut().scaling = if actual { Scaling::ActualSize } else { Scaling::FitToPage });
}

/// Size of the content in points when printed at `scaling`, for a sheet whose
/// printable area is `area`. `content` is in the content's own units (`unit`
/// points per unit when printed at actual size).
fn scaled_size(content: (f64, f64), unit: f64, area: (f64, f64), scaling: Scaling) -> f64 {
    match scaling {
        Scaling::FitToPage => (area.0 / content.0).min(area.1 / content.1),
        Scaling::ActualSize => unit,
    }
}

/// Open the print dialog for `job`, or, with `export_to`, write a PDF without asking.
pub fn print(job: Job, name: &str, parent: Option<&gtk::Window>, export_to: Option<&Path>) -> Result<()> {
    let (n_pages, orientations) = match &job {
        Job::Pdf { page_sizes, .. } => (
            page_sizes.len(),
            page_sizes.iter().map(|&(w, h)| w > h).collect::<Vec<_>>(),
        ),
        Job::Image { image, .. } => {
            let (w, h) = image.display_size();
            (1, vec![w > h])
        }
    };
    let content = Rc::new(Content::open(job)?);

    let op = gtk::PrintOperation::new();
    op.set_job_name(name);
    op.set_n_pages(n_pages as i32);
    op.set_unit(gtk::Unit::Points);
    op.set_allow_async(true);
    op.set_embed_page_setup(true);
    op.set_custom_tab_label(Some("Scaling"));
    SESSION.with(|s| {
        let s = s.borrow();
        op.set_print_settings(Some(&s.settings));
        if let Some(setup) = &s.page_setup {
            op.set_default_page_setup(Some(setup));
        }
    });

    // Landscape content gets a landscape sheet.
    op.connect_request_page_setup(move |_, _, page, setup| {
        let landscape = orientations.get(page as usize).copied().unwrap_or(false);
        setup.set_orientation(if landscape {
            gtk::PageOrientation::Landscape
        } else {
            gtk::PageOrientation::Portrait
        });
        // Next time, start from the paper and margins the user picked.
        SESSION.with(|s| s.borrow_mut().page_setup = Some(setup.copy()));
    });

    // The "Scaling" tab of the print dialog.
    let choice: Rc<RefCell<Option<gtk::DropDown>>> = Rc::default();
    op.connect_create_custom_widget({
        let choice = Rc::clone(&choice);
        move |_| {
            let dropdown = gtk::DropDown::from_strings(&["Fit to page", "Actual size"]);
            dropdown.set_selected(SESSION.with(|s| s.borrow().scaling == Scaling::ActualSize) as u32);
            let row = gtk::Box::builder().spacing(12).margin_top(12).margin_start(12).margin_end(12).build();
            row.append(&gtk::Label::new(Some("Size on paper")));
            row.append(&dropdown);
            *choice.borrow_mut() = Some(dropdown);
            let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
            container.append(&row);
            Some(container.upcast())
        }
    });
    op.connect_custom_widget_apply({
        let choice = Rc::clone(&choice);
        move |_, _| {
            if let Some(dropdown) = choice.borrow().as_ref() {
                let scaling = if dropdown.selected() == 1 { Scaling::ActualSize } else { Scaling::FitToPage };
                SESSION.with(|s| s.borrow_mut().scaling = scaling);
            }
        }
    });

    let failed: Rc<Cell<bool>> = Rc::default();
    op.connect_draw_page({
        let failed = Rc::clone(&failed);
        move |op, context, page| {
            let scaling = SESSION.with(|s| s.borrow().scaling);
            if let Err(err) = content.draw(page as usize, context, scaling) {
                tracing::warn!("printing page {} failed: {err:#}", page + 1);
                failed.set(true);
                op.cancel();
            }
        }
    });
    op.connect_done(move |op, result| {
        if result == gtk::PrintOperationResult::Apply {
            SESSION.with(|s| {
                let mut s = s.borrow_mut();
                if let Some(settings) = op.print_settings() {
                    s.settings = settings;
                }
            });
        }
    });

    let action = match export_to {
        Some(path) => {
            op.set_export_filename(path);
            gtk::PrintOperationAction::Export
        }
        None => gtk::PrintOperationAction::PrintDialog,
    };
    op.run(action, parent).context("printing failed")?;
    anyhow::ensure!(!failed.get(), "a page could not be drawn");
    Ok(())
}

/// The document being drawn, prepared once for the whole job.
enum Content {
    Pdf {
        document: PrintDocument,
        sizes: Vec<(f64, f64)>,
        layers: std::collections::BTreeMap<usize, AnnotationLayer>,
    },
    Image {
        surface: cairo::ImageSurface,
        orientation: cairo::Matrix,
        size: (f64, f64),
        layer: AnnotationLayer,
    },
}

impl Content {
    fn open(job: Job) -> Result<Self> {
        match job {
            Job::Pdf { source, page_sizes, layers } => Ok(Self::Pdf {
                document: PrintDocument::open(&source)?,
                sizes: page_sizes,
                layers,
            }),
            Job::Image { image, layer } => {
                let (tw, th) = (image.texture.width(), image.texture.height());
                let stride = tw as usize * 4;
                let mut data = vec![0u8; stride * th as usize];
                // GDK's premultiplied BGRA is cairo's native ARGB32 on little-endian machines.
                image.texture.download(&mut data, stride);
                let surface = cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, tw, th, stride as i32)
                    .context("cannot prepare the image for printing")?;

                // Rotation and mirroring from the EXIF orientation, about the image centre.
                let (xx, yx, xy, yy, x0, y0) = image.orientation.transform().to_2d();
                let orientation = cairo::Matrix::new(xx as f64, yx as f64, xy as f64, yy as f64, x0 as f64, y0 as f64);
                let (w, h) = image.display_size();
                Ok(Self::Image { surface, orientation, size: (w as f64, h as f64), layer })
            }
        }
    }

    fn draw(&self, page: usize, context: &gtk::PrintContext, scaling: Scaling) -> Result<()> {
        let cr = context.cairo_context();
        let area = (context.width(), context.height());
        match self {
            Self::Pdf { document, sizes, layers } => {
                let size = sizes.get(page).copied().context("no such page")?;
                let scale = scaled_size(size, 1.0, area, scaling);
                cr.save()?;
                cr.translate((area.0 - size.0 * scale) / 2.0, (area.1 - size.1 * scale) / 2.0);
                cr.scale(scale, scale);
                document.render_for_printing(page, &cr)?;
                if let Some(layer) = layers.get(&page) {
                    annot_render::draw_layer(&cr, layer);
                }
                cr.restore()?;
            }
            Self::Image { surface, orientation, size, layer } => {
                let scale = scaled_size(*size, PIXEL_TO_POINT, area, scaling);
                cr.save()?;
                cr.translate((area.0 - size.0 * scale) / 2.0, (area.1 - size.1 * scale) / 2.0);
                cr.scale(scale, scale);
                // The oriented image is centred on the origin in `orientation`'s frame.
                cr.save()?;
                cr.translate(size.0 / 2.0, size.1 / 2.0);
                cr.transform(*orientation);
                let (tw, th) = (surface.width() as f64, surface.height() as f64);
                cr.set_source_surface(surface, -tw / 2.0, -th / 2.0)?;
                cr.source().set_filter(cairo::Filter::Good);
                cr.paint()?;
                cr.restore()?;
                // Annotations are in the displayed image's pixels, with its top-left as origin.
                annot_render::draw_layer(&cr, layer);
                cr.restore()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_to_page_fills_the_limiting_side() {
        // A 4:3 landscape photo on a portrait-less A4 landscape area (842×595 pt, minus margins).
        let scale = scaled_size((4000.0, 3000.0), PIXEL_TO_POINT, (800.0, 560.0), Scaling::FitToPage);
        assert!((4000.0 * scale - 746.67).abs() < 0.1, "height is the limit: {scale}");
        // Fitting also enlarges small content.
        assert!(scaled_size((100.0, 100.0), 1.0, (500.0, 500.0), Scaling::FitToPage) > 1.0);
    }

    #[test]
    fn actual_size_ignores_the_sheet() {
        assert_eq!(scaled_size((4000.0, 3000.0), PIXEL_TO_POINT, (800.0, 560.0), Scaling::ActualSize), 0.75);
        assert_eq!(scaled_size((595.0, 842.0), 1.0, (100.0, 100.0), Scaling::ActualSize), 1.0);
    }
}
