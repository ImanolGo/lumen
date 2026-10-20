//! The control interface: Lumen's state and operations on the session bus, for scripts and
//! tests. `io.github.imanolgo.Lumen.Control1` at `/io/github/imanolgo/Lumen/Control`.
//!
//! Every method returns JSON (most of them the state afterwards, see `state`). Work that
//! finishes later (opening, saving, page changes, form fields, exports, searching) answers
//! when it has finished, or with an error. Nothing destructive happens implicitly: replacing
//! unsaved edits needs `discard`, writing over a file `overwrite`, and saving a rearranged
//! protected PDF without its password `drop-password`. No method ever waits on a dialog.

use std::{cell::RefCell, collections::HashMap, path::PathBuf};

use relm4::{
    ComponentSender,
    adw::{self, prelude::*},
    gtk::{gio, glib},
};
use serde_json::{Value, json};

use super::{App, AppMsg, edit::Pending, pages::PageRequest};
use crate::{
    annotate::{Annotation, Color, Point, Rect, Shape, Style, Tool},
    edit::{Adjustments, CropRect, EditOp},
    pdf::{FieldKind, FieldValue, HitRect},
    save::Format,
};

pub const OBJECT_PATH: &str = "/io/github/imanolgo/Lumen/Control";
const INTERFACE: &str = "io.github.imanolgo.Lumen.Control1";

const XML: &str = r#"<node>
  <interface name="io.github.imanolgo.Lumen.Control1">
    <method name="GetState"><arg name="state" type="s" direction="out"/></method>
    <method name="Open">
      <arg name="paths" type="as" direction="in"/>
      <arg name="options" type="a{sv}" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="GotoPage">
      <arg name="page" type="u" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="ListPages"><arg name="pages" type="s" direction="out"/></method>
    <method name="ListFields"><arg name="fields" type="s" direction="out"/></method>
    <method name="FillField">
      <arg name="id" type="i" direction="in"/>
      <arg name="value" type="v" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="ListAnnotations">
      <arg name="page" type="u" direction="in"/>
      <arg name="annotations" type="s" direction="out"/>
    </method>
    <method name="Annotate">
      <arg name="page" type="u" direction="in"/>
      <arg name="annotation" type="s" direction="in"/>
      <arg name="result" type="s" direction="out"/>
    </method>
    <method name="DeleteAnnotation">
      <arg name="page" type="u" direction="in"/>
      <arg name="id" type="t" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="EditImage">
      <arg name="operation" type="s" direction="in"/>
      <arg name="args" type="a{sv}" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="ChangePages">
      <arg name="operation" type="s" direction="in"/>
      <arg name="pages" type="au" direction="in"/>
      <arg name="args" type="a{sv}" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="Undo"><arg name="state" type="s" direction="out"/></method>
    <method name="Redo"><arg name="state" type="s" direction="out"/></method>
    <method name="Save">
      <arg name="path" type="s" direction="in"/>
      <arg name="options" type="a{sv}" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="Export">
      <arg name="kind" type="s" direction="in"/>
      <arg name="path" type="s" direction="in"/>
      <arg name="options" type="a{sv}" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="Search">
      <arg name="text" type="s" direction="in"/>
      <arg name="result" type="s" direction="out"/>
    </method>
    <method name="SetMarkup">
      <arg name="options" type="a{sv}" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
    <method name="Screenshot">
      <arg name="path" type="s" direction="in"/>
      <arg name="state" type="s" direction="out"/>
    </method>
  </interface>
</node>"#;

thread_local! {
    /// Calls waiting to be handled or answered, by the id their message carries (a method
    /// invocation cannot travel in a message, which must be `Send`).
    static CALLS: RefCell<HashMap<u64, gio::DBusMethodInvocation>> = RefCell::default();
}

/// Put the interface on the application's connection. Calls arrive as `AppMsg::Control`.
pub fn register(sender: &ComponentSender<App>) {
    let app = relm4::main_application();
    let Some(bus) = app.dbus_connection() else {
        return;
    };
    let info = gio::DBusNodeInfo::for_xml(XML)
        .ok()
        .and_then(|node| node.lookup_interface(INTERFACE))
        .expect("the interface XML is valid");
    let sender = sender.clone();
    let next = std::cell::Cell::new(0u64);
    let registered = bus
        .register_object(OBJECT_PATH, &info)
        .method_call(move |_, _, _, _, method, params, invocation| {
            next.set(next.get() + 1);
            let id = next.get();
            CALLS.with(|calls| calls.borrow_mut().insert(id, invocation));
            sender.input(AppMsg::Control {
                id,
                method: method.to_owned(),
                params,
            });
        })
        .build();
    if let Err(err) = registered {
        tracing::warn!("cannot offer the control interface: {err}");
    }
}

/// What a call that answers later is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    Load,
    Reload,
    Save,
    Export,
    Search,
}

#[derive(Default)]
pub struct ControlState {
    waiting: Vec<(Wait, u64)>,
    /// The password to try if the file being opened for a call needs one.
    password: Option<String>,
}

/// A failure to report: a D-Bus error name (after `io.github.imanolgo.Lumen.Error.`) and a message.
struct Failure(&'static str, String);

fn fail(kind: &'static str, message: impl Into<String>) -> Failure {
    Failure(kind, message.into())
}

fn answer(id: u64, result: Result<Value, Failure>) {
    let Some(invocation) = CALLS.with(|calls| calls.borrow_mut().remove(&id)) else {
        return;
    };
    match result {
        Ok(value) => invocation.return_value(Some(&(value.to_string(),).to_variant())),
        Err(Failure(kind, message)) => invocation
            .return_dbus_error(&format!("io.github.imanolgo.Lumen.Error.{kind}"), &message),
    }
}

/// Options from an `a{sv}`.
struct Options(glib::VariantDict);

impl Options {
    fn new(variant: Option<glib::Variant>) -> Self {
        Self(glib::VariantDict::new(variant.as_ref()))
    }
    fn get<T: glib::variant::FromVariant>(&self, key: &str) -> Option<T> {
        self.0.lookup::<T>(key).ok().flatten()
    }
    fn flag(&self, key: &str) -> bool {
        self.get::<bool>(key).unwrap_or(false)
    }
}

impl App {
    /// A call arrived: handle it, or keep it until the work it started has finished.
    pub(super) fn control(
        &mut self,
        id: u64,
        method: &str,
        params: glib::Variant,
        sender: &ComponentSender<App>,
    ) {
        let child = |i: usize| params.try_child_value(i);
        let result = match method {
            "GetState" => Ok(Some(self.state())),
            "Open" => self.control_open(id, child(0), child(1), sender),
            "GotoPage" => match (&self.pdf, child(0).and_then(|v| v.get::<u32>())) {
                (Some(pdf), Some(page)) if (page as usize) < pdf.n_pages => {
                    self.goto_page(page as usize);
                    Ok(Some(self.state()))
                }
                (Some(_), _) => Err(fail("InvalidArgs", "no such page")),
                (None, _) => Err(fail("NotSupported", "no PDF is open")),
            },
            "ListPages" => self.list_pages().map(Some),
            "ListFields" => self.list_fields().map(Some),
            "FillField" => self.control_fill(id, child(0), child(1), sender),
            "ListAnnotations" => self
                .list_annotations(child(0).and_then(|v| v.get::<u32>()).unwrap_or(0) as usize)
                .map(Some),
            "Annotate" => self.control_annotate(child(0), child(1), sender).map(Some),
            "DeleteAnnotation" => self.control_delete(child(0), child(1), sender).map(Some),
            "EditImage" => self.control_edit(child(0), child(1), sender).map(Some),
            "ChangePages" => self.control_pages(id, child(0), child(1), child(2), sender),
            "Undo" | "Redo" => self.control_history(id, method == "Undo", sender),
            "Save" => self.control_save(id, child(0), child(1), sender),
            "Export" => self.control_export(id, child(0), child(1), child(2), sender),
            "Search" => self.control_search(id, child(0)),
            "SetMarkup" => self.control_markup(child(0), sender).map(Some),
            "Screenshot" => self.control_screenshot(child(0)).map(Some),
            _ => Err(fail("UnknownMethod", format!("no method {method}"))),
        };
        match result {
            // Answered later, by `control_finished`.
            Ok(None) => {}
            Ok(Some(value)) => answer(id, Ok(value)),
            Err(failure) => answer(id, Err(failure)),
        }
    }

    /// Work that calls may be waiting for has finished: answer them.
    pub(super) fn control_finished(&mut self, wait: Wait, result: Result<(), String>) {
        let ready: Vec<u64> = self
            .control
            .waiting
            .iter()
            .filter(|(w, _)| *w == wait)
            .map(|(_, id)| *id)
            .collect();
        if ready.is_empty() {
            return;
        }
        self.control.waiting.retain(|(w, _)| *w != wait);
        if wait == Wait::Load {
            self.control.password = None;
        }
        let state = self.state();
        for id in ready {
            let reply = match &result {
                Ok(()) => Ok(state.clone()),
                Err(message) => Err(fail("Failed", message.clone())),
            };
            answer(id, reply);
        }
    }

    /// A save finished and the saved file is being opened again: calls waiting for the
    /// save now wait for that.
    pub(super) fn control_saved_reopens(&mut self) {
        for (wait, _) in &mut self.control.waiting {
            if *wait == Wait::Save {
                *wait = Wait::Load;
            }
        }
    }

    /// Whether a call is waiting for a file to open (so no dialog is shown for it).
    pub(super) fn control_loading(&self) -> bool {
        self.control.waiting.iter().any(|(w, _)| *w == Wait::Load)
    }

    /// The password to try for the file being opened for a call, once.
    pub(super) fn control_password(&mut self) -> Option<String> {
        self.control.password.take()
    }

    fn busy(&self) -> Result<(), Failure> {
        if self.loading || self.page_busy || self.saving || !self.control.waiting.is_empty() {
            Err(fail(
                "Busy",
                "Lumen is still busy with an earlier operation",
            ))
        } else {
            Ok(())
        }
    }

    fn wait(&mut self, wait: Wait, id: u64) -> Result<Option<Value>, Failure> {
        self.control.waiting.push((wait, id));
        Ok(None)
    }

    /// Everything a script needs to know about what is on screen.
    fn state(&self) -> Value {
        let (can_undo, can_redo) = (self.can_undo(), self.can_redo());
        json!({
            "file": self.document.as_ref().map(|d| d.path.display().to_string()),
            "kind": match (&self.pdf, &self.image) {
                (Some(_), _) => Some("pdf"),
                (None, Some(_)) => Some("image"),
                _ => None,
            },
            "files": self.list.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "index": self.index,
            "page": self.pdf.as_ref().map(|p| p.page),
            "page_count": self.pdf.as_ref().map(|p| p.n_pages),
            "size": self.shown_image().map(|i| { let (w, h) = i.display_size(); [w, h] }),
            "zoom": self.zoom,
            "dirty": self.is_dirty(),
            "can_undo": can_undo,
            "can_redo": can_redo,
            "busy": self.loading || self.page_busy || self.saving,
            "markup": {
                "open": self.markup.open,
                "tool": tool_name(self.markup.editor.tool),
            },
            "error": self.error,
        })
    }

    fn control_open(
        &mut self,
        id: u64,
        paths: Option<glib::Variant>,
        options: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Option<Value>, Failure> {
        self.busy()?;
        let paths: Vec<PathBuf> = paths
            .and_then(|v| v.get::<Vec<String>>())
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        if paths.is_empty() {
            return Err(fail("InvalidArgs", "no files given"));
        }
        let options = Options::new(options);
        if options.flag("add") && !self.list.is_empty() {
            // Added files are shown as dropped ones are: the first new one opens.
            sender.input(AppMsg::Add(paths));
            return self.wait(Wait::Load, id);
        }
        if self.is_dirty() && !options.flag("discard") {
            return Err(fail(
                "Unsaved",
                "the open document has unsaved changes; pass discard=true to drop them",
            ));
        }
        self.control.password = options.get::<String>("password");
        if self.is_dirty() {
            sender.input(AppMsg::Discard(Pending::Open(paths)));
        } else {
            sender.input(AppMsg::Open(paths));
        }
        self.wait(Wait::Load, id)
    }

    fn list_pages(&self) -> Result<Value, Failure> {
        let pdf = self
            .pdf
            .as_ref()
            .ok_or_else(|| fail("NotSupported", "no PDF is open"))?;
        Ok(Value::Array(
            pdf.layout
                .rects
                .iter()
                .enumerate()
                .map(|(i, r)| json!({"index": i, "width": r.size().0, "height": r.size().1}))
                .collect(),
        ))
    }

    fn list_fields(&self) -> Result<Value, Failure> {
        if self.pdf.is_none() {
            return Err(fail("NotSupported", "no PDF is open"));
        }
        let forms = self.canvas.forms();
        let mut fields = Vec::new();
        for (page, list) in forms {
            for f in list {
                let (kind, value) = match &f.kind {
                    FieldKind::Text { text, .. } => ("text", json!(text)),
                    FieldKind::Check { on } => ("check", json!(on)),
                    FieldKind::Radio { on } => ("radio", json!(on)),
                    FieldKind::Choice {
                        items, selected, ..
                    } => ("choice", json!({"items": items, "selected": selected})),
                    FieldKind::Signature => ("signature", Value::Null),
                    FieldKind::Push => ("button", Value::Null),
                };
                fields.push(json!({
                    "id": f.id, "page": page, "kind": kind, "value": value,
                    "read_only": f.read_only, "area": area_json(&f.area),
                }));
            }
        }
        Ok(Value::Array(fields))
    }

    fn control_fill(
        &mut self,
        id: u64,
        field: Option<glib::Variant>,
        value: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Option<Value>, Failure> {
        self.busy()?;
        if self.pdf.is_none() {
            return Err(fail("NotSupported", "no PDF is open"));
        }
        let field = field.and_then(|v| v.get::<i32>()).unwrap_or(-1);
        let value = value
            .and_then(|v| v.as_variant())
            .ok_or_else(|| fail("InvalidArgs", "no value"))?;
        let value = if let Some(on) = value.get::<bool>() {
            FieldValue::On(on)
        } else if let Some(text) = value.get::<String>() {
            FieldValue::Text(text)
        } else if let Some(index) = value.get::<u32>() {
            FieldValue::Item(index as usize)
        } else {
            return Err(fail(
                "InvalidArgs",
                "the value must be a boolean, a string or an item index (u)",
            ));
        };
        let known = self
            .canvas
            .forms()
            .values()
            .flatten()
            .any(|f| f.id == field && f.editable());
        if !known {
            return Err(fail(
                "InvalidArgs",
                format!("no field {field} that can be filled in"),
            ));
        }
        self.fill_form(field, value, sender);
        self.wait(Wait::Reload, id)
    }

    fn list_annotations(&self, page: usize) -> Result<Value, Failure> {
        let store = self.markup.store.borrow();
        let items = store
            .layer(page)
            .map(|layer| layer.items.iter().map(annotation_json).collect())
            .unwrap_or_default();
        Ok(Value::Array(items))
    }

    fn control_annotate(
        &mut self,
        page: Option<glib::Variant>,
        text: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Value, Failure> {
        if !self.can_markup() {
            return Err(fail(
                "NotSupported",
                "this document cannot be marked up right now",
            ));
        }
        let page = page.and_then(|v| v.get::<u32>()).unwrap_or(0) as usize;
        if self.pdf.as_ref().is_some_and(|p| page >= p.n_pages) || (self.pdf.is_none() && page != 0)
        {
            return Err(fail("InvalidArgs", "no such page"));
        }
        let spec: Value = text
            .and_then(|v| v.get::<String>())
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or_else(|| fail("InvalidArgs", "the annotation must be a JSON object"))?;
        let (shape, style) = self.annotation_from(&spec)?;
        let id = {
            let mut store = self.markup.store.borrow_mut();
            store.pages.layer_mut(page).add(shape, style)
        };
        self.finish_markup(
            crate::annotate::Outcome {
                redraw: true,
                committed: true,
                ..Default::default()
            },
            sender,
        );
        Ok(json!({"id": id, "page": page}))
    }

    fn annotation_from(&self, spec: &Value) -> Result<(Shape, Style), Failure> {
        let num = |key: &str| spec.get(key).and_then(Value::as_f64);
        let need =
            |key: &str| num(key).ok_or_else(|| fail("InvalidArgs", format!("missing \"{key}\"")));
        let rect = || -> Result<Rect, Failure> {
            Ok(Rect::new(need("x")?, need("y")?, need("w")?, need("h")?))
        };
        let ends = || -> Result<(Point, Point), Failure> {
            Ok((
                Point::new(need("x1")?, need("y1")?),
                Point::new(need("x2")?, need("y2")?),
            ))
        };
        let points = |value: Option<&Value>| -> Option<Vec<Point>> {
            value?
                .as_array()?
                .iter()
                .map(|p| Some(Point::new(p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
                .collect()
        };
        let kind = spec.get("type").and_then(Value::as_str).unwrap_or_default();
        let editor = &self.markup.editor;
        let mut style = editor.style.clone();
        let shape = match kind {
            "rect" => Shape::Rect(rect()?),
            "ellipse" => Shape::Ellipse(rect()?),
            "line" => {
                let (a, b) = ends()?;
                Shape::Line(a, b)
            }
            "arrow" => {
                let (a, b) = ends()?;
                Shape::Arrow(a, b)
            }
            "pen" => Shape::Freehand(
                points(spec.get("points"))
                    .ok_or_else(|| fail("InvalidArgs", "\"points\" must be [[x, y], …]"))?,
            ),
            "ink" => Shape::Ink(
                spec.get("strokes")
                    .and_then(Value::as_array)
                    .and_then(|strokes| strokes.iter().map(|s| points(Some(s))).collect())
                    .ok_or_else(|| fail("InvalidArgs", "\"strokes\" must be [[[x, y], …], …]"))?,
            ),
            "text" => Shape::Text {
                bounds: rect()?,
                text: spec
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            "highlight" => {
                style.stroke = editor.highlight_color;
                style.fill = Some(editor.highlight_color);
                style.width = 0.0;
                Shape::Highlight(
                    spec.get("rects")
                        .and_then(Value::as_array)
                        .and_then(|rects| {
                            rects
                                .iter()
                                .map(|r| {
                                    Some(Rect::new(
                                        r.get(0)?.as_f64()?,
                                        r.get(1)?.as_f64()?,
                                        r.get(2)?.as_f64()?,
                                        r.get(3)?.as_f64()?,
                                    ))
                                })
                                .collect()
                        })
                        .ok_or_else(|| {
                            fail("InvalidArgs", "\"rects\" must be [[x, y, w, h], …]")
                        })?,
                )
            }
            other => {
                return Err(fail(
                    "InvalidArgs",
                    format!(
                        "unknown type \"{other}\" (rect, ellipse, line, arrow, pen, ink, text, highlight)"
                    ),
                ));
            }
        };
        if let Some(color) = spec.get("color").and_then(Value::as_str) {
            let color = parse_color(color)?;
            style.stroke = color;
            if kind == "highlight" {
                style.fill = Some(color);
            }
        }
        if let Some(fill) = spec.get("fill").and_then(Value::as_str) {
            style.fill = Some(parse_color(fill)?);
        }
        if let Some(width) = num("width") {
            style.width = width;
        }
        if let Some(size) = num("size") {
            style.font_size = size;
        }
        Ok((shape, style))
    }

    fn control_delete(
        &mut self,
        page: Option<glib::Variant>,
        id: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Value, Failure> {
        let page = page.and_then(|v| v.get::<u32>()).unwrap_or(0) as usize;
        let id = id.and_then(|v| v.get::<u64>()).unwrap_or(0);
        let exists = self
            .markup
            .store
            .borrow()
            .layer(page)
            .is_some_and(|l| l.get(id).is_some());
        if !exists {
            return Err(fail(
                "InvalidArgs",
                format!("no annotation {id} on page {page}"),
            ));
        }
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            store.selection = Some(crate::annotate::Selection { page, id });
            self.markup.editor.delete_selected(&mut store)
        };
        self.finish_markup(outcome, sender);
        Ok(self.state())
    }

    fn control_edit(
        &mut self,
        operation: Option<glib::Variant>,
        args: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Value, Failure> {
        if !self.can_edit() {
            return Err(fail(
                "NotSupported",
                "no picture that can be edited is open",
            ));
        }
        let args = Options::new(args);
        let int = |key: &str| {
            args.get::<u32>(key)
                .ok_or_else(|| fail("InvalidArgs", format!("missing \"{key}\" (u)")))
        };
        let op = match operation
            .and_then(|v| v.get::<String>())
            .unwrap_or_default()
            .as_str()
        {
            "rotate-right" => EditOp::RotateCw,
            "rotate-left" => EditOp::RotateCcw,
            "flip-horizontally" => EditOp::FlipH,
            "flip-vertically" => EditOp::FlipV,
            "crop" => EditOp::Crop(CropRect {
                x: int("x")?,
                y: int("y")?,
                w: int("w")?,
                h: int("h")?,
            }),
            "resize" => EditOp::Resize {
                w: int("w")?,
                h: int("h")?,
            },
            "adjust" => EditOp::Adjust(Adjustments {
                exposure: args.get::<f64>("exposure").unwrap_or(0.0) as f32,
                contrast: args.get::<f64>("contrast").unwrap_or(0.0) as f32,
                saturation: args.get::<f64>("saturation").unwrap_or(0.0) as f32,
            }),
            other => {
                return Err(fail(
                    "InvalidArgs",
                    format!(
                        "unknown operation \"{other}\" (rotate-right, rotate-left, flip-horizontally, flip-vertically, crop, resize, adjust)"
                    ),
                ));
            }
        };
        self.push_edit(op, sender);
        Ok(self.state())
    }

    fn control_pages(
        &mut self,
        id: u64,
        operation: Option<glib::Variant>,
        pages: Option<glib::Variant>,
        args: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Option<Value>, Failure> {
        self.busy()?;
        let Some(pdf) = &self.pdf else {
            return Err(fail("NotSupported", "no PDF is open"));
        };
        let n_pages = pdf.n_pages;
        let pages: Vec<usize> = pages
            .and_then(|v| v.get::<Vec<u32>>())
            .unwrap_or_default()
            .into_iter()
            .map(|p| p as usize)
            .collect();
        if pages.iter().any(|&p| p >= n_pages) {
            return Err(fail("InvalidArgs", "no such page"));
        }
        let args = Options::new(args);
        let request = match operation
            .and_then(|v| v.get::<String>())
            .unwrap_or_default()
            .as_str()
        {
            "rotate" => PageRequest::Rotate {
                pages,
                degrees: args.get::<i32>("degrees").unwrap_or(90),
            },
            "delete" if pages.len() >= n_pages => {
                return Err(fail("InvalidArgs", "a PDF must keep at least one page"));
            }
            "delete" => PageRequest::Delete(pages),
            "move" => PageRequest::Move {
                pages,
                before: args
                    .get::<u32>("before")
                    .ok_or_else(|| fail("InvalidArgs", "missing \"before\" (u)"))?
                    as usize,
            },
            "insert" => PageRequest::Insert {
                at: args.get::<u32>("at").unwrap_or(n_pages as u32) as usize,
                file: args
                    .get::<String>("path")
                    .map(PathBuf::from)
                    .ok_or_else(|| fail("InvalidArgs", "missing \"path\" (s)"))?,
            },
            other => {
                return Err(fail(
                    "InvalidArgs",
                    format!("unknown operation \"{other}\" (rotate, delete, move, insert)"),
                ));
            }
        };
        if !self.can_pages() {
            return Err(fail(
                "NotSupported",
                "the pages cannot be changed right now",
            ));
        }
        self.page_request(request, sender);
        self.wait(Wait::Reload, id)
    }

    fn control_history(
        &mut self,
        id: u64,
        undo: bool,
        sender: &ComponentSender<App>,
    ) -> Result<Option<Value>, Failure> {
        self.busy()?;
        let possible = if undo {
            self.can_undo()
        } else {
            self.can_redo()
        };
        if !possible {
            return Err(fail(
                "NotSupported",
                if undo {
                    "nothing to undo"
                } else {
                    "nothing to redo"
                },
            ));
        }
        if undo {
            self.undo(sender);
        } else {
            self.redo(sender);
        }
        // Going back across a page change reloads the PDF; answer once it is back.
        if self.page_busy {
            self.wait(Wait::Reload, id)
        } else {
            Ok(Some(self.state()))
        }
    }

    fn control_save(
        &mut self,
        id: u64,
        path: Option<glib::Variant>,
        options: Option<glib::Variant>,
        _sender: &ComponentSender<App>,
    ) -> Result<Option<Value>, Failure> {
        self.busy()?;
        let options = Options::new(options);
        let original = self
            .document_path()
            .ok_or_else(|| fail("NotSupported", "nothing that can be saved is open"))?;
        let path = path.and_then(|v| v.get::<String>()).unwrap_or_default();
        let dest = if path.is_empty() {
            original.clone()
        } else {
            PathBuf::from(path)
        };
        if dest.exists() && !options.flag("overwrite") {
            return Err(fail(
                "Exists",
                format!(
                    "{} exists; pass overwrite=true to replace it",
                    dest.display()
                ),
            ));
        }
        let format = if self.pdf.is_some() {
            if let Some(pdf) = &self.pdf
                && pdf.protected
                && pdf.source.path != pdf.original
            {
                if !options.flag("drop-password") {
                    return Err(fail(
                        "Unsafe",
                        "the pages of this protected PDF were changed, so it can only be saved without its password; pass drop-password=true",
                    ));
                }
                self.unprotected_ok = true;
            }
            Format::Png
        } else {
            if !self.can_edit() {
                return Err(fail("NotSupported", "this picture cannot be saved"));
            }
            match Format::from_path(&dest) {
                Some(Format::Jpeg { .. }) => Format::Jpeg {
                    quality: options.get::<u32>("quality").unwrap_or(90).clamp(1, 100) as u8,
                },
                Some(format) => format,
                None => return Err(fail("InvalidArgs", "save pictures as .jpg, .png or .webp")),
            }
        };
        self.start_save(dest, format, _sender);
        self.wait(Wait::Save, id)
    }

    fn control_export(
        &mut self,
        id: u64,
        kind: Option<glib::Variant>,
        path: Option<glib::Variant>,
        options: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Option<Value>, Failure> {
        self.busy()?;
        let options = Options::new(options);
        let dest = PathBuf::from(path.and_then(|v| v.get::<String>()).unwrap_or_default());
        if dest.as_os_str().is_empty() {
            return Err(fail("InvalidArgs", "no path given"));
        }
        if dest.exists() && !options.flag("overwrite") {
            return Err(fail(
                "Exists",
                format!(
                    "{} exists; pass overwrite=true to replace it",
                    dest.display()
                ),
            ));
        }
        match kind
            .and_then(|v| v.get::<String>())
            .unwrap_or_default()
            .as_str()
        {
            "pages" => {
                let pdf = self
                    .pdf
                    .as_ref()
                    .ok_or_else(|| fail("NotSupported", "no PDF is open"))?;
                let pages: Vec<usize> = options
                    .get::<Vec<u32>>("pages")
                    .map(|p| p.into_iter().map(|p| p as usize).collect())
                    .unwrap_or_else(|| (0..pdf.n_pages).collect());
                if pages.is_empty() || pages.iter().any(|&p| p >= pdf.n_pages) {
                    return Err(fail("InvalidArgs", "no such page"));
                }
                self.export_pages(dest, pages, sender);
            }
            "png" if self.pdf.is_some() => self.export_png(dest, sender),
            "pdf" if self.can_edit() => self.export_image_pdf(dest, sender),
            "print" => {
                if self.image.is_none() && self.pdf.is_none() {
                    return Err(fail("NotSupported", "nothing is open"));
                }
                // Printing to a PDF file is done at once.
                self.print(Some(dest));
                return Ok(Some(self.state()));
            }
            other => {
                return Err(fail(
                    "InvalidArgs",
                    format!(
                        "cannot export \"{other}\" here (pages and png for PDFs, pdf for pictures, print)"
                    ),
                ));
            }
        }
        self.wait(Wait::Export, id)
    }

    fn control_search(
        &mut self,
        id: u64,
        text: Option<glib::Variant>,
    ) -> Result<Option<Value>, Failure> {
        if self.pdf.is_none() {
            return Err(fail("NotSupported", "no PDF is open"));
        }
        let text = text.and_then(|v| v.get::<String>()).unwrap_or_default();
        self.search_open = !text.is_empty();
        self.search_text = text;
        self.restart_search();
        if self.searching {
            self.wait(Wait::Search, id)
        } else {
            Ok(Some(self.search_json()))
        }
    }

    /// Where the search found the text, once it has finished.
    pub(super) fn search_finished(&mut self) {
        let ready: Vec<u64> = self
            .control
            .waiting
            .iter()
            .filter(|(w, _)| *w == Wait::Search)
            .map(|(_, id)| *id)
            .collect();
        self.control.waiting.retain(|(w, _)| *w != Wait::Search);
        let result = self.search_json();
        for id in ready {
            answer(id, Ok(result.clone()));
        }
    }

    fn search_json(&self) -> Value {
        let hits: Vec<Value> = self
            .hits
            .iter()
            .flat_map(|(page, hits)| {
                hits.iter()
                    .map(move |h| json!({"page": page, "area": area_json(h)}))
            })
            .collect();
        json!({"text": self.search_text, "count": hits.len(), "hits": hits})
    }

    fn control_markup(
        &mut self,
        options: Option<glib::Variant>,
        sender: &ComponentSender<App>,
    ) -> Result<Value, Failure> {
        let options = Options::new(options);
        if let Some(open) = options.get::<bool>("open")
            && open != self.markup.open
        {
            if open && !self.can_markup() {
                return Err(fail(
                    "NotSupported",
                    "this document cannot be marked up right now",
                ));
            }
            self.toggle_markup();
        }
        if let Some(name) = options.get::<String>("tool") {
            let tool = tool_from(&name).ok_or_else(|| {
                fail(
                    "InvalidArgs",
                    "tool: select, rectangle, ellipse, arrow, line, pen, text or highlight",
                )
            })?;
            self.set_tool(tool);
        }
        if let Some(color) = options.get::<String>("color") {
            sender.input(AppMsg::SetColor(parse_color(&color)?));
        }
        if let Some(size) = options.get::<f64>("size") {
            sender.input(AppMsg::SetSize(size));
        }
        Ok(self.state())
    }

    fn control_screenshot(&self, path: Option<glib::Variant>) -> Result<Value, Failure> {
        let path = path.and_then(|v| v.get::<String>()).unwrap_or_default();
        let window = relm4::main_application()
            .active_window()
            .and_downcast::<adw::ApplicationWindow>()
            .ok_or_else(|| fail("Failed", "no window"))?;
        super::screenshot(&window, &path).map_err(|e| fail("Failed", e))?;
        Ok(self.state())
    }
}

fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "select",
        Tool::Rectangle => "rectangle",
        Tool::Ellipse => "ellipse",
        Tool::Arrow => "arrow",
        Tool::Line => "line",
        Tool::Pen => "pen",
        Tool::Text => "text",
        Tool::Highlight => "highlight",
    }
}

fn tool_from(name: &str) -> Option<Tool> {
    [
        Tool::Select,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Arrow,
        Tool::Line,
        Tool::Pen,
        Tool::Text,
        Tool::Highlight,
    ]
    .into_iter()
    .find(|t| tool_name(*t) == name)
}

fn parse_color(text: &str) -> Result<Color, Failure> {
    let c = relm4::gtk::gdk::RGBA::parse(text)
        .map_err(|_| fail("InvalidArgs", format!("not a colour: {text}")))?;
    Ok(Color::rgba(c.red(), c.green(), c.blue(), c.alpha()))
}

fn color_json(c: Color) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

fn area_json(a: &HitRect) -> Value {
    json!({"x": a.x1, "y": a.y1, "w": a.x2 - a.x1, "h": a.y2 - a.y1})
}

/// An annotation in the form `Annotate` takes, plus its id.
fn annotation_json(a: &Annotation) -> Value {
    let point = |p: &Point| json!([p.x, p.y]);
    let mut value = match &a.shape {
        Shape::Rect(r) => json!({"type": "rect", "x": r.x, "y": r.y, "w": r.w, "h": r.h}),
        Shape::Ellipse(r) => json!({"type": "ellipse", "x": r.x, "y": r.y, "w": r.w, "h": r.h}),
        Shape::Line(p, q) => json!({"type": "line", "x1": p.x, "y1": p.y, "x2": q.x, "y2": q.y}),
        Shape::Arrow(p, q) => json!({"type": "arrow", "x1": p.x, "y1": p.y, "x2": q.x, "y2": q.y}),
        Shape::Freehand(points) => {
            json!({"type": "pen", "points": points.iter().map(point).collect::<Vec<_>>()})
        }
        Shape::Ink(strokes) => {
            json!({"type": "ink", "strokes": strokes.iter().map(|s| s.iter().map(point).collect::<Vec<_>>()).collect::<Vec<_>>()})
        }
        Shape::Text { bounds: r, text } => {
            json!({"type": "text", "x": r.x, "y": r.y, "w": r.w, "h": r.h, "text": text, "size": a.style.font_size})
        }
        Shape::Highlight(rects) => {
            json!({"type": "highlight", "rects": rects.iter().map(|r| json!([r.x, r.y, r.w, r.h])).collect::<Vec<_>>()})
        }
    };
    value["id"] = json!(a.id);
    value["color"] = json!(color_json(a.style.stroke));
    value["width"] = json!(a.style.width);
    value
}
