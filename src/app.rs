use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke as EguiStroke};
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::commands::Command;
use crate::geometry;
use crate::model::{BlendMode, Document, Layer, LayerId, Stroke, StrokeId, StrokePoint};
use crate::render::{CanvasCallback, GpuRenderState};
use crate::selection::{self, StrokeRef};

const DOCUMENT_WIDTH: f32 = 1280.0;
const DOCUMENT_HEIGHT: f32 = 800.0;
const MAX_RECENT_PROJECTS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Brush,
    Select,
    Transform,
}

struct DrawingState {
    layer_id: LayerId,
    points: Vec<StrokePoint>,
    last_point: StrokePoint,
}

struct SelectionDrag {
    layer_id: LayerId,
    before: Vec<Stroke>,
    start: [f32; 2],
    last: [f32; 2],
    mode: DragMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragMode {
    Move,
    Scale,
    Marquee,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileDialog {
    None,
    SaveProject,
    ImportProject,
    ExportPng,
    ExportJpeg,
    ExportSvg,
}

pub struct InklineApp {
    document: Arc<RwLock<Document>>,
    render_state: Arc<Mutex<GpuRenderState>>,
    undo: Vec<Command>,
    redo: Vec<Command>,
    tool: Tool,
    brush_color: [f32; 4],
    brush_width: f32,
    drawing: Option<DrawingState>,
    selection_drag: Option<SelectionDrag>,
    selected: Vec<StrokeRef>,
    dirty_layers: HashSet<LayerId>,
    revision: u64,
    status: String,
    file_dialog: FileDialog,
    project_path: String,
    recent_projects: Vec<String>,
    export_path: String,
    export_width: u32,
    export_height: u32,
}

impl InklineApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("wgpu render state is required");

        let gpu = Arc::new(Mutex::new(GpuRenderState::new(
            render_state.device.clone(),
            render_state.queue.clone(),
            render_state.target_format,
        )));

        Self {
            document: Arc::new(RwLock::new(Document::new(DOCUMENT_WIDTH, DOCUMENT_HEIGHT))),
            render_state: gpu,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Brush,
            brush_color: [0.08, 0.12, 0.18, 1.0],
            brush_width: 1.0,
            drawing: None,
            selection_drag: None,
            selected: Vec::new(),
            dirty_layers: HashSet::new(),
            revision: 0,
            status: "Ready".to_owned(),
            file_dialog: FileDialog::None,
            project_path: String::new(),
            recent_projects: load_recent_projects().unwrap_or_default(),
            export_path: String::new(),
            export_width: 2560,
            export_height: 1600,
        }
    }

    fn document(&self) -> std::sync::RwLockReadGuard<'_, Document> {
        self.document.read().expect("document lock poisoned")
    }

    fn document_mut(&self) -> std::sync::RwLockWriteGuard<'_, Document> {
        self.document.write().expect("document lock poisoned")
    }

    fn allocate_stroke_id(&self) -> StrokeId {
        let mut document = self.document_mut();
        let id = document.next_stroke_id;
        document.next_stroke_id += 1;
        id
    }

    fn allocate_layer_id(&self) -> LayerId {
        let mut document = self.document_mut();
        let id = document.next_layer_id;
        document.next_layer_id += 1;
        id
    }

    fn execute(&mut self, command: Command) {
        if let Ok(mut document) = self.document.write() {
            command.apply(&mut document);
        }
        self.undo.push(command);
        self.redo.clear();
        self.after_command_change();
    }

    fn after_command_change(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty_layers.clear();
    }

    fn undo(&mut self) {
        let Some(command) = self.undo.pop() else {
            self.status = "Nothing to undo".to_owned();
            return;
        };
        if let Ok(mut document) = self.document.write() {
            command.revert(&mut document);
        }
        self.redo.push(command);
        self.after_command_change();
        self.selected.clear();
        self.status = "Undo".to_owned();
    }

    fn redo(&mut self) {
        let Some(command) = self.redo.pop() else {
            self.status = "Nothing to redo".to_owned();
            return;
        };
        if let Ok(mut document) = self.document.write() {
            command.apply(&mut document);
        }
        self.undo.push(command);
        self.after_command_change();
        self.selected.clear();
        self.status = "Redo".to_owned();
    }

    fn is_interacting(&self) -> bool {
        self.drawing.is_some() || self.selection_drag.is_some()
    }

    fn show_menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Save Project...").clicked() {
                        self.file_dialog = FileDialog::SaveProject;
                        ui.close();
                    }
                    if ui.button("Import Project...").clicked() {
                        self.file_dialog = FileDialog::ImportProject;
                        ui.close();
                    }

                    ui.separator();

                    let recent_projects = self.recent_projects.clone();
                    ui.menu_button("Recent Files", |ui| {
                        if recent_projects.is_empty() {
                            ui.add_enabled(false, egui::Button::new("No recent files"));
                        } else {
                            let mut clicked = false;
                            for path in &recent_projects {
                                let label = Path::new(path)
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .filter(|name| !name.is_empty())
                                    .unwrap_or(path.as_str());
                                if ui.button(label).on_hover_text(path).clicked() {
                                    self.open_recent(path);
                                    ui.close();
                                    clicked = true;
                                    break;
                                }
                            }

                            if !clicked {
                                ui.separator();
                                if ui.button("Clear Recent Files").clicked() {
                                    self.recent_projects.clear();
                                    save_recent_projects(&self.recent_projects);
                                    ui.close();
                                }
                            }
                        }
                    });

                    ui.separator();

                    if ui.button("Export PNG...").clicked() {
                        self.file_dialog = FileDialog::ExportPng;
                        ui.close();
                    }
                    if ui.button("Export JPEG...").clicked() {
                        self.file_dialog = FileDialog::ExportJpeg;
                        ui.close();
                    }
                    if ui.button("Export SVG...").clicked() {
                        self.file_dialog = FileDialog::ExportSvg;
                        ui.close();
                    }
                });

                ui.menu_button("Edit", |ui| {
                    if ui.button("Undo").clicked() {
                        self.undo();
                        ui.close();
                    }
                    if ui.button("Redo").clicked() {
                        self.redo();
                        ui.close();
                    }

                    ui.separator();

                    if ui
                        .add_enabled(
                            !self.selected.is_empty(),
                            egui::Button::new("Delete Selected"),
                        )
                        .clicked()
                    {
                        self.delete_selected();
                        ui.close();
                    }
                });
            });
        });
    }

    fn show_file_dialogs(&mut self, ctx: &egui::Context) {
        let dialog = self.file_dialog;
        if dialog == FileDialog::None {
            return;
        }

        let title = match dialog {
            FileDialog::SaveProject => "Save Project",
            FileDialog::ImportProject => "Import Project",
            FileDialog::ExportPng => "Export PNG",
            FileDialog::ExportJpeg => "Export JPEG",
            FileDialog::ExportSvg => "Export SVG",
            FileDialog::None => unreachable!(),
        };

        let mut open = true;
        let response = egui::Window::new(title)
            .id(egui::Id::new("file_dialog"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(390.0)
            .show(ctx, |ui| self.file_dialog_contents(ui, dialog));

        if !open {
            self.file_dialog = FileDialog::None;
        }

        if let Some(action) = response.and_then(|response| response.inner).flatten() {
            self.file_dialog = FileDialog::None;
            self.perform_file_action(action);
        }
    }

    fn file_dialog_contents(
        &mut self,
        ui: &mut egui::Ui,
        dialog: FileDialog,
    ) -> Option<FileDialog> {
        let is_project = matches!(dialog, FileDialog::SaveProject | FileDialog::ImportProject);
        let path = if is_project {
            &mut self.project_path
        } else {
            &mut self.export_path
        };

        ui.label(if is_project {
            "Project file"
        } else {
            "Output file"
        });
        ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
        ui.add_space(6.0);

        if matches!(dialog, FileDialog::ExportPng | FileDialog::ExportJpeg) {
            ui.horizontal(|ui| {
                ui.label("Width");
                ui.add(
                    egui::DragValue::new(&mut self.export_width)
                        .range(1..=16384)
                        .speed(10.0),
                );
                ui.label("Height");
                ui.add(
                    egui::DragValue::new(&mut self.export_height)
                        .range(1..=16384)
                        .speed(10.0),
                );
            });
            ui.add_space(6.0);
        }

        let confirm_label = match dialog {
            FileDialog::SaveProject => "Save",
            FileDialog::ImportProject => "Import",
            FileDialog::ExportPng | FileDialog::ExportJpeg | FileDialog::ExportSvg => "Export",
            FileDialog::None => "OK",
        };

        let mut cancel = false;
        let mut confirm = false;
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.button(confirm_label).clicked() {
                confirm = true;
            }
        });

        if cancel {
            self.file_dialog = FileDialog::None;
            None
        } else if confirm {
            Some(dialog)
        } else {
            None
        }
    }

    fn perform_file_action(&mut self, action: FileDialog) {
        match action {
            FileDialog::SaveProject => self.save_project(),
            FileDialog::ImportProject => self.load_project(),
            FileDialog::ExportPng => self.export_raster(true),
            FileDialog::ExportJpeg => self.export_raster(false),
            FileDialog::ExportSvg => self.export_svg(),
            FileDialog::None => {}
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool, Tool::Brush, "Brush");
                ui.selectable_value(&mut self.tool, Tool::Select, "Select");
                ui.selectable_value(&mut self.tool, Tool::Transform, "Transform");

                ui.separator();

                ui.label("Color");
                let mut color = Color32::from_rgba_unmultiplied(
                    (self.brush_color[0] * 255.0) as u8,
                    (self.brush_color[1] * 255.0) as u8,
                    (self.brush_color[2] * 255.0) as u8,
                    (self.brush_color[3] * 255.0) as u8,
                );
                if ui.color_edit_button_srgba(&mut color).changed() {
                    let values = color.to_array();
                    self.brush_color = [
                        values[0] as f32 / 255.0,
                        values[1] as f32 / 255.0,
                        values[2] as f32 / 255.0,
                        values[3] as f32 / 255.0,
                    ];
                    self.recolor_selected();
                }

                ui.label("Width");
                ui.add(
                    egui::Slider::new(&mut self.brush_width, 0.2..=6.0)
                        .logarithmic(true)
                        .fixed_decimals(2),
                );

                ui.separator();

                ui.label("Canvas");
                let mut background = Color32::from_rgba_unmultiplied(
                    (self.document().background_color[0] * 255.0) as u8,
                    (self.document().background_color[1] * 255.0) as u8,
                    (self.document().background_color[2] * 255.0) as u8,
                    (self.document().background_color[3] * 255.0) as u8,
                );
                if ui.color_edit_button_srgba(&mut background).changed() {
                    let values = background.to_array();
                    let before = self.document().background_color;
                    let after = [
                        values[0] as f32 / 255.0,
                        values[1] as f32 / 255.0,
                        values[2] as f32 / 255.0,
                        values[3] as f32 / 255.0,
                    ];
                    self.execute(Command::SetBackgroundColor { before, after });
                }

                ui.separator();
            });
            ui.horizontal(|ui| {
                ui.label(&self.status);
            });
        });
    }

    fn show_layer_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("layers")
            .exact_size(300.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Layers");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.document().layers.len() > 1,
                                egui::Button::new(egui::RichText::new("×").size(16.0)).small(),
                            )
                            .on_hover_text("Delete active layer")
                            .clicked()
                        {
                            self.delete_active_layer();
                        }
                        if ui
                            .add(egui::Button::new(egui::RichText::new("+").size(16.0)).small())
                            .on_hover_text("New layer")
                            .clicked()
                        {
                            self.add_layer();
                        }
                    });
                });

                ui.separator();

                let (mut active_id, layer_count, layers) = {
                    let document = self.document();
                    (
                        document.active_layer_id,
                        document.layers.len(),
                        document.layers.clone(),
                    )
                };
                let layer_list_height = (ui.available_height() - 190.0).max(80.0);

                egui::ScrollArea::vertical()
                    .max_height(layer_list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().spacing.item_spacing.x = 4.0;

                        for layer in layers.iter().cloned() {
                            let selected = active_id == layer.id;
                            let original = layer.clone();
                            let mut target = layer.clone();
                            let mut changed = false;

                            ui.horizontal(|ui| {
                                let eye = if target.visible { "●" } else { "○" };
                                if ui
                                    .add(
                                        egui::Button::new(egui::RichText::new(eye).size(15.0))
                                            .small()
                                            .frame(false),
                                    )
                                    .on_hover_text("Show/hide layer")
                                    .clicked()
                                {
                                    target.visible = !target.visible;
                                    changed = true;
                                }

                                if ui.selectable_label(selected, &target.name).clicked() {
                                    active_id = target.id;
                                    self.selected.clear();
                                }
                            });

                            if changed {
                                self.execute(Command::SetLayer {
                                    before: original,
                                    after: target,
                                });
                            }
                        }
                    });

                if active_id != self.document().active_layer_id {
                    if let Ok(mut document) = self.document.write() {
                        document.active_layer_id = active_id;
                    }
                }

                ui.separator();

                let (active_index, active_layer) = {
                    let document = self.document();
                    let Some(index) = document.layer_index(active_id) else {
                        return;
                    };
                    (index, document.layers[index].clone())
                };

                let original = active_layer.clone();
                let mut target = active_layer;
                let mut changed = false;

                ui.add_space(2.0);
                ui.label(egui::RichText::new("Layer Properties").strong());

                ui.horizontal(|ui| {
                    ui.label("Name");
                    let name_before = target.name.clone();
                    ui.add(egui::TextEdit::singleline(&mut target.name).desired_width(170.0));
                    if target.name != name_before {
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Opacity");
                    ui.add(
                        egui::Slider::new(&mut target.opacity, 0.0..=1.0)
                            .fixed_decimals(2)
                            .show_value(true),
                    );
                    if (target.opacity - original.opacity).abs() > f32::EPSILON {
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Blend");
                    egui::ComboBox::from_id_salt(("active_layer_blend", target.id))
                        .selected_text(target.blend_mode.label())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for mode in BlendMode::ALL {
                                ui.selectable_value(&mut target.blend_mode, mode, mode.label());
                            }
                        });
                    if target.blend_mode != original.blend_mode {
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            active_index > 0,
                            egui::Button::new(egui::RichText::new("▲").size(13.0)).small(),
                        )
                        .on_hover_text("Move layer up")
                        .clicked()
                    {
                        self.move_layer(active_index, active_index.saturating_sub(1));
                    }
                    if ui
                        .add_enabled(
                            active_index + 1 < layer_count,
                            egui::Button::new(egui::RichText::new("▼").size(13.0)).small(),
                        )
                        .on_hover_text("Move layer down")
                        .clicked()
                    {
                        self.move_layer(
                            active_index,
                            (active_index + 1).min(layer_count.saturating_sub(1)),
                        );
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.document().layers.len() > 1,
                            egui::Button::new("Delete").small(),
                        )
                        .clicked()
                    {
                        self.delete_active_layer();
                    }
                });

                if changed {
                    self.execute(Command::SetLayer {
                        before: original,
                        after: target,
                    });
                }
            });
    }

    fn show_canvas(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let available = ui.available_size();
            let width = available.x.max(1.0);
            let height = available.y.max(1.0);
            let (response, painter) =
                ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
            let rect = response.rect;

            let background = color_from_linear(self.document().background_color);
            painter.rect_filled(rect, 0.0, background);

            self.handle_canvas_input(ui, rect, &response);

            let callback = egui_wgpu::Callback::new_paint_callback(
                rect,
                CanvasCallback {
                    document: self.document.clone(),
                    render_state: self.render_state.clone(),
                    dirty_layers: self.dirty_layers.clone(),
                    revision: self.revision,
                },
            );
            painter.add(egui::Shape::Callback(callback));

            let doc = self.document();
            self.paint_overlay(&painter, rect, &doc);
        });
    }

    fn handle_canvas_input(&mut self, ui: &mut egui::Ui, rect: Rect, response: &egui::Response) {
        let Some(pointer_pos) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };

        if !rect.contains(pointer_pos) && !response.dragged() {
            return;
        }

        let mut doc_point = self.to_document(rect, pointer_pos);
        let (current_pressure, has_real_pressure) = pressure(ui);
        doc_point.pressure = current_pressure;
        let pressed = response.drag_started();
        let released = response.drag_stopped();
        let down = response.is_pointer_button_down_on();

        if pressed {
            match self.tool {
                Tool::Brush => self.start_drawing(doc_point, current_pressure, has_real_pressure),
                Tool::Select => self.start_selection([doc_point.x, doc_point.y]),
                Tool::Transform => self.start_transform([doc_point.x, doc_point.y]),
            }
        }

        if down {
            match self.tool {
                Tool::Brush => {
                    self.continue_drawing(doc_point, current_pressure, has_real_pressure)
                }
                Tool::Select => self.continue_selection([doc_point.x, doc_point.y]),
                Tool::Transform => self.continue_transform([doc_point.x, doc_point.y]),
            }
        }

        if released {
            match self.tool {
                Tool::Brush => self.finish_drawing(),
                Tool::Select => self.finish_selection(),
                Tool::Transform => self.finish_transform(),
            }
        }
    }

    fn start_drawing(&mut self, point: StrokePoint, _pressure: f32, _has_real_pressure: bool) {
        let layer_id = self.document().active_layer_id;
        self.selected.clear();
        self.drawing = Some(DrawingState {
            layer_id,
            points: Vec::new(),
            last_point: point,
        });
        self.push_drawing_point(point);
    }

    fn continue_drawing(&mut self, raw: StrokePoint, _pressure: f32, _has_real_pressure: bool) {
        let Some(drawing) = self.drawing.as_mut() else {
            return;
        };

        let dx = raw.x - drawing.last_point.x;
        let dy = raw.y - drawing.last_point.y;
        if dx * dx + dy * dy > 0.35 * 0.35 {
            drawing.points.push(raw);
            drawing.last_point = raw;
        }
    }

    fn push_drawing_point(&mut self, point: StrokePoint) {
        if let Some(drawing) = self.drawing.as_mut() {
            drawing.points.push(point);
            drawing.last_point = point;
        }
    }

    fn finish_drawing(&mut self) {
        let Some(mut drawing) = self.drawing.take() else {
            return;
        };

        if let Some(last) = drawing.points.last().copied() {
            if (last.x - drawing.last_point.x).abs() > f32::EPSILON
                || (last.y - drawing.last_point.y).abs() > f32::EPSILON
            {
                drawing.points.push(drawing.last_point);
            }
        }

        if drawing.points.len() < 2 {
            let Some(point) = drawing.points.first().copied() else {
                return;
            };
            let stroke = Stroke {
                id: self.allocate_stroke_id(),
                points: vec![point],
                color: self.brush_color,
                width_scale: self.brush_width,
            };
            let index = self
                .document()
                .layer(drawing.layer_id)
                .map(|layer| layer.strokes.len())
                .unwrap_or(0);
            self.execute(Command::AddStroke {
                layer_id: drawing.layer_id,
                stroke,
                index,
            });
            return;
        }

        let stroke = Stroke {
            id: self.allocate_stroke_id(),
            points: drawing.points,
            color: self.brush_color,
            width_scale: self.brush_width,
        };
        let index = self
            .document()
            .layer(drawing.layer_id)
            .map(|layer| layer.strokes.len())
            .unwrap_or(0);
        self.execute(Command::AddStroke {
            layer_id: drawing.layer_id,
            stroke,
            index,
        });
    }

    fn start_selection(&mut self, point: [f32; 2]) {
        let layer_id = self.document().active_layer_id;
        let hits = selection::hit_test(&self.document(), point, 8.0);
        let hits_on_active: Vec<_> = hits
            .into_iter()
            .filter(|item| item.layer_id == layer_id)
            .collect();

        if let Some(hit) = hits_on_active.first() {
            if !self.selected.iter().any(|item| item == hit) {
                self.selected.clear();
                self.selected.push(*hit);
            }
            let before = collect_strokes(&self.document(), &self.selected, layer_id);
            self.selection_drag = Some(SelectionDrag {
                layer_id,
                before,
                start: point,
                last: point,
                mode: DragMode::Move,
            });
        } else {
            self.selected.clear();
            self.selection_drag = Some(SelectionDrag {
                layer_id,
                before: Vec::new(),
                start: point,
                last: point,
                mode: DragMode::Marquee,
            });
        }
    }

    fn continue_selection(&mut self, point: [f32; 2]) {
        let Some(drag) = self.selection_drag.as_mut() else {
            return;
        };
        match drag.mode {
            DragMode::Move => {
                let dx = point[0] - drag.last[0];
                let dy = point[1] - drag.last[1];
                drag.last = point;
                let layer_id = drag.layer_id;
                if let Ok(mut document) = self.document.write() {
                    if let Some(layer) = document.layer_mut(layer_id) {
                        for stroke in &mut layer.strokes {
                            if self.selected.iter().any(|item| {
                                item.stroke_id == stroke.id && item.layer_id == layer_id
                            }) {
                                for point in &mut stroke.points {
                                    point.x += dx;
                                    point.y += dy;
                                }
                            }
                        }
                    }
                }
                self.dirty_layers.insert(layer_id);
                self.revision = self.revision.wrapping_add(1);
            }
            DragMode::Marquee => {
                drag.last = point;
            }
            DragMode::Scale => {}
        }
    }

    fn finish_selection(&mut self) {
        let Some(drag) = self.selection_drag.take() else {
            return;
        };
        match drag.mode {
            DragMode::Move => {
                let after = collect_strokes(&self.document(), &self.selected, drag.layer_id);
                if !drag.before.is_empty() && drag.before != after {
                    self.execute(Command::ModifyStrokes {
                        layer_id: drag.layer_id,
                        before: drag.before,
                        after,
                    });
                } else {
                    self.revision = self.revision.wrapping_add(1);
                    self.dirty_layers.clear();
                }
            }
            DragMode::Marquee => {
                let min = [
                    drag.start[0].min(drag.last[0]),
                    drag.start[1].min(drag.last[1]),
                ];
                let max = [
                    drag.start[0].max(drag.last[0]),
                    drag.start[1].max(drag.last[1]),
                ];
                let hits = selection::rect_select(&self.document(), min, max);
                self.selected = hits
                    .into_iter()
                    .filter(|item| item.layer_id == drag.layer_id)
                    .collect();
            }
            DragMode::Scale => {}
        }
    }

    fn start_transform(&mut self, point: [f32; 2]) {
        let layer_id = self.document().active_layer_id;
        if self.selected.is_empty() {
            self.status = "Select strokes first".to_owned();
            return;
        }
        let before = collect_strokes(&self.document(), &self.selected, layer_id);
        self.selection_drag = Some(SelectionDrag {
            layer_id,
            before,
            start: point,
            last: point,
            mode: DragMode::Scale,
        });
    }

    fn continue_transform(&mut self, point: [f32; 2]) {
        let Some(drag) = self.selection_drag.as_mut() else {
            return;
        };
        if drag.mode != DragMode::Scale {
            return;
        }

        let before = &drag.before;
        let Some(center) = strokes_center(before) else {
            return;
        };
        let start_distance =
            ((drag.start[0] - center[0]).powi(2) + (drag.start[1] - center[1]).powi(2)).sqrt();
        let current_distance =
            ((point[0] - center[0]).powi(2) + (point[1] - center[1]).powi(2)).sqrt();
        if start_distance < 1.0 {
            return;
        }
        let scale = (current_distance / start_distance).clamp(0.1, 8.0);
        let layer_id = drag.layer_id;

        if let Ok(mut document) = self.document.write() {
            if let Some(layer) = document.layer_mut(layer_id) {
                for stroke in &mut layer.strokes {
                    if self
                        .selected
                        .iter()
                        .any(|item| item.stroke_id == stroke.id && item.layer_id == layer_id)
                    {
                        scale_stroke(stroke, center, scale);
                    }
                }
            }
        }
        drag.last = point;
        self.dirty_layers.insert(layer_id);
        self.revision = self.revision.wrapping_add(1);
    }

    fn finish_transform(&mut self) {
        let Some(drag) = self.selection_drag.take() else {
            return;
        };
        let after = collect_strokes(&self.document(), &self.selected, drag.layer_id);
        if !drag.before.is_empty() && drag.before != after {
            self.execute(Command::ModifyStrokes {
                layer_id: drag.layer_id,
                before: drag.before,
                after,
            });
        } else {
            self.revision = self.revision.wrapping_add(1);
            self.dirty_layers.clear();
        }
    }

    fn recolor_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let layer_id = self.document().active_layer_id;
        let before = collect_strokes(&self.document(), &self.selected, layer_id);
        if before.is_empty() {
            return;
        }
        let after: Vec<_> = before
            .iter()
            .map(|stroke| {
                let mut stroke = stroke.clone();
                stroke.color = self.brush_color;
                stroke
            })
            .collect();
        self.execute(Command::ModifyStrokes {
            layer_id,
            before,
            after,
        });
    }

    fn add_layer(&mut self) {
        let id = self.allocate_layer_id();
        let (id, index, previous_active_layer_id, layer) = {
            let document = self.document();
            let index = document.layers.len();
            let previous_active_layer_id = document.active_layer_id;
            let layer = Layer::new(id, format!("Layer {}", document.layers.len() + 1));
            (id, index, previous_active_layer_id, layer)
        };
        self.execute(Command::AddLayer {
            layer,
            index,
            previous_active_layer_id,
            new_active_layer_id: id,
        });
        self.selected.clear();
    }

    fn delete_active_layer(&mut self) {
        if self.document().layers.len() <= 1 {
            self.status = "At least one layer is required".to_owned();
            return;
        }
        let (layer, index, previous_active_layer_id, new_active_layer_id) = {
            let document = self.document();
            let id = document.active_layer_id;
            let Some(index) = document.layer_index(id) else {
                return;
            };
            let layer = document.layers[index].clone();
            let previous_active_layer_id = id;
            let new_active_layer_id = if document.layers.len() > 1 {
                let candidate = (index + 1).min(document.layers.len() - 1);
                if candidate == document.layers.len() - 1 && candidate == index {
                    document
                        .layers
                        .get(index.saturating_sub(1))
                        .map(|item| item.id)
                } else {
                    document.layers.get(candidate).map(|item| item.id)
                }
                .unwrap_or(0)
            } else {
                0
            };
            (layer, index, previous_active_layer_id, new_active_layer_id)
        };
        self.execute(Command::RemoveLayer {
            layer,
            index,
            previous_active_layer_id,
            new_active_layer_id,
        });
        self.selected.clear();
    }

    fn move_layer(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let len = self.document().layers.len();
        if from >= len || to >= len {
            return;
        }
        self.execute(Command::ReorderLayer { from, to });
    }

    fn to_document(&self, rect: Rect, pos: Pos2) -> StrokePoint {
        let document = self.document();
        let x = ((pos.x - rect.min.x) / rect.width().max(1.0)) * document.width;
        let y = ((pos.y - rect.min.y) / rect.height().max(1.0)) * document.height;
        StrokePoint::new(x, y, 0.55)
    }

    fn to_screen(&self, rect: Rect, point: StrokePoint) -> Pos2 {
        let document = self.document();
        let x = rect.min.x + (point.x / document.width) * rect.width();
        let y = rect.min.y + (point.y / document.height) * rect.height();
        Pos2::new(x, y)
    }

    fn paint_overlay(&self, painter: &egui::Painter, rect: Rect, document: &Document) {
        if let Some(drawing) = &self.drawing {
            if drawing.points.len() >= 2 {
                let stroke = Stroke {
                    id: 0,
                    points: drawing.points.clone(),
                    color: self.brush_color,
                    width_scale: self.brush_width,
                };
                let geometry = geometry::tessellate_stroke_with_options(&stroke, false);
                self.paint_gpu_geometry(painter, rect, &geometry);
            } else if let Some(point) = drawing.points.first() {
                let pos = self.to_screen(rect, *point);
                let radius = geometry::width_for_pressure(
                    &Stroke {
                        id: 0,
                        points: drawing.points.clone(),
                        color: self.brush_color,
                        width_scale: self.brush_width,
                    },
                    point.pressure,
                ) / 2.0;
                painter.circle_filled(pos, radius.max(1.0), color_from_linear(self.brush_color));
            }
        }

        if let Some(drag) = &self.selection_drag {
            if drag.mode == DragMode::Marquee {
                let min = self.to_screen(
                    rect,
                    StrokePoint::new(
                        drag.start[0].min(drag.last[0]),
                        drag.start[1].min(drag.last[1]),
                        0.0,
                    ),
                );
                let max = self.to_screen(
                    rect,
                    StrokePoint::new(
                        drag.start[0].max(drag.last[0]),
                        drag.start[1].max(drag.last[1]),
                        0.0,
                    ),
                );
                painter.rect_stroke(
                    Rect::from_min_max(min, max),
                    0.0,
                    EguiStroke::new(1.5, Color32::from_rgb(45, 110, 210)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        for item in &self.selected {
            let Some(stroke) = document.find_stroke(item.layer_id, item.stroke_id) else {
                continue;
            };
            let Some(bounds) = stroke.bounds() else {
                continue;
            };
            let min = self.to_screen(
                rect,
                StrokePoint::new(bounds[0] - 8.0, bounds[1] - 8.0, 0.0),
            );
            let max = self.to_screen(
                rect,
                StrokePoint::new(bounds[2] + 8.0, bounds[3] + 8.0, 0.0),
            );
            painter.rect_stroke(
                Rect::from_min_max(min, max),
                0.0,
                EguiStroke::new(1.5, Color32::from_rgb(45, 110, 210)),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn paint_gpu_geometry(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        geometry: &geometry::GpuGeometry,
    ) {
        if geometry.vertices.is_empty() || geometry.indices.is_empty() {
            return;
        }

        let mut mesh = egui::Mesh::default();
        mesh.vertices.reserve(geometry.vertices.len());
        mesh.indices.reserve(geometry.indices.len());

        for vertex in &geometry.vertices {
            let position = self.to_screen(
                rect,
                StrokePoint::new(vertex.position[0], vertex.position[1], 0.0),
            );
            mesh.vertices.push(egui::epaint::Vertex::untextured(
                position,
                color_from_linear(vertex.color),
            ));
        }
        mesh.indices.extend_from_slice(&geometry.indices);
        painter.add(egui::Shape::mesh(mesh));
    }

    fn open_recent(&mut self, path: &str) {
        self.project_path = path.to_owned();
        self.load_project();
    }

    fn remember_project(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }

        self.recent_projects
            .retain(|recent_path| recent_path != path);
        self.recent_projects.insert(0, path.to_owned());
        self.recent_projects.truncate(MAX_RECENT_PROJECTS);
        save_recent_projects(&self.recent_projects);
    }

    fn save_project(&mut self) {
        let path = self.project_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Project path is empty".to_owned();
            return;
        }
        let document = self.document();
        let result = save_document(Path::new(&path), &document);
        drop(document);
        match result {
            Ok(()) => {
                self.status = format!("Saved {path}");
                self.remember_project(&path);
            }
            Err(error) => self.status = format!("Save failed: {error}"),
        }
    }

    fn load_project(&mut self) {
        let path = self.project_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Project path is empty".to_owned();
            return;
        }
        match load_document(Path::new(&path)) {
            Ok(document) => {
                if let Ok(mut current) = self.document.write() {
                    *current = document;
                }
                self.undo.clear();
                self.redo.clear();
                self.selected.clear();
                self.dirty_layers.clear();
                self.revision = self.revision.wrapping_add(1);
                self.status = format!("Loaded {path}");
                self.remember_project(&path);
            }
            Err(error) => self.status = format!("Load failed: {error}"),
        }
    }

    fn export_svg(&mut self) {
        let path = self.export_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Export path is empty".to_owned();
            return;
        }
        let document = self.document();
        let result = std::fs::write(Path::new(&path), svg_document(&document));
        drop(document);
        self.status = match result {
            Ok(()) => format!("Exported {path}"),
            Err(error) => format!("SVG export failed: {error}"),
        };
    }

    fn export_raster(&mut self, png: bool) {
        let path = self.export_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Export path is empty".to_owned();
            return;
        }
        let width = self.export_width.max(1);
        let height = self.export_height.max(1);
        match self.render_raster(width, height, png) {
            Ok(()) => self.status = format!("Exported {path}"),
            Err(error) => self.status = format!("Raster export failed: {error}"),
        }
    }

    fn render_raster(&mut self, width: u32, height: u32, png: bool) -> Result<(), String> {
        let path = self.export_path.trim().to_owned();
        let document = self.document();
        let mut render_state = self
            .render_state
            .lock()
            .map_err(|_| "render state poisoned")?;
        let rgba =
            crate::render::render_document_to_rgba(&mut render_state, &document, width, height)
                .map_err(|error| error.to_string())?;

        let image = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| "invalid image buffer".to_owned())?;
        if png {
            image
                .save(Path::new(&path))
                .map_err(|error| error.to_string())?;
        } else {
            image
                .save_with_format(Path::new(&path), image::ImageFormat::Jpeg)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl eframe::App for InklineApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_interacting() {
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ui);
        self.show_menu_bar(ui);
        self.show_toolbar(ui);
        self.show_layer_panel(ui);
        self.show_canvas(ui);
        self.show_file_dialogs(ui.ctx());
    }
}

impl InklineApp {
    fn handle_shortcuts(&mut self, ui: &mut egui::Ui) {
        if self.file_dialog != FileDialog::None {
            return;
        }

        let ctx = ui.ctx().clone();
        let undo = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
        let redo = ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            ) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        let delete = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
        });
        if delete && !self.selected.is_empty() {
            self.delete_selected();
        }
    }

    fn delete_selected(&mut self) {
        let layer_id = self.document().active_layer_id;
        let strokes = collect_strokes(&self.document(), &self.selected, layer_id);
        if strokes.is_empty() {
            return;
        }
        let indices = stroke_indices(&self.document(), &self.selected, layer_id);
        self.execute(Command::RemoveStrokes {
            layer_id,
            strokes,
            indices,
        });
        self.selected.clear();
    }
}

fn pressure(ui: &egui::Ui) -> (f32, bool) {
    let pressure = ui.input(|i| {
        i.events.iter().find_map(|event| match event {
            egui::Event::Touch { force, .. } => Some(force.unwrap_or(0.55)),
            _ => None,
        })
    });

    match pressure {
        Some(force) => (force.clamp(0.0, 1.0), true),
        None => (0.55, false),
    }
}

fn collect_strokes(document: &Document, selected: &[StrokeRef], layer_id: LayerId) -> Vec<Stroke> {
    selected
        .iter()
        .filter(|item| item.layer_id == layer_id)
        .filter_map(|item| document.find_stroke(item.layer_id, item.stroke_id).cloned())
        .collect()
}

fn stroke_indices(document: &Document, selected: &[StrokeRef], layer_id: LayerId) -> Vec<usize> {
    let mut indices = Vec::new();
    if let Some(layer) = document.layer(layer_id) {
        for (index, stroke) in layer.strokes.iter().enumerate() {
            if selected
                .iter()
                .any(|item| item.layer_id == layer_id && item.stroke_id == stroke.id)
            {
                indices.push(index);
            }
        }
    }
    indices.sort_unstable_by(|a, b| b.cmp(a));
    indices
}

fn strokes_center(strokes: &[Stroke]) -> Option<[f32; 2]> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for stroke in strokes {
        if let Some(bounds) = stroke.bounds() {
            min_x = min_x.min(bounds[0]);
            min_y = min_y.min(bounds[1]);
            max_x = max_x.max(bounds[2]);
            max_y = max_y.max(bounds[3]);
        }
    }
    if min_x.is_finite() {
        Some([(min_x + max_x) / 2.0, (min_y + max_y) / 2.0])
    } else {
        None
    }
}

fn scale_stroke(stroke: &mut Stroke, center: [f32; 2], scale: f32) {
    for point in &mut stroke.points {
        point.x = center[0] + (point.x - center[0]) * scale;
        point.y = center[1] + (point.y - center[1]) * scale;
    }
}

fn color_from_linear(color: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn recent_projects_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));

    base.map(|base| base.join("inkline").join("recent_projects.bin"))
}

fn load_recent_projects() -> Option<Vec<String>> {
    let bytes = std::fs::read(recent_projects_path()?).ok()?;
    bincode::deserialize(&bytes).ok()
}

fn save_recent_projects(projects: &[String]) {
    let Some(path) = recent_projects_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = bincode::serialize(projects) {
        let _ = std::fs::write(path, bytes);
    }
}

fn save_document(path: &Path, document: &Document) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = bincode::serialize(document)?;
    let file = std::fs::File::create(path)?;
    let mut encoder = ZlibEncoder::new(file, Compression::default());
    encoder.write_all(&bytes)?;
    encoder.finish()?;
    Ok(())
}

fn load_document(path: &Path) -> Result<Document, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut decoder = flate2::read::ZlibDecoder::new(file);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut bytes)?;
    let document = bincode::deserialize(&bytes)?;
    Ok(document)
}

fn svg_document(document: &Document) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
        document.width, document.height, document.width, document.height
    ));
    out.push('\n');
    let [r, g, b, a] = document.background_color;
    out.push_str(&format!(
        r#"<rect width="{}" height="{}" fill="rgb({},{},{})" opacity="{:.3}"/>"#,
        document.width,
        document.height,
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
        a.clamp(0.0, 1.0)
    ));
    out.push('\n');
    for layer in &document.layers {
        if !layer.visible {
            continue;
        }
        for stroke in &layer.strokes {
            let path = geometry::stroke_svg_path(stroke);
            let [r, g, b, a] = stroke.color;
            let stroke_opacity = (a * layer.opacity).clamp(0.0, 1.0);
            out.push_str(&format!(
                r#"<path d="{}" fill="rgb({},{},{})" fill-rule="nonzero" opacity="{:.3}"/>"#,
                path,
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
                stroke_opacity
            ));
            out.push('\n');
        }
    }
    out.push_str("</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Layer, StrokePoint};

    fn sample_document() -> Document {
        let mut document = Document::new(200.0, 120.0);
        document.layers[0].strokes.push(Stroke {
            id: 5,
            points: vec![
                StrokePoint::new(10.0, 10.0, 1.0),
                StrokePoint::new(60.0, 70.0, 0.5),
            ],
            color: [0.1, 0.2, 0.3, 1.0],
            width_scale: 2.0,
        });
        document
    }

    #[test]
    fn binary_document_round_trip_preserves_strokes() {
        let document = sample_document();
        let path = std::env::temp_dir().join(format!(
            "inkline-{}-project.bin",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        save_document(&path, &document).unwrap();
        let loaded = load_document(&path).unwrap();
        let _ = std::fs::remove_file(path);

        assert_eq!(loaded, document);
    }

    #[test]
    fn svg_export_contains_path_and_layer_opacity() {
        let mut document = sample_document();
        document.background_color = [0.2, 0.4, 0.6, 1.0];
        document.layers.push(Layer::new(2, "Hidden"));
        let svg = svg_document(&document);

        assert!(svg.contains("<svg"));
        assert!(svg.contains("<rect width=\"200\" height=\"120\""));
        assert!(svg.contains("fill=\"rgb(51,102,153)\""));
        assert!(svg.contains("<path d=\""));
        assert!(svg.contains("fill-rule=\"nonzero\""));
        assert!(!svg.contains("Hidden"));
    }
}
