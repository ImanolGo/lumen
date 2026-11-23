//! Lumen's own file chooser, for desktops without a file chooser portal: open files,
//! open a folder, pick a PDF to insert pages from, or choose where to save a copy.
//!
//! The portal is always asked first ([`App::ask_for_files`]); this dialog appears
//! only when there is none. Once confirmed it sends the same message the portal's
//! answer would have, so the rest of Lumen does not know the difference.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use iced::widget::{
    Id, button, center, checkbox, column, container, mouse_area, opaque, operation, row, scrollable, space, stack, svg,
    text, text_input,
};
use iced::{Alignment, Element, Font, Length, Task, Theme};

use super::export::ExportMsg;
use super::markup::MarkupMsg;
use super::pages::PagesMsg;
use super::{App, Message, blocking, err_string, file_name, muted};
use crate::i18n::tr;
use crate::library::Kind;
use crate::{icons, style};

const ROW: f32 = 32.0;
const LIST_H: f32 = 340.0;
const DOUBLE_CLICK_MS: u128 = 450;

fn list_id() -> Id {
    Id::new("chooser-list")
}
fn name_id() -> Id {
    Id::new("chooser-name")
}

/// What the chooser is for, and where its answer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    OpenFiles,
    OpenFolder,
    /// A PDF to insert pages from (the page organiser).
    InsertPdf,
    /// Where to save a copy of the open file (markup, edits, form values).
    SaveCopy,
    /// Where to save the rearranged PDF (the page organiser).
    SavePages,
    /// Where to write an export (any file name).
    Export,
    /// The folder to write several exported files into.
    ExportFolder,
}

impl Purpose {
    fn saves(self) -> bool {
        matches!(self, Self::SaveCopy | Self::SavePages | Self::Export)
    }

    /// Choosing a folder rather than a file.
    fn folder(self) -> bool {
        matches!(self, Self::OpenFolder | Self::ExportFolder)
    }

    fn title(self) -> String {
        match self {
            Self::OpenFiles => tr("Open Files"),
            Self::OpenFolder => tr("Open Folder"),
            Self::InsertPdf => tr("Insert Pages From"),
            Self::SaveCopy | Self::SavePages => tr("Save a Copy"),
            Self::Export => tr("Export"),
            Self::ExportFolder => tr("Export to Folder"),
        }
    }

    /// Whether a file belongs in the list.
    fn shows(self, path: &Path) -> bool {
        match self {
            Self::OpenFiles => Kind::of(path).is_some(),
            Self::OpenFolder | Self::ExportFolder => false,
            Self::InsertPdf => Kind::of(path) == Some(Kind::Pdf),
            Self::SaveCopy | Self::SavePages | Self::Export => true,
        }
    }

    /// The message the portal's answer would have become.
    fn answer(self, paths: Vec<PathBuf>) -> Message {
        let one = paths.into_iter().next();
        match self {
            Self::OpenFiles | Self::OpenFolder => Message::Chosen(Ok(one.into_iter().collect())),
            Self::InsertPdf => Message::Pages(PagesMsg::InsertChosen(Ok(one.into_iter().collect()))),
            Self::SaveCopy => Message::Markup(MarkupMsg::SaveTo(Ok(one))),
            Self::SavePages => Message::Pages(PagesMsg::SaveTo(Ok(one))),
            Self::Export | Self::ExportFolder => Message::Export(ExportMsg::Target(Ok(one))),
        }
    }

    fn answer_many(self, paths: Vec<PathBuf>) -> Message {
        match self {
            Self::OpenFiles => Message::Chosen(Ok(paths)),
            Self::InsertPdf => Message::Pages(PagesMsg::InsertChosen(Ok(paths))),
            _ => self.answer(paths),
        }
    }

    fn failed(self, error: String) -> Message {
        match self {
            Self::OpenFiles | Self::OpenFolder => Message::Chosen(Err(error)),
            Self::InsertPdf => Message::Pages(PagesMsg::InsertChosen(Err(error))),
            Self::SaveCopy => Message::Markup(MarkupMsg::SaveTo(Err(error))),
            Self::SavePages => Message::Pages(PagesMsg::SaveTo(Err(error))),
            Self::Export | Self::ExportFolder => Message::Export(ExportMsg::Target(Err(error))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

pub struct Chooser {
    purpose: Purpose,
    dir: PathBuf,
    entries: Vec<Entry>,
    loading: bool,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    last_click: Option<(usize, Instant)>,
    /// The file name, when saving.
    name: String,
    hidden: bool,
    scroll: f32,
    /// The chosen file exists: the next confirmation replaces it.
    replace: bool,
    error: Option<String>,
    seq: u64,
}

#[derive(Debug, Clone)]
pub enum ChooserMsg {
    Show(Purpose, Option<PathBuf>),
    Listed(u64, Result<Vec<Entry>, String>),
    Go(PathBuf),
    Up,
    Click(usize),
    Name(String),
    Hidden(bool),
    Scrolled(f32),
    Confirm,
    Cancel,
}

/// The folders offered on the left: home, the usual user folders, the whole computer.
fn places() -> Vec<(String, PathBuf)> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return vec![("Computer".into(), "/".into())] };
    let mut out = vec![("Home".to_string(), home.clone())];
    // ~/.config/user-dirs.dirs knows the (possibly translated) names.
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    for key in ["DESKTOP", "DOCUMENTS", "DOWNLOAD", "PICTURES"] {
        let path = dirs
            .lines()
            .find_map(|l| l.strip_prefix(&format!("XDG_{key}_DIR=")))
            .map(|v| PathBuf::from(v.trim_matches('"').replace("$HOME", &home.to_string_lossy())))
            .unwrap_or_else(|| {
                home.join(match key {
                    "DESKTOP" => "Desktop",
                    "DOCUMENTS" => "Documents",
                    "DOWNLOAD" => "Downloads",
                    _ => "Pictures",
                })
            });
        if path.is_dir() && path != home && !out.iter().any(|(_, p)| *p == path) {
            out.push((file_name(&path), path));
        }
    }
    out.push(("Computer".into(), "/".into()));
    out
}

fn list(dir: &Path, purpose: Purpose, hidden: bool) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                return None;
            }
            let path = e.path();
            let meta = std::fs::metadata(&path).ok()?;
            let dir = meta.is_dir();
            (dir || (meta.is_file() && purpose.shows(&path))).then(|| Entry {
                name,
                path,
                dir,
                size: meta.len(),
                modified: meta.modified().ok(),
            })
        })
        .take(50_000)
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| natord::compare_ignore_case(&a.name, &b.name)));
    Ok(entries)
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / 1048576.0),
        b => format!("{:.1} GB", b as f64 / 1073741824.0),
    }
}

fn age(t: Option<SystemTime>) -> String {
    let Some(secs) = t.and_then(|t| SystemTime::now().duration_since(t).ok()).map(|d| d.as_secs()) else {
        return String::new();
    };
    match secs {
        s if s < 3600 => tr("Just now"),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 86_400 * 60 => format!("{} days ago", s / 86_400),
        s if s < 86_400 * 730 => format!("{} months ago", s / (86_400 * 30)),
        s => format!("{} years ago", s / (86_400 * 365)),
    }
}

impl App {
    /// Asks the desktop's file chooser for what `purpose` needs, or Lumen's own when there is none.
    pub(super) fn ask_for_files(purpose: Purpose, suggested: Option<PathBuf>) -> Task<Message> {
        Task::perform(
            async move {
                let result = match purpose {
                    Purpose::OpenFiles | Purpose::InsertPdf => crate::portal::choose(false).await,
                    Purpose::OpenFolder | Purpose::ExportFolder => crate::portal::choose(true).await,
                    Purpose::SaveCopy | Purpose::SavePages | Purpose::Export => match suggested.clone() {
                        Some(s) => crate::portal::save_as(s).await.map(|p| p.into_iter().collect()),
                        None => Ok(Vec::new()),
                    },
                };
                match result {
                    Ok(paths) if purpose.saves() => purpose.answer(paths),
                    Ok(paths) => purpose.answer_many(paths),
                    Err(e) if e.is::<crate::portal::NoPortal>() => {
                        Message::Chooser(ChooserMsg::Show(purpose, suggested))
                    }
                    Err(e) => purpose.failed(err_string(e)),
                }
            },
            |m| m,
        )
    }

    fn list_dir(&mut self) -> Task<Message> {
        let Some(c) = &mut self.chooser else { return Task::none() };
        c.seq += 1;
        c.loading = true;
        c.selected.clear();
        c.anchor = None;
        c.replace = false;
        c.scroll = 0.0;
        let (seq, dir, purpose, hidden) = (c.seq, c.dir.clone(), c.purpose, c.hidden);
        Task::batch([
            Task::perform(blocking(move || list(&dir, purpose, hidden)), move |r| {
                Message::Chooser(ChooserMsg::Listed(seq, r.and_then(|r| r)))
            }),
            operation::scroll_to(list_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
        ])
    }

    pub(super) fn chooser_update(&mut self, msg: ChooserMsg) -> Task<Message> {
        if let ChooserMsg::Show(purpose, suggested) = msg {
            let start = suggested
                .as_ref()
                .and_then(|p| p.parent())
                .filter(|p| p.is_dir())
                .map(Path::to_path_buf)
                .or_else(|| self.current_path().and_then(Path::parent).map(Path::to_path_buf))
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                .unwrap_or_else(|| "/".into());
            self.chooser = Some(Chooser {
                purpose,
                dir: start,
                entries: Vec::new(),
                loading: true,
                selected: BTreeSet::new(),
                anchor: None,
                last_click: None,
                name: suggested.as_deref().map(file_name).unwrap_or_default(),
                hidden: false,
                scroll: 0.0,
                replace: false,
                error: None,
                seq: 0,
            });
            let focus = if purpose.saves() { operation::focus(name_id()) } else { Task::none() };
            return Task::batch([self.list_dir(), focus]);
        }
        let shift = self.modifiers.shift();
        let ctrl = self.modifiers.command();
        let Some(c) = &mut self.chooser else { return Task::none() };
        match msg {
            ChooserMsg::Show(..) => Task::none(),
            ChooserMsg::Listed(seq, result) => {
                if seq == c.seq {
                    c.loading = false;
                    match result {
                        Ok(entries) => {
                            c.entries = entries;
                            c.error = None;
                        }
                        Err(e) => {
                            c.entries.clear();
                            c.error = Some(e);
                        }
                    }
                }
                Task::none()
            }
            ChooserMsg::Go(dir) => {
                c.dir = dir;
                self.list_dir()
            }
            ChooserMsg::Up => {
                let Some(parent) = c.dir.parent().map(Path::to_path_buf) else { return Task::none() };
                c.dir = parent;
                self.list_dir()
            }
            ChooserMsg::Click(i) => {
                let Some(entry) = c.entries.get(i).cloned() else { return Task::none() };
                let double = c.last_click.is_some_and(|(j, t)| j == i && t.elapsed().as_millis() < DOUBLE_CLICK_MS);
                c.last_click = Some((i, Instant::now()));
                c.replace = false;
                if double {
                    return if entry.dir {
                        self.chooser_update(ChooserMsg::Go(entry.path))
                    } else {
                        self.chooser_update(ChooserMsg::Confirm)
                    };
                }
                let many = c.purpose == Purpose::OpenFiles;
                if many && ctrl {
                    if !c.selected.remove(&i) {
                        c.selected.insert(i);
                    }
                } else if many
                    && shift
                    && let Some(a) = c.anchor
                {
                    c.selected = (a.min(i)..=a.max(i)).collect();
                } else {
                    c.selected = BTreeSet::from([i]);
                }
                c.anchor = Some(i);
                if c.purpose.saves() && !entry.dir {
                    c.name = entry.name;
                }
                Task::none()
            }
            ChooserMsg::Name(name) => {
                c.name = name.replace('/', "");
                c.replace = false;
                Task::none()
            }
            ChooserMsg::Hidden(on) => {
                c.hidden = on;
                self.list_dir()
            }
            ChooserMsg::Scrolled(y) => {
                c.scroll = y;
                Task::none()
            }
            ChooserMsg::Cancel => {
                self.chooser = None;
                Task::none()
            }
            ChooserMsg::Confirm => {
                let chosen: Vec<Entry> = c.selected.iter().filter_map(|&i| c.entries.get(i).cloned()).collect();
                let purpose = c.purpose;
                // A single folder picked: go into it (or, opening a folder, open it).
                if let [one] = chosen.as_slice()
                    && one.dir
                    && !purpose.folder()
                {
                    let dir = one.path.clone();
                    return self.chooser_update(ChooserMsg::Go(dir));
                }
                let paths: Vec<PathBuf> = match purpose {
                    Purpose::OpenFolder | Purpose::ExportFolder => {
                        vec![chosen.first().filter(|e| e.dir).map_or_else(|| c.dir.clone(), |e| e.path.clone())]
                    }
                    Purpose::SaveCopy | Purpose::SavePages | Purpose::Export => {
                        let name = c.name.trim();
                        if name.is_empty() {
                            return Task::none();
                        }
                        let path = c.dir.join(name);
                        if path.is_dir() {
                            return self.chooser_update(ChooserMsg::Go(path));
                        }
                        if path.exists() && !c.replace {
                            c.replace = true;
                            return Task::none();
                        }
                        vec![path]
                    }
                    _ => chosen.into_iter().filter(|e| !e.dir).map(|e| e.path).collect(),
                };
                if paths.is_empty() {
                    return Task::none();
                }
                self.chooser = None;
                let answer = if purpose.saves() { purpose.answer(paths) } else { purpose.answer_many(paths) };
                Task::done(answer)
            }
        }
    }

    /// Keys while the chooser is open; it takes them all.
    pub(super) fn chooser_key(&mut self, key: &iced::keyboard::Key) -> Option<Task<Message>> {
        use iced::keyboard::{Key, key::Named};
        self.chooser.as_ref()?;
        Some(match key.as_ref() {
            Key::Named(Named::Enter) => self.chooser_update(ChooserMsg::Confirm),
            Key::Named(Named::Backspace) => self.chooser_update(ChooserMsg::Up),
            _ => Task::none(),
        })
    }

    pub(super) fn chooser_view(&self) -> Option<Element<'_, Message>> {
        let c = self.chooser.as_ref()?;
        let m = |x: ChooserMsg| Message::Chooser(x);

        // The path, as buttons to each folder on the way.
        let mut crumbs = row![].spacing(2).align_y(Alignment::Center);
        let mut at = PathBuf::new();
        for (k, part) in c.dir.components().enumerate() {
            at.push(part);
            let label = if k == 0 { "/".to_string() } else { part.as_os_str().to_string_lossy().into_owned() };
            if k > 1 {
                crumbs = crumbs.push(text("›").size(13).style(muted));
            }
            crumbs = crumbs.push(
                button(text(label).size(13).wrapping(text::Wrapping::None))
                    .padding([4, 6])
                    .style(style::menu_item)
                    .on_press(m(ChooserMsg::Go(at.clone()))),
            );
        }
        let bar = row![
            super::icon_button(icons::up(), tr("Up (Backspace)"), c.dir.parent().map(|_| m(ChooserMsg::Up)), false),
            scrollable(crumbs)
                .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(0).scroller_width(0)))
                .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let mut side = column![].spacing(2).width(150);
        for (name, path) in places() {
            let here = path == c.dir;
            side = side.push(
                button(text(name).size(13).wrapping(text::Wrapping::None))
                    .padding([6, 10])
                    .width(Length::Fill)
                    .style(style::segment(here))
                    .on_press(m(ChooserMsg::Go(path))),
            );
        }

        // Only the rows in view become widgets, so big folders stay quick.
        let first = ((c.scroll / ROW).floor().max(0.0) as usize).saturating_sub(4).min(c.entries.len());
        let last = (((c.scroll + LIST_H) / ROW).ceil() as usize + 4).min(c.entries.len());
        let mut rows = column![space().height(first as f32 * ROW)];
        for (i, e) in c.entries.iter().enumerate().take(last).skip(first) {
            let icon = if e.dir { icons::folder() } else { icons::open() };
            let line = row![
                svg(icon).width(18).height(18).style(style::icon_muted),
                text(e.name.as_str()).size(13).width(Length::Fill).wrapping(text::Wrapping::None),
                text(if e.dir { String::new() } else { human_size(e.size) })
                    .size(12)
                    .style(muted)
                    .width(70)
                    .align_x(Alignment::End),
                text(age(e.modified)).size(12).style(muted).width(100).align_x(Alignment::End),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            rows = rows.push(
                button(container(line).clip(true))
                    .padding([0, 10])
                    .height(ROW)
                    .width(Length::Fill)
                    .style(style::segment(c.selected.contains(&i)))
                    .on_press(m(ChooserMsg::Click(i))),
            );
        }
        rows = rows.push(space().height((c.entries.len() - last) as f32 * ROW));
        let listing: Element<'_, Message> = if c.loading && c.entries.is_empty() {
            center(text(tr("Loading…")).size(13).style(muted)).into()
        } else if let Some(e) = &c.error {
            center(text(e.as_str()).size(13).style(muted)).into()
        } else if c.entries.is_empty() {
            center(
                text(if c.purpose.folder() { tr("No folders here") } else { tr("Nothing here") }).size(13).style(muted),
            )
            .into()
        } else {
            scrollable(rows.width(Length::Fill))
                .id(list_id())
                .on_scroll(|vp| Message::Chooser(ChooserMsg::Scrolled(vp.absolute_offset().y)))
                .height(Length::Fill)
                .into()
        };
        let listing = container(listing).height(LIST_H).width(Length::Fill).style(|t: &Theme| {
            let p = t.extended_palette();
            container::Style {
                border: iced::Border { radius: 8.0.into(), width: 1.0, color: p.background.strong.color },
                ..Default::default()
            }
        });

        let confirm = match c.purpose {
            Purpose::OpenFiles | Purpose::OpenFolder => tr("Open"),
            Purpose::InsertPdf => tr("Insert"),
            Purpose::ExportFolder => tr("Export here"),
            _ if c.replace => tr("Replace"),
            _ => tr("Save"),
        };
        let can = match c.purpose {
            p if p.folder() => true,
            p if p.saves() => !c.name.trim().is_empty(),
            _ => !c.selected.is_empty(),
        };
        let mut bottom = row![
            checkbox(c.hidden)
                .label(tr("Hidden files"))
                .on_toggle(move |on| m(ChooserMsg::Hidden(on)))
                .text_size(13)
                .size(16)
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let file_name_hint = tr("File name");
        if c.purpose.saves() {
            bottom = bottom.push(
                text_input(&file_name_hint, &c.name)
                    .id(name_id())
                    .on_input(move |t| m(ChooserMsg::Name(t)))
                    .on_submit(m(ChooserMsg::Confirm))
                    .size(14)
                    .padding([6, 8])
                    .width(Length::Fill),
            );
        } else {
            bottom = bottom.push(space().width(Length::Fill));
        }
        bottom = bottom
            .push(
                button(text(tr("Cancel")).size(14))
                    .padding([8, 16])
                    .style(style::secondary)
                    .on_press(m(ChooserMsg::Cancel)),
            )
            .push(
                button(text(confirm).size(14))
                    .padding([8, 16])
                    .style(if c.replace { iced::widget::button::danger } else { style::primary })
                    .on_press_maybe(can.then_some(m(ChooserMsg::Confirm))),
            );
        let mut card = column![
            text(c.purpose.title()).size(18).font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT }),
            bar,
            row![side, listing].spacing(12),
        ]
        .spacing(12);
        if c.replace {
            card = card.push(text(format!("“{}” already exists. Saving will replace it.", c.name)).size(13));
        }
        let card = container(card.push(bottom).width(Length::Fixed(760.0_f32.min(self.area.width.max(560.0)))))
            .padding(20)
            .style(style::panel);
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill)).on_press(m(ChooserMsg::Cancel));
        Some(stack![container(backdrop).style(style::scrim), center(opaque(card))].into())
    }
}
