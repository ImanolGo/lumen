//! Organising a PDF's pages: a grid of thumbnails to reorder by dragging, turn,
//! delete, and fill with pages from other PDFs. Nothing is written until saved;
//! saving rewrites the file with the new page order (keeping a password-protected
//! one protected), together with any unsaved markup and form values.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use iced::widget::{Id, button, center, column, container, image, mouse_area, row, scrollable, space, text};
use iced::{Alignment, Element, Font, Length, Radians, Rotation, Task};

use super::{App, Doc, Message, blocking, err_string, file_name, icon_button, muted, separator, tip};
use crate::i18n::tr;
use crate::pages::{self, Entry};
use crate::raster::Picture;
use crate::{icons, pdf, style};

const CELL_W: f32 = 172.0;
const CELL_H: f32 = 232.0;
/// Empty space above the first row, so the floating toolbar does not cover it.
const TOP_INSET: f32 = 56.0;
/// Longest side of a thumbnail, in logical pixels.
const THUMB: f32 = 150.0;
const GAP: f32 = 6.0;

fn grid_id() -> Id {
    Id::new("pages-grid")
}

pub struct Organizer {
    plan: Vec<Entry>,
    past: Vec<Vec<Entry>>,
    future: Vec<Vec<Entry>>,
    /// Selected positions in the plan.
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    /// Source 0 is the open document; the rest are the PDFs pages were inserted from.
    pub(super) sources: Vec<pdf::Document>,
    thumbs: HashMap<(usize, usize), image::Handle>,
    requested: HashSet<(usize, usize)>,
    scroll: (f32, f32),
    /// The press that may become a drag, and where the pages would go.
    pressed: Option<usize>,
    dragging: bool,
    drop_at: Option<usize>,
    saving: bool,
}

impl Organizer {
    pub fn is_changed(&self) -> bool {
        self.plan != pages::identity(self.sources[0].page_count())
    }

    fn change(&mut self, f: impl FnOnce(&mut Vec<Entry>) -> bool) {
        let before = self.plan.clone();
        if f(&mut self.plan) && self.plan != before {
            self.past.push(before);
            self.future.clear();
        }
    }

    /// The plan's entries at the selected positions, in page order.
    pub fn selected_entries(&self) -> Vec<Entry> {
        self.selected.iter().filter_map(|&p| self.plan.get(p).cloned()).collect()
    }

    fn positions(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }
}

#[derive(Debug, Clone)]
pub enum PagesMsg {
    Open,
    Close,
    Scrolled(f32, f32),
    Thumb(u64, usize, usize, Result<Picture, String>),
    Press(usize),
    Enter(usize),
    Rotate(i32),
    Delete,
    MoveBy(i32),
    SelectAll,
    Undo,
    Redo,
    Insert,
    InsertChosen(Result<Vec<PathBuf>, String>),
    Inserted(PathBuf, Result<pdf::Opened, pdf::OpenError>),
    Save,
    SaveAs,
    SaveTo(Result<Option<PathBuf>, String>),
    Saved(u64, PathBuf, Result<pdf::Opened, String>),
}

impl App {
    pub(super) fn pages_update(&mut self, msg: PagesMsg) -> Task<Message> {
        match msg {
            PagesMsg::Open => {
                if self.organizer.is_some() {
                    return self.pages_update(PagesMsg::Close);
                }
                let done = self.finish_text();
                let Doc::Pdf(view) = &self.doc else { return done };
                let n = view.doc.page_count();
                self.organizer = Some(Organizer {
                    plan: pages::identity(n),
                    past: Vec::new(),
                    future: Vec::new(),
                    selected: BTreeSet::from([view.current_page]),
                    anchor: Some(view.current_page),
                    sources: vec![view.doc.clone()],
                    thumbs: HashMap::new(),
                    requested: HashSet::new(),
                    scroll: (0.0, 0.0),
                    pressed: None,
                    dragging: false,
                    drop_at: None,
                    saving: false,
                });
                let row = view.current_page / self.grid_columns();
                let y = (TOP_INSET + row as f32 * CELL_H - 80.0).max(0.0);
                Task::batch([
                    done,
                    iced::widget::operation::scroll_to(grid_id(), scrollable::AbsoluteOffset { x: 0.0, y }),
                    self.request_thumbs_grid(),
                ])
            }
            PagesMsg::Close => {
                self.organizer = None;
                Task::none()
            }
            PagesMsg::Scrolled(offset, height) => {
                if let Some(o) = &mut self.organizer {
                    o.scroll = (offset, height);
                }
                self.request_thumbs_grid()
            }
            PagesMsg::Thumb(generation, source, index, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                if let (Some(o), Ok(p)) = (&mut self.organizer, result) {
                    o.thumbs.insert((source, index), image::Handle::from_rgba(p.width, p.height, p.rgba));
                }
                Task::none()
            }
            PagesMsg::Press(pos) => {
                let shift = self.modifiers.shift();
                let ctrl = self.modifiers.command();
                let Some(o) = &mut self.organizer else { return Task::none() };
                if ctrl {
                    if !o.selected.remove(&pos) {
                        o.selected.insert(pos);
                    }
                    o.anchor = Some(pos);
                } else if shift && let Some(a) = o.anchor {
                    o.selected = (a.min(pos)..=a.max(pos)).collect();
                } else {
                    if !o.selected.contains(&pos) {
                        o.selected = BTreeSet::from([pos]);
                    }
                    o.anchor = Some(pos);
                    o.pressed = Some(pos);
                }
                Task::none()
            }
            PagesMsg::Enter(pos) => {
                if let Some(o) = &mut self.organizer
                    && let Some(pressed) = o.pressed
                {
                    o.dragging |= pos != pressed;
                    if o.dragging {
                        o.drop_at = Some(pos);
                    }
                }
                Task::none()
            }
            PagesMsg::Rotate(degrees) => {
                if let Some(o) = &mut self.organizer {
                    let positions = o.positions();
                    o.change(|plan| {
                        pages::rotate(plan, &positions, degrees);
                        true
                    });
                }
                Task::none()
            }
            PagesMsg::Delete => {
                let Some(o) = &mut self.organizer else { return Task::none() };
                let positions = o.positions();
                if positions.len() >= o.plan.len() {
                    return self.toast(tr("A document needs at least one page"));
                }
                o.change(|plan| pages::delete(plan, &positions));
                let first = positions.first().copied().unwrap_or(0).min(o.plan.len() - 1);
                o.selected = BTreeSet::from([first]);
                o.anchor = Some(first);
                self.request_thumbs_grid()
            }
            PagesMsg::MoveBy(delta) => {
                let Some(o) = &mut self.organizer else { return Task::none() };
                let positions = o.positions();
                let (Some(&first), Some(&last)) = (positions.first(), positions.last()) else { return Task::none() };
                let before = if delta < 0 {
                    first.saturating_sub(1)
                } else if last + 2 <= o.plan.len() {
                    last + 2
                } else {
                    return Task::none();
                };
                if delta < 0 && first == 0 {
                    return Task::none();
                }
                let mut at = 0;
                o.change(|plan| {
                    at = pages::move_to(plan, &positions, before);
                    true
                });
                o.selected = (at..at + positions.len()).collect();
                o.anchor = Some(at);
                Task::none()
            }
            PagesMsg::SelectAll => {
                if let Some(o) = &mut self.organizer {
                    o.selected = (0..o.plan.len()).collect();
                }
                Task::none()
            }
            PagesMsg::Undo | PagesMsg::Redo => {
                let Some(o) = &mut self.organizer else { return Task::none() };
                let (from, to) = if matches!(msg, PagesMsg::Undo) {
                    (&mut o.past, &mut o.future)
                } else {
                    (&mut o.future, &mut o.past)
                };
                if let Some(plan) = from.pop() {
                    to.push(std::mem::replace(&mut o.plan, plan));
                    o.selected.retain(|&p| p < o.plan.len());
                }
                self.request_thumbs_grid()
            }
            PagesMsg::Insert => App::ask_for_files(super::chooser::Purpose::InsertPdf, None),
            PagesMsg::InsertChosen(Err(e)) => self.toast(e),
            PagesMsg::InsertChosen(Ok(paths)) => {
                let tasks = paths
                    .into_iter()
                    .filter(|p| crate::library::Kind::of(p) == Some(crate::library::Kind::Pdf))
                    .map(|path| {
                        let password = self.passwords.get(&path).cloned();
                        Task::perform(
                            {
                                let path = path.clone();
                                blocking(move || pdf::Document::open_full(&path, false, password.as_deref()))
                            },
                            move |r| {
                                let r = r.map_err(pdf::OpenError::Failed).and_then(|r| r.map_err(pdf::OpenError::from));
                                Message::Pages(PagesMsg::Inserted(path.clone(), r))
                            },
                        )
                    });
                Task::batch(tasks)
            }
            PagesMsg::Inserted(path, result) => {
                let opened = match result {
                    Ok(opened) => opened,
                    Err(pdf::OpenError::Password { .. }) => {
                        return self
                            .toast(format!("“{}” is password-protected: open it in Lumen first", file_name(&path)));
                    }
                    Err(pdf::OpenError::Failed(e)) => {
                        return self.toast(trf!("Could not insert “{}”: {}", file_name(&path), e));
                    }
                };
                let Some(o) = &mut self.organizer else { return Task::none() };
                let source = o.sources.len();
                let count = opened.doc.page_count();
                o.sources.push(opened.doc);
                let at = o.selected.last().map_or(o.plan.len(), |&p| p + 1);
                o.change(|plan| {
                    let new = (0..count).map(|index| Entry { source, index, rotate: 0 });
                    plan.splice(at..at, new);
                    true
                });
                o.selected = (at..at + count).collect();
                o.anchor = Some(at);
                self.request_thumbs_grid()
            }
            PagesMsg::Save => {
                let Doc::Pdf(view) = &self.doc else { return Task::none() };
                let path = view.doc.path().to_owned();
                self.save_pages(path)
            }
            PagesMsg::SaveAs => {
                let Doc::Pdf(view) = &self.doc else { return Task::none() };
                let p = view.doc.path();
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let suggested = p.with_file_name(format!("{stem} (rearranged).pdf"));
                App::ask_for_files(super::chooser::Purpose::SavePages, Some(suggested))
            }
            PagesMsg::SaveTo(Ok(Some(target))) => self.save_pages(target),
            PagesMsg::SaveTo(Ok(None)) => Task::none(),
            PagesMsg::SaveTo(Err(e)) => self.toast(e),
            PagesMsg::Saved(generation, target, result) => {
                if let Some(o) = &mut self.organizer {
                    o.saving = false;
                }
                let opened = match result {
                    Ok(opened) => opened,
                    Err(e) => return self.toast(trf!("Could not save: {}", e)),
                };
                if generation != self.generation {
                    return self.notice(trf!("Saved “{}”", file_name(&target)));
                }
                // Markup and form values are in the file now; start from it afresh.
                self.organizer = None;
                self.marks = None;
                self.form = None;
                self.form_path = None;
                self.stash.remove(&target);
                self.stash_forms.remove(&target);
                if self.files.get(self.current).is_some_and(|p| *p != target) {
                    self.files[self.current] = target.clone();
                }
                self.thumbs.remove(&target);
                self.generation += 1;
                self.doc = Doc::Loading(target.clone());
                Task::batch([
                    self.install_opened(target.clone(), opened),
                    self.notice(trf!("Saved “{}”", file_name(&target))),
                ])
            }
        }
    }

    /// A press anywhere ended: drop dragged pages where they were held.
    pub(super) fn pages_mouse_up(&mut self) -> bool {
        let Some(o) = &mut self.organizer else { return false };
        let Some(pressed) = o.pressed.take() else { return false };
        let (dragging, drop_at) = (std::mem::take(&mut o.dragging), o.drop_at.take());
        match (dragging, drop_at) {
            (true, Some(before)) => {
                let positions = o.positions();
                if positions.contains(&before)
                    && !positions.contains(&before.saturating_sub(1))
                    && before != o.plan.len()
                {
                    // Dropped onto itself.
                } else {
                    let mut at = 0;
                    o.change(|plan| {
                        at = pages::move_to(plan, &positions, before);
                        true
                    });
                    o.selected = (at..at + positions.len()).collect();
                    o.anchor = Some(at);
                }
            }
            // A plain click on a page that was part of a selection selects just it.
            _ => {
                o.selected = BTreeSet::from([pressed]);
                o.anchor = Some(pressed);
            }
        }
        true
    }

    fn save_pages(&mut self, target: PathBuf) -> Task<Message> {
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        let Some(o) = &mut self.organizer else { return Task::none() };
        if o.saving {
            return Task::none();
        }
        o.saving = true;
        let source = view.doc.path().to_owned();
        let password = view.doc.password().map(str::to_owned);
        let extras: Vec<(PathBuf, Option<String>)> =
            o.sources[1..].iter().map(|d| (d.path().to_owned(), d.password().map(str::to_owned))).collect();
        let plan = o.plan.clone();
        let unsaved = self.has_unsaved_changes();
        let pending = self.pdf_changes();
        let markup = self.marks.is_some();
        let generation = self.generation;
        Task::perform(
            {
                let target = target.clone();
                blocking(move || -> anyhow::Result<pdf::Opened> {
                    let bytes = if unsaved {
                        pdf::Document::with_changes(&source, &pending.changes())?
                    } else {
                        std::fs::read(&source)?
                    };
                    let extras = extras
                        .into_iter()
                        .map(|(path, password)| Ok(pages::Source { bytes: std::fs::read(path)?, password }))
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    let out = pages::compose(&pages::Source { bytes, password: password.clone() }, &extras, &plan)?;
                    pdf::write_atomic(&target, &out)?;
                    pdf::Document::open_full(&target, markup, password.as_deref())
                })
            },
            move |r| {
                let r = r.and_then(|r| r.map_err(err_string));
                Message::Pages(PagesMsg::Saved(generation, target.clone(), r))
            },
        )
    }

    fn grid_columns(&self) -> usize {
        ((self.area.width - 2.0 * GAP) / CELL_W).floor().max(1.0) as usize
    }

    fn grid_rows(&self) -> std::ops::Range<usize> {
        let Some(o) = &self.organizer else { return 0..0 };
        let cols = self.grid_columns();
        let (offset, height) = o.scroll;
        let height = if height > 0.0 { height } else { self.area.height };
        let first = ((offset - TOP_INSET) / CELL_H).floor().max(0.0) as usize;
        let last = ((offset + height - TOP_INSET) / CELL_H).ceil().max(0.0) as usize + 1;
        let rows = o.plan.len().div_ceil(cols) + 1;
        first.min(rows)..last.min(rows)
    }

    /// Asks for the thumbnails of the rows on screen.
    fn request_thumbs_grid(&mut self) -> Task<Message> {
        let cols = self.grid_columns();
        let rows = self.grid_rows();
        let generation = self.generation;
        let scale = self.scale_factor;
        let Some(o) = &mut self.organizer else { return Task::none() };
        let mut tasks = Vec::new();
        let start = rows.start.saturating_sub(1) * cols;
        let end = ((rows.end + 1) * cols).min(o.plan.len());
        for entry in o.plan.get(start..end).unwrap_or(&[]) {
            let key = (entry.source, entry.index);
            if o.thumbs.contains_key(&key) || !o.requested.insert(key) {
                continue;
            }
            let doc = o.sources[entry.source].clone();
            let (w, h) = doc.page_size(entry.index);
            let px = THUMB * scale / w.max(h).max(1.0);
            let (source, index) = key;
            tasks.push(Task::perform(async move { doc.render(index, px).await.map_err(err_string) }, move |r| {
                Message::Pages(PagesMsg::Thumb(generation, source, index, r))
            }));
        }
        Task::batch(tasks)
    }

    /// Keys while organising pages; `None` for the rest.
    pub(super) fn pages_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        use iced::keyboard::{Key, key::Named};
        self.organizer.as_ref()?;
        let ctrl = modifiers.command();
        let msg = match key.as_ref() {
            Key::Character(c) => match (c.to_lowercase().as_str(), ctrl, modifiers.shift()) {
                ("a", true, _) => PagesMsg::SelectAll,
                ("z", true, false) => PagesMsg::Undo,
                ("z", true, true) | ("y", true, false) => PagesMsg::Redo,
                ("s", true, false) => PagesMsg::Save,
                ("s", true, true) => PagesMsg::SaveAs,
                ("r", false, false) => PagesMsg::Rotate(90),
                ("r", false, true) => PagesMsg::Rotate(-90),
                ("g", false, _) => PagesMsg::Close,
                _ => return Some(Task::none()),
            },
            Key::Named(Named::Delete | Named::Backspace) => PagesMsg::Delete,
            Key::Named(Named::ArrowLeft | Named::ArrowUp) if ctrl => PagesMsg::MoveBy(-1),
            Key::Named(Named::ArrowRight | Named::ArrowDown) if ctrl => PagesMsg::MoveBy(1),
            Key::Named(Named::Escape) => return Some(Task::none()),
            _ => return Some(Task::none()),
        };
        Some(self.pages_update(msg))
    }

    /// Escape closes the organiser when nothing would be lost.
    pub(super) fn pages_escape(&mut self) -> bool {
        match &self.organizer {
            Some(o) if !o.is_changed() => {
                self.organizer = None;
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    // ------------------------------------------------------------ view

    pub(super) fn pages_bar(&self) -> Element<'_, Message> {
        let Some(o) = &self.organizer else { return space().into() };
        let p = |m: PagesMsg| Some(Message::Pages(m));
        let any = !o.selected.is_empty();
        let busy = o.saving;
        let count = o.plan.len();
        let status = if busy {
            tr("Saving…").to_string()
        } else {
            format!(
                "{count} page{}{}",
                if count == 1 { "" } else { "s" },
                if o.is_changed() { " · unsaved" } else { "" }
            )
        };
        let save = button(text(tr("Save")).size(13))
            .padding([6, 14])
            .style(style::primary)
            .on_press_maybe((o.is_changed() && !busy).then_some(Message::Pages(PagesMsg::Save)));
        let content = row![
            icon_button(
                icons::rotate_left(),
                tr("Turn left (Shift+R)"),
                any.then_some(Message::Pages(PagesMsg::Rotate(-90))),
                false
            ),
            icon_button(
                icons::rotate_right(),
                tr("Turn right (R)"),
                any.then_some(Message::Pages(PagesMsg::Rotate(90))),
                false
            ),
            icon_button(
                icons::move_back(),
                tr("Move earlier (Ctrl+←)"),
                any.then_some(Message::Pages(PagesMsg::MoveBy(-1))),
                false
            ),
            icon_button(
                icons::move_forward(),
                tr("Move later (Ctrl+→)"),
                any.then_some(Message::Pages(PagesMsg::MoveBy(1))),
                false
            ),
            icon_button(icons::delete(), tr("Delete (Del)"), any.then_some(Message::Pages(PagesMsg::Delete)), false),
            separator(),
            icon_button(icons::insert(), tr("Insert pages from another PDF…"), p(PagesMsg::Insert), false),
            icon_button(
                icons::export(),
                tr("Export the selected pages…"),
                any.then_some(Message::Export(super::export::ExportMsg::Open)),
                false
            ),
            separator(),
            icon_button(
                icons::undo(),
                tr("Undo (Ctrl+Z)"),
                (!o.past.is_empty()).then_some(Message::Pages(PagesMsg::Undo)),
                false
            ),
            icon_button(
                icons::redo(),
                tr("Redo (Ctrl+Shift+Z)"),
                (!o.future.is_empty()).then_some(Message::Pages(PagesMsg::Redo)),
                false
            ),
            separator(),
            text(status).size(12).style(muted),
            button(text(tr("Cancel")).size(13))
                .padding([6, 14])
                .style(style::secondary)
                .on_press(Message::Pages(PagesMsg::Close)),
            tip(save.into(), tr("Save the new page order into the PDF (Ctrl+S)")),
            icon_button(
                icons::save_copy(),
                tr("Save as a new PDF… (Ctrl+Shift+S)"),
                (!busy).then_some(Message::Pages(PagesMsg::SaveAs)),
                false
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        container(content).padding([4, 6]).style(style::panel).into()
    }

    pub(super) fn pages_view(&self) -> Element<'_, Message> {
        let Some(o) = &self.organizer else { return space().into() };
        let cols = self.grid_columns();
        let rows = self.grid_rows();
        let total_rows = o.plan.len().div_ceil(cols) + usize::from(o.dragging);
        let mut grid = column![space().height(TOP_INSET + rows.start as f32 * CELL_H)].padding([0.0, GAP]);
        for r in rows.clone().take_while(|&r| r < total_rows) {
            let mut line = row![].spacing(0);
            for c in 0..cols {
                let pos = r * cols + c;
                if pos < o.plan.len() {
                    line = line.push(self.page_cell(o, pos));
                } else if pos == o.plan.len() && o.dragging {
                    // Somewhere to drop pages at the very end.
                    let target = o.drop_at == Some(pos);
                    let end = container(center(text(tr("End")).size(13).style(muted)))
                        .width(CELL_W - 2.0 * GAP)
                        .height(CELL_H - 2.0 * GAP)
                        .style(move |t: &iced::Theme| style::drop_slot(t, target));
                    line = line
                        .push(container(mouse_area(end).on_enter(Message::Pages(PagesMsg::Enter(pos)))).padding(GAP));
                }
            }
            grid = grid.push(line);
        }
        let below = total_rows.saturating_sub(rows.end) as f32 * CELL_H;
        grid = grid.push(space().height(below.max(0.0) + 24.0));
        scrollable(grid.width(Length::Fill))
            .id(grid_id())
            .on_scroll(|vp| Message::Pages(PagesMsg::Scrolled(vp.absolute_offset().y, vp.bounds().height)))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn page_cell<'a>(&'a self, o: &'a Organizer, pos: usize) -> Element<'a, Message> {
        let entry = o.plan[pos];
        let doc = &o.sources[entry.source];
        let (pw, ph) = doc.page_size(entry.index);
        let turned = entry.rotate % 180 != 0;
        let (w, h) = if turned { (ph, pw) } else { (pw, ph) };
        let scale = THUMB / w.max(h).max(1.0);
        let (tw, th) = (w * scale, h * scale);
        let picture: Element<'_, Message> = match o.thumbs.get(&(entry.source, entry.index)) {
            // The turned picture is fitted (not stretched) into the turned page's box.
            Some(handle) => image(handle.clone())
                .width(tw)
                .height(th)
                .content_fit(iced::ContentFit::Contain)
                .rotation(Rotation::Solid(Radians((entry.rotate as f32).to_radians())))
                .into(),
            None => container(space()).width(tw).height(th).style(style::thumb_placeholder).into(),
        };
        let paper = container(picture).style(style::page);
        let label = if entry.source == 0 {
            format!("{}", entry.index + 1)
        } else {
            format!("{} · {}", entry.index + 1, file_name(doc.path()))
        };
        let selected = o.selected.contains(&pos);
        let target = o.drop_at == Some(pos) && o.dragging;
        let cell = column![
            container(paper).width(Length::Fill).height(THUMB + 8.0).center_x(Length::Fill).center_y(THUMB + 8.0),
            text(label)
                .size(12)
                .font(Font { weight: iced::font::Weight::Medium, ..crate::style::FONT })
                .wrapping(text::Wrapping::None),
        ]
        .spacing(6)
        .align_x(Alignment::Center);
        let framed = container(cell)
            .padding(8)
            .width(CELL_W - 2.0 * GAP)
            .height(CELL_H - 2.0 * GAP)
            .clip(true)
            .style(move |t: &iced::Theme| style::page_cell(t, selected, target));
        let area = mouse_area(framed)
            .on_press(Message::Pages(PagesMsg::Press(pos)))
            .on_enter(Message::Pages(PagesMsg::Enter(pos)))
            .interaction(if o.dragging {
                iced::mouse::Interaction::Grabbing
            } else {
                iced::mouse::Interaction::Pointer
            });
        container(area).padding(GAP).into()
    }
}
