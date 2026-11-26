//! Renaming the file on screen (F2), in its folder. Never over another file.

use std::path::{Path, PathBuf};

use iced::widget::{column, operation, text};
use iced::{Element, Task};

use super::{App, Message, TOOLBAR_H, blocking, err_string, file_name, muted};
use crate::i18n::tr;
use crate::{style, ui};

#[derive(Debug, Clone)]
pub enum RenameMsg {
    Open,
    Input(String),
    Go,
    Close,
    Done(PathBuf, Result<PathBuf, String>),
}

/// What is wrong with `name` as a new name for `path`, if anything.
fn problem(path: &Path, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some(tr("Type a name"));
    }
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Some(tr("A name cannot have a slash in it"));
    }
    let target = path.with_file_name(name);
    let same = target == path;
    // A change of case only is the same file on some disks: that is allowed.
    if !same && target.symlink_metadata().is_ok() && !name.eq_ignore_ascii_case(&file_name(path)) {
        return Some(tr("There is a file of that name already"));
    }
    None
}

impl App {
    pub(super) fn rename_update(&mut self, msg: RenameMsg) -> Task<Message> {
        match msg {
            RenameMsg::Open => {
                let Some(path) = self.current_file() else { return Task::none() };
                if self.has_unsaved_changes() || self.organizer.is_some() {
                    return self.toast(tr("Save or discard the changes first, then rename it"));
                }
                let name = file_name(&path);
                // The name without its ending is what is usually changed.
                let stem = name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len());
                let stem = name[..stem].chars().count();
                self.menu = None;
                self.renaming = Some(name);
                let id = self.widget_id("rename");
                Task::batch([operation::focus(id.clone()), operation::select_range(id, 0, stem)])
            }
            RenameMsg::Input(name) => {
                if let Some(typed) = &mut self.renaming {
                    *typed = name;
                }
                Task::none()
            }
            RenameMsg::Close => {
                self.renaming = None;
                Task::none()
            }
            RenameMsg::Go => {
                let (Some(name), Some(path)) = (self.renaming.clone(), self.current_file()) else {
                    return Task::none();
                };
                if problem(&path, &name).is_some() {
                    return Task::none();
                }
                let target = path.with_file_name(name.trim());
                self.renaming = None;
                if target == path {
                    return Task::none();
                }
                Task::perform(
                    {
                        let (path, target) = (path.clone(), target.clone());
                        blocking(move || {
                            crate::fsio::rename_new(&path, &target)
                                .map(|()| target)
                                .map_err(|e| err_string(anyhow::Error::from(e)))
                        })
                    },
                    move |r| Message::Rename(RenameMsg::Done(path.clone(), r.and_then(|r| r))),
                )
            }
            RenameMsg::Done(_, Err(e)) => self.toast(trf!("Could not rename it: {}", e)),
            RenameMsg::Done(old, Ok(new)) => {
                let Some(at) = self.files.iter().position(|p| *p == old) else { return Task::none() };
                self.files[at] = new.clone();
                if let Some(thumb) = self.thumbs.remove(&old) {
                    self.thumbs.insert(new.clone(), thumb);
                }
                self.pictures.retain(|(p, _)| *p != old);
                if let Some(password) = self.passwords.remove(&old) {
                    self.passwords.insert(new.clone(), password);
                }
                self.state.rename(&old, &new);
                let notice = self.notice(trf!("Renamed to “{}”", file_name(&new)));
                // Shown again from where it is now.
                if at == self.current {
                    self.current = usize::MAX;
                    return Task::batch([notice, self.show(at), self.state_changed()]);
                }
                Task::batch([notice, self.state_changed()])
            }
        }
    }

    pub(super) fn rename_dialog(&self) -> Option<Element<'_, Message>> {
        let typed = self.renaming.as_ref()?;
        let path = self.current_file()?;
        let wrong = problem(&path, typed).filter(|_| !typed.is_empty());
        let input = ui::input("", typed)
            .id(self.widget_id("rename"))
            .on_input(|t| Message::Rename(RenameMsg::Input(t)))
            .on_submit(Message::Rename(RenameMsg::Go));
        let mut body = column![input].spacing(8);
        if let Some(wrong) = &wrong {
            body = body.push(text(wrong.clone()).size(style::SMALL).style(super::problem));
        } else {
            body = body.push(text(trf!("In {}", super::shorten_home(path.parent()?))).size(style::SMALL).style(muted));
        }
        let can = problem(&path, typed).is_none();
        Some(
            ui::dialog(tr("Rename"), ui::ASK, body)
                .cancel(tr("Cancel"), Message::Rename(RenameMsg::Close))
                .button(ui::Kind::Main, tr("Rename"), can.then_some(Message::Rename(RenameMsg::Go)))
                .view(TOOLBAR_H),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_name_must_be_a_name_and_free() {
        let dir = std::env::temp_dir().join(format!("lumen-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(dir.join("b.png"), b"x").unwrap();
        assert!(problem(&a, "c.png").is_none());
        assert!(problem(&a, "a.png").is_none(), "unchanged is not a problem");
        assert!(problem(&a, "A.png").is_none(), "nor a change of case");
        assert!(problem(&a, "b.png").is_some());
        assert!(problem(&a, "  ").is_some() && problem(&a, "x/y.png").is_some() && problem(&a, "..").is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
