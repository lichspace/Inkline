use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke as EguiStroke};
use flate2::write::ZlibEncoder;
use flate2::Compression;
#[cfg(not(target_os = "ios"))]
use rfd::FileDialog as RfdFileDialog;

use crate::commands::Command;
use crate::geometry;
use crate::model::{BlendMode, Document, Layer, LayerId, Stroke, StrokeId, StrokePoint};
use crate::render::{CanvasCallback, GpuRenderState};
use crate::selection::{self, StrokeRef};

const DOCUMENT_WIDTH: f32 = 1280.0;
const DOCUMENT_HEIGHT: f32 = 800.0;
const MAX_RECENT_PROJECTS: usize = 10;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_CHROME: Color32 = Color32::from_rgb(24, 28, 35);
const APP_CHROME_RAISED: Color32 = Color32::from_rgb(31, 36, 45);
const APP_BORDER: Color32 = Color32::from_rgb(54, 62, 75);
const APP_ACCENT: Color32 = Color32::from_rgb(75, 132, 255);
const WORKSPACE_BACKGROUND: Color32 = Color32::from_rgb(42, 46, 54);

#[cfg(target_os = "ios")]
fn ios_safe_area_top() -> f32 {
    extern "C" {
        fn inkline_safe_area_top() -> f64;
    }

    // The Xcode launcher provides this on iOS, and eframe runs UI code on the
    // main thread where UIKit's safe-area values may be queried.
    unsafe { inkline_safe_area_top() as f32 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Brush,
    Select,
    Pan,
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
    renaming_layer_id: Option<LayerId>,
    renaming_layer_name: String,
    rename_focus_requested: bool,
    dirty_layers: HashSet<LayerId>,
    revision: u64,
    saved_revision: u64,
    status: String,
    file_dialog: FileDialog,
    project_path: String,
    recent_projects: Vec<String>,
    export_path: String,
    export_width: u32,
    export_height: u32,
    close_prompt_open: bool,
    allow_close: bool,
    layers_open: bool,
    layers_preference_explicit: bool,
    canvas_scale: Option<f32>,
    canvas_pan: egui::Vec2,
    canvas_fit_frames_remaining: u8,
    size_new_document_to_workspace: bool,
}

impl InklineApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_ui_style(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let saved_layers_open = load_layers_open();

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
            renaming_layer_id: None,
            renaming_layer_name: String::new(),
            rename_focus_requested: false,
            dirty_layers: HashSet::new(),
            revision: 0,
            saved_revision: 0,
            status: "Ready".to_owned(),
            file_dialog: FileDialog::None,
            project_path: default_action_path(FileDialog::SaveProject),
            recent_projects: load_recent_projects().unwrap_or_default(),
            export_path: default_action_path(FileDialog::ExportPng),
            export_width: 2560,
            export_height: 1600,
            close_prompt_open: false,
            allow_close: false,
            layers_open: saved_layers_open.unwrap_or(true),
            layers_preference_explicit: saved_layers_open.is_some(),
            canvas_scale: None,
            canvas_pan: egui::Vec2::ZERO,
            canvas_fit_frames_remaining: 8,
            size_new_document_to_workspace: true,
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

    fn finish_renaming(&mut self) {
        if let Some(layer_id) = self.renaming_layer_id {
            self.commit_rename(layer_id);
        }
    }

    fn commit_rename(&mut self, layer_id: LayerId) {
        if self.renaming_layer_id != Some(layer_id) {
            return;
        }

        self.renaming_layer_id = None;
        let new_name = std::mem::take(&mut self.renaming_layer_name);
        if new_name.trim().is_empty() {
            return;
        }

        let before = match self.document().layer(layer_id) {
            Some(layer) => layer.clone(),
            None => return,
        };
        if before.name == new_name {
            return;
        }

        let mut after = before.clone();
        after.name = new_name;
        self.execute(Command::SetLayer { before, after });
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

    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn set_layers_open(&mut self, open: bool) {
        if self.layers_open == open && self.layers_preference_explicit {
            return;
        }

        self.layers_open = open;
        self.layers_preference_explicit = true;
        save_layers_open(open);
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && self.is_dirty() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_prompt_open = true;
        }
    }

    fn update_window_title(&self, ctx: &egui::Context) {
        let file_name = file_name_or_default(self.project_path.trim(), "Untitled");
        let dirty = if self.is_dirty() { "*" } else { "" };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "Inkline {APP_VERSION} - {file_name}{dirty}"
        )));
    }

    fn show_menu_bar(&mut self, ui: &mut egui::Ui) {
        let margin = if cfg!(target_os = "ios") {
            egui::Margin::symmetric(10, 4)
        } else {
            egui::Margin::symmetric(6, 1)
        };
        egui::Panel::top("menu_bar")
            .frame(
                egui::Frame::NONE
                    .fill(APP_CHROME)
                    .inner_margin(margin)
                    .stroke(EguiStroke::new(1.0, APP_BORDER)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Save Project...").clicked() {
                            ui.close();
                            self.begin_file_dialog(FileDialog::SaveProject);
                        }
                        if ui.button("Open Project...").clicked() {
                            ui.close();
                            self.begin_file_dialog(FileDialog::ImportProject);
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
                            self.begin_file_dialog(FileDialog::ExportPng);
                            ui.close();
                        }
                        if ui.button("Export JPEG...").clicked() {
                            self.begin_file_dialog(FileDialog::ExportJpeg);
                            ui.close();
                        }
                        if ui.button("Export SVG...").clicked() {
                            self.begin_file_dialog(FileDialog::ExportSvg);
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

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&self.status)
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .truncate(),
                        );
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
            FileDialog::ImportProject => "Open Project",
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

        #[cfg(target_os = "ios")]
        {
            ui.label(if is_project {
                "Project file name"
            } else {
                "Output file name"
            });
            let mut file_name = file_name_or_default(path, default_file_name(dialog));
            if ui
                .add(egui::TextEdit::singleline(&mut file_name).desired_width(f32::INFINITY))
                .changed()
            {
                *path = ios_action_path(dialog, &file_name)
                    .to_string_lossy()
                    .into_owned();
            }
            ui.small("Files → On My iPhone/iPad → Inkline");

            if dialog == FileDialog::ImportProject {
                let projects = ios_project_files();
                if !projects.is_empty() {
                    ui.separator();
                    ui.label("Projects on this device");
                    egui::ScrollArea::vertical()
                        .max_height(132.0)
                        .show(ui, |ui| {
                            for project in projects {
                                let label = file_name_or_default(
                                    &project.to_string_lossy(),
                                    "Untitled.inkline",
                                );
                                if ui.button(label).clicked() {
                                    *path = project.to_string_lossy().into_owned();
                                }
                            }
                        });
                }
            }
        }

        #[cfg(not(target_os = "ios"))]
        {
            ui.label(if is_project {
                "Project file"
            } else {
                "Output file"
            });
            let mut browse = false;
            ui.horizontal(|ui| {
                let edit_width = (ui.available_width() - 88.0).max(80.0);
                ui.add(egui::TextEdit::singleline(path).desired_width(edit_width));
                if ui.button("Browse...").clicked() {
                    browse = true;
                }
            });
            if browse {
                let current_path = path.clone();
                let starting_directory = Path::new(&current_path)
                    .parent()
                    .filter(|directory| directory.exists())
                    .map(|directory| directory.to_path_buf());
                let selected = choose_file_dialog(dialog, &current_path, starting_directory);
                if let Some(selected) = selected {
                    *path = selected;
                }
            }
        }
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
            FileDialog::ImportProject => "Open",
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
        #[cfg(target_os = "ios")]
        {
            let path = if matches!(action, FileDialog::SaveProject | FileDialog::ImportProject) {
                &mut self.project_path
            } else {
                &mut self.export_path
            };
            *path = ios_action_path(action, path).to_string_lossy().into_owned();
        }

        match action {
            FileDialog::SaveProject => self.save_project(),
            FileDialog::ImportProject => self.load_project(),
            FileDialog::ExportPng => self.export_raster(true),
            FileDialog::ExportJpeg => self.export_raster(false),
            FileDialog::ExportSvg => self.export_svg(),
            FileDialog::None => {}
        }
    }

    fn begin_file_dialog(&mut self, dialog: FileDialog) {
        #[cfg(not(target_os = "ios"))]
        if matches!(dialog, FileDialog::SaveProject | FileDialog::ImportProject) {
            if let Some(path) = self.choose_project_file(dialog) {
                self.project_path = path;
                self.perform_file_action(dialog);
            }
            return;
        }

        #[cfg(target_os = "ios")]
        {
            let path = if matches!(dialog, FileDialog::SaveProject | FileDialog::ImportProject) {
                &mut self.project_path
            } else {
                &mut self.export_path
            };
            *path = ios_action_path(dialog, path).to_string_lossy().into_owned();
        }

        self.file_dialog = dialog;
    }

    #[cfg(not(target_os = "ios"))]
    fn choose_project_file(&self, dialog: FileDialog) -> Option<String> {
        let current_path = self.project_path.clone();
        let starting_directory = Path::new(&current_path)
            .parent()
            .filter(|directory| directory.exists())
            .map(|directory| directory.to_path_buf());
        choose_file_dialog(dialog, &current_path, starting_directory)
    }

    fn save_project_or_choose(&mut self) {
        if self.project_path.trim().is_empty() {
            #[cfg(target_os = "ios")]
            {
                self.begin_file_dialog(FileDialog::SaveProject);
                return;
            }

            #[cfg(not(target_os = "ios"))]
            if let Some(path) = self.choose_project_file(FileDialog::SaveProject) {
                self.project_path = path;
                self.save_project();
            }
        } else {
            self.save_project();
        }
    }

    fn show_close_prompt(&mut self, ctx: &egui::Context) {
        if !self.close_prompt_open {
            return;
        }

        let mut open = self.close_prompt_open;
        let mut save_requested = false;
        let mut discard_requested = false;
        let ctx = ctx.clone();

        egui::Window::new("Unsaved Changes")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(&ctx, |ui| {
                ui.label("Save changes before closing?");
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save_requested = true;
                    }
                    if ui.button("Discard").clicked() {
                        discard_requested = true;
                    }
                });
            });

        self.close_prompt_open = open;

        if save_requested {
            if self.project_path.trim().is_empty() {
                #[cfg(target_os = "ios")]
                {
                    self.close_prompt_open = false;
                    self.begin_file_dialog(FileDialog::SaveProject);
                    return;
                }

                #[cfg(not(target_os = "ios"))]
                {
                    let Some(path) = self.choose_project_file(FileDialog::SaveProject) else {
                        return;
                    };
                    self.project_path = path;
                }
            }

            self.save_project();
            if !self.is_dirty() {
                self.close_prompt_open = false;
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        if discard_requested {
            self.close_prompt_open = false;
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui, stacked: bool) {
        let touch_sized = cfg!(target_os = "ios");
        let margin = if touch_sized {
            egui::Margin::symmetric(5, 3)
        } else {
            egui::Margin::symmetric(6, 3)
        };
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(APP_CHROME_RAISED)
                    .inner_margin(margin)
                    .stroke(EguiStroke::new(1.0, APP_BORDER)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.spacing_mut().button_padding = egui::vec2(5.0, 2.0);
                if touch_sized {
                    // Keep the iOS toolbar visually dense without changing
                    // the roomier controls used by dialogs and layer editing.
                    ui.spacing_mut().interact_size = egui::vec2(34.0, 34.0);
                }
                if stacked {
                    ui.horizontal(|ui| self.show_primary_toolbar_controls(ui, touch_sized));
                    ui.add_space(1.0);
                    ui.separator();
                    ui.add_space(1.0);
                    ui.horizontal(|ui| self.show_paint_controls(ui, true));
                } else {
                    ui.horizontal(|ui| {
                        self.show_primary_toolbar_controls(ui, touch_sized);
                        ui.separator();
                        self.show_paint_controls(ui, false);
                    });
                }
            });
    }

    fn show_primary_toolbar_controls(&mut self, ui: &mut egui::Ui, touch_sized: bool) {
        let button_size = if touch_sized {
            egui::vec2(34.0, 34.0)
        } else {
            egui::vec2(26.0, 24.0)
        };
        let icon_size = 16.0;

        if ui
            .add(
                egui::Button::new(svg_icon(
                    egui::include_image!("../assets/icons/brush.svg"),
                    "Brush",
                    icon_size,
                ))
                .selected(self.tool == Tool::Brush)
                .image_tint_follows_text_color(true)
                .min_size(button_size),
            )
            .on_hover_text("Brush")
            .clicked()
        {
            self.tool = Tool::Brush;
        }
        if ui
            .add(
                egui::Button::new(svg_icon(
                    egui::include_image!("../assets/icons/select.svg"),
                    "Select",
                    icon_size,
                ))
                .selected(self.tool == Tool::Select)
                .image_tint_follows_text_color(true)
                .min_size(button_size),
            )
            .on_hover_text("Select and move")
            .clicked()
        {
            self.tool = Tool::Select;
        }
        if ui
            .add(
                egui::Button::new(svg_icon(
                    egui::include_image!("../assets/icons/hand.svg"),
                    "Pan",
                    icon_size,
                ))
                .selected(self.tool == Tool::Pan)
                .image_tint_follows_text_color(true)
                .min_size(button_size),
            )
            .on_hover_text("Pan canvas (hold Space)")
            .clicked()
        {
            self.tool = Tool::Pan;
        }

        if ui
            .add_enabled(
                !self.undo.is_empty(),
                egui::Button::new(svg_icon(
                    egui::include_image!("../assets/icons/undo.svg"),
                    "Undo",
                    icon_size,
                ))
                .image_tint_follows_text_color(true)
                .min_size(button_size),
            )
            .on_hover_text("Undo")
            .clicked()
        {
            self.undo();
        }
        if ui
            .add_enabled(
                !self.redo.is_empty(),
                egui::Button::new(svg_icon(
                    egui::include_image!("../assets/icons/redo.svg"),
                    "Redo",
                    icon_size,
                ))
                .image_tint_follows_text_color(true)
                .min_size(button_size),
            )
            .on_hover_text("Redo")
            .clicked()
        {
            self.redo();
        }

        if ui
            .add(
                egui::Button::new(svg_icon(
                    egui::include_image!("../assets/icons/layers.svg"),
                    "Layers",
                    icon_size,
                ))
                .selected(self.layers_open)
                .image_tint_follows_text_color(true)
                .min_size(button_size),
            )
            .on_hover_text("Show or hide layers")
            .clicked()
        {
            self.set_layers_open(!self.layers_open);
        }
    }

    fn show_paint_controls(&mut self, ui: &mut egui::Ui, stacked: bool) {
        let icon_size = 16.0;
        let background_icon_size = if cfg!(target_os = "ios") { 22.0 } else { 20.0 };
        let icon_color = ui.visuals().weak_text_color();
        ui.add(
            svg_icon(
                egui::include_image!("../assets/icons/ink.svg"),
                "Ink color",
                icon_size,
            )
            .tint(icon_color),
        )
        .on_hover_text("Ink color");
        let mut color = Color32::from_rgba_unmultiplied(
            (self.brush_color[0] * 255.0) as u8,
            (self.brush_color[1] * 255.0) as u8,
            (self.brush_color[2] * 255.0) as u8,
            (self.brush_color[3] * 255.0) as u8,
        );
        if compact_color_edit_button_srgba(ui, &mut color).changed() {
            let values = color.to_array();
            self.brush_color = [
                values[0] as f32 / 255.0,
                values[1] as f32 / 255.0,
                values[2] as f32 / 255.0,
                values[3] as f32 / 255.0,
            ];
            self.recolor_selected();
        }

        ui.add(
            svg_icon(
                egui::include_image!("../assets/icons/width.svg"),
                "Brush width",
                icon_size,
            )
            .tint(icon_color),
        )
        .on_hover_text("Brush width");
        ui.spacing_mut().slider_width = if stacked { 76.0 } else { 96.0 };
        ui.add(
            egui::Slider::new(&mut self.brush_width, 0.2..=6.0)
                .logarithmic(true)
                .fixed_decimals(2)
                .show_value(false),
        )
        .on_hover_text(format!("Brush width: {:.2}", self.brush_width));

        if !stacked {
            ui.separator();
        }
        ui.add(
            svg_icon(
                egui::include_image!("../assets/icons/canvas.svg"),
                "Canvas color",
                background_icon_size,
            )
            .tint(icon_color),
        )
        .on_hover_text("Canvas color");
        let before = self.document().background_color;
        let mut background = Color32::from_rgba_unmultiplied(
            (before[0] * 255.0) as u8,
            (before[1] * 255.0) as u8,
            (before[2] * 255.0) as u8,
            (before[3] * 255.0) as u8,
        );
        if compact_color_edit_button_srgba(ui, &mut background).changed() {
            let values = background.to_array();
            let after = [
                values[0] as f32 / 255.0,
                values[1] as f32 / 255.0,
                values[2] as f32 / 255.0,
                values[3] as f32 / 255.0,
            ];
            self.execute(Command::SetBackgroundColor { before, after });
        }
    }

    fn show_layer_panel(&mut self, ui: &mut egui::Ui) {
        let mut close_requested = false;
        egui::Panel::left("layers")
            .default_size(200.0)
            .min_size(180.0)
            .max_size(260.0)
            .resizable(true)
            .frame(
                egui::Frame::NONE
                    .fill(APP_CHROME)
                    .inner_margin(egui::Margin::same(6))
                    .stroke(EguiStroke::new(1.0, APP_BORDER)),
            )
            .show(ui, |ui| {
                close_requested = self.show_layer_contents(ui, true);
            });
        if close_requested {
            self.set_layers_open(false);
        }
    }

    fn show_layer_window(&mut self, ctx: &egui::Context) {
        if !self.layers_open {
            return;
        }

        let mut open = self.layers_open;
        egui::Window::new("Layers")
            .id(egui::Id::new("compact_layers"))
            .open(&mut open)
            .default_width(300.0)
            .default_height(420.0)
            .resizable(true)
            .show(ctx, |ui| {
                self.show_layer_contents(ui, false);
            });
        if open != self.layers_open {
            self.set_layers_open(open);
        }
    }

    fn show_layer_contents(&mut self, ui: &mut egui::Ui, show_close_button: bool) -> bool {
        let mut close_requested = false;
        ui.horizontal(|ui| {
            if show_close_button {
                ui.heading(egui::RichText::new("Layers").size(17.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if show_close_button
                    && ui
                        .add(egui::Button::new("Hide").min_size(egui::vec2(40.0, 26.0)))
                        .on_hover_text("Hide layers")
                        .clicked()
                {
                    close_requested = true;
                }
                if ui
                    .add_enabled(
                        self.document().layers.len() > 1,
                        egui::Button::new(egui::RichText::new("−").size(16.0)).small(),
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
                        let eye_color = if target.visible {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        let mut eye_text = egui::RichText::new("👁").size(15.0).color(eye_color);
                        if !target.visible {
                            eye_text = eye_text.strikethrough();
                        }
                        if ui
                            .add(egui::Button::new(eye_text).small().frame(false))
                            .on_hover_text("Show/hide layer")
                            .clicked()
                        {
                            target.visible = !target.visible;
                            changed = true;
                        }

                        if self.renaming_layer_id == Some(target.id) {
                            let request_focus = self.rename_focus_requested;
                            if request_focus {
                                self.rename_focus_requested = false;
                            }

                            let edit_width = (ui.available_width() - 8.0).max(80.0);
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.renaming_layer_name)
                                    .id(egui::Id::new(("layer_rename", target.id)))
                                    .desired_width(edit_width),
                            );
                            if request_focus {
                                response.request_focus();
                            }
                            if response.lost_focus() {
                                self.commit_rename(target.id);
                            }
                        } else {
                            let label = ui
                                .selectable_label(selected, egui::RichText::new(&target.name))
                                .on_hover_text("Double-click to rename");
                            if label.clicked() {
                                active_id = target.id;
                                self.selected.clear();
                            }
                            if label.double_clicked() {
                                self.finish_renaming();
                                self.renaming_layer_id = Some(target.id);
                                self.renaming_layer_name = target.name.clone();
                                self.rename_focus_requested = true;
                                active_id = target.id;
                                self.selected.clear();
                            }
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
                return close_requested;
            };
            (index, document.layers[index].clone())
        };

        let original = active_layer.clone();
        let mut target = active_layer;
        let mut changed = false;

        ui.add_space(2.0);
        ui.label(egui::RichText::new("Layer Properties").strong());

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
                .width(if cfg!(target_os = "ios") {
                    150.0
                } else {
                    110.0
                })
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
                .add_enabled(active_index > 0, egui::Button::new("Up").small())
                .on_hover_text("Move layer up")
                .clicked()
            {
                self.move_layer(active_index, active_index.saturating_sub(1));
            }
            if ui
                .add_enabled(
                    active_index + 1 < layer_count,
                    egui::Button::new("Down").small(),
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

        close_requested
    }

    fn show_canvas(&mut self, ui: &mut egui::Ui) {
        let canvas_margin = if cfg!(target_os = "ios") { 10 } else { 4 };
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(WORKSPACE_BACKGROUND)
                    .inner_margin(egui::Margin::same(canvas_margin)),
            )
            .show(ui, |ui| {
                let available = ui.available_size();
                let width = available.x.max(1.0);
                let height = available.y.max(1.0);
                let (response, painter) =
                    ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
                let workspace_rect = response.rect;
                painter.rect_filled(workspace_rect, 0.0, WORKSPACE_BACKGROUND);

                let fitting_canvas =
                    self.canvas_fit_frames_remaining > 0 || self.canvas_scale.is_none();
                if fitting_canvas && self.size_new_document_to_workspace {
                    let initial_size = initial_document_size(workspace_rect.size());
                    let mut document = self.document_mut();
                    document.width = initial_size.x;
                    document.height = initial_size.y;
                }

                let document_size = {
                    let document = self.document();
                    egui::vec2(document.width, document.height)
                };
                if fitting_canvas {
                    self.canvas_scale =
                        Some(fit_canvas_scale(workspace_rect.size(), document_size));
                    self.canvas_fit_frames_remaining =
                        self.canvas_fit_frames_remaining.saturating_sub(1);
                    if self.canvas_fit_frames_remaining == 0 {
                        self.size_new_document_to_workspace = false;
                    }
                    ui.ctx().request_repaint();
                }
                let scale = self.canvas_scale.unwrap_or(1.0);
                let space_pan = ui.input(|input| input.key_down(egui::Key::Space));
                let panning = self.tool == Tool::Pan || space_pan;
                let cursor = if response.is_pointer_button_down_on() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                };
                let response = if panning {
                    response.on_hover_cursor(cursor)
                } else {
                    response
                };

                if panning && response.is_pointer_button_down_on() {
                    let delta = ui.input(|input| input.pointer.delta());
                    if delta != egui::Vec2::ZERO {
                        self.canvas_pan += delta;
                        ui.ctx().request_repaint();
                    }
                }

                let canvas_rect =
                    document_canvas_rect(workspace_rect, document_size, scale, self.canvas_pan);
                let shadow_rect = canvas_rect.translate(egui::vec2(0.0, 5.0)).expand(3.0);
                painter.rect_filled(shadow_rect, 5.0, Color32::from_black_alpha(90));

                let background = color_from_linear(self.document().background_color);
                painter.rect_filled(canvas_rect, 2.0, background);

                self.handle_canvas_input(ui, canvas_rect, &response, panning);

                if let Some((visible_rect, uv_rect)) =
                    visible_canvas_region(canvas_rect, workspace_rect)
                {
                    let callback = egui_wgpu::Callback::new_paint_callback(
                        visible_rect,
                        CanvasCallback {
                            document: self.document.clone(),
                            render_state: self.render_state.clone(),
                            dirty_layers: self.dirty_layers.clone(),
                            revision: self.revision,
                            uv_rect,
                        },
                    );
                    painter.add(egui::Shape::Callback(callback));
                }

                let doc = self.document();
                self.paint_overlay(&painter, canvas_rect, &doc);
                painter.rect_stroke(
                    canvas_rect,
                    2.0,
                    EguiStroke::new(1.0, APP_BORDER),
                    egui::StrokeKind::Inside,
                );
            });
    }

    fn handle_canvas_input(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        response: &egui::Response,
        panning: bool,
    ) {
        let pressed = response.drag_started();
        let released = response.drag_stopped();
        let down = response.is_pointer_button_down_on();

        if panning {
            return;
        }

        // Touch input clears `latest_pos` as soon as the finger is lifted. Keep
        // release handling outside this block so the completed stroke is still
        // committed on iPhone and iPad.
        if let Some(pointer_pos) = ui.input(|i| i.pointer.latest_pos()) {
            let gesture_active = self.drawing.is_some() || self.selection_drag.is_some();
            if rect.contains(pointer_pos) || gesture_active {
                let pointer_pos = if gesture_active {
                    Pos2::new(
                        pointer_pos.x.clamp(rect.min.x, rect.max.x),
                        pointer_pos.y.clamp(rect.min.y, rect.max.y),
                    )
                } else {
                    pointer_pos
                };
                let mut doc_point = self.to_document(rect, pointer_pos);
                let (current_pressure, has_real_pressure) = pressure(ui);
                doc_point.pressure = current_pressure;

                if pressed {
                    match self.tool {
                        Tool::Brush => {
                            self.start_drawing(doc_point, current_pressure, has_real_pressure)
                        }
                        Tool::Select => {
                            let document_width = self.document().width.max(1.0);
                            let scale = rect.width() / document_width;
                            self.start_selection([doc_point.x, doc_point.y], 8.0 / scale.max(0.01));
                        }
                        Tool::Pan => {}
                    }
                }

                if down {
                    match self.tool {
                        Tool::Brush => {
                            self.continue_drawing(doc_point, current_pressure, has_real_pressure)
                        }
                        Tool::Select => self.continue_selection([doc_point.x, doc_point.y]),
                        Tool::Pan => {}
                    }
                }
            }
        }

        if released {
            match self.tool {
                Tool::Brush => self.finish_drawing(),
                Tool::Select => self.finish_selection(),
                Tool::Pan => {}
            }
        }
    }

    fn start_drawing(&mut self, point: StrokePoint, _pressure: f32, _has_real_pressure: bool) {
        // A cancelled or interrupted mobile touch may not deliver its release
        // to the same response. Preserve that stroke before starting another.
        if self.drawing.is_some() {
            self.finish_drawing();
        }

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

    fn start_selection(&mut self, point: [f32; 2], hit_tolerance: f32) {
        let layer_id = self.document().active_layer_id;
        let hits = selection::hit_test(&self.document(), point, hit_tolerance);
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
                ) / 2.0
                    * (rect.width() / document.width.max(1.0));
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
                self.saved_revision = self.revision;
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
                self.canvas_scale = None;
                self.canvas_pan = egui::Vec2::ZERO;
                self.canvas_fit_frames_remaining = 8;
                self.size_new_document_to_workspace = false;
                self.revision = self.revision.wrapping_add(1);
                self.saved_revision = self.revision;
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
        self.handle_close_request(ctx);
        self.update_window_title(ctx);

        if self.is_interacting() {
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "ios")]
        egui::Panel::top("ios_safe_area")
            .exact_size(ios_safe_area_top())
            .frame(egui::Frame::NONE)
            .show(ui, |_| {});

        let compact = ui.available_width() < 720.0;
        if !self.layers_preference_explicit {
            self.layers_open = !compact;
        }
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(ui);
        self.show_menu_bar(ui);
        self.show_toolbar(ui, compact);
        if !compact && self.layers_open {
            self.show_layer_panel(ui);
        }
        self.show_canvas(ui);
        if compact {
            self.show_layer_window(&ctx);
        }
        self.show_file_dialogs(&ctx);
        self.show_close_prompt(&ctx);
    }
}

impl InklineApp {
    fn handle_shortcuts(&mut self, ui: &mut egui::Ui) {
        if self.file_dialog != FileDialog::None || self.close_prompt_open {
            return;
        }

        let ctx = ui.ctx().clone();
        let save = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        let undo = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
        let redo = ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            ) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
        });
        if save {
            self.save_project_or_choose();
        }
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

fn svg_icon(
    source: egui::ImageSource<'static>,
    alt_text: &'static str,
    size: f32,
) -> egui::Image<'static> {
    egui::Image::new(source)
        .fit_to_exact_size(egui::Vec2::splat(size))
        .alt_text(alt_text)
}

fn compact_color_edit_button_srgba(ui: &mut egui::Ui, color: &mut Color32) -> egui::Response {
    let size = if cfg!(target_os = "ios") { 22.0 } else { 18.0 };
    ui.scope(|ui| {
        ui.spacing_mut().interact_size = egui::Vec2::splat(size);
        ui.color_edit_button_srgba(color)
    })
    .inner
}

fn initial_document_size(workspace_size: egui::Vec2) -> egui::Vec2 {
    egui::vec2(workspace_size.x.max(1.0), workspace_size.y.max(1.0))
}

fn fit_canvas_scale(workspace_size: egui::Vec2, document_size: egui::Vec2) -> f32 {
    let width_scale = workspace_size.x.max(1.0) / document_size.x.max(1.0);
    let height_scale = workspace_size.y.max(1.0) / document_size.y.max(1.0);
    width_scale.min(height_scale).max(0.05)
}

fn document_canvas_rect(
    workspace_rect: Rect,
    document_size: egui::Vec2,
    scale: f32,
    pan: egui::Vec2,
) -> Rect {
    Rect::from_center_size(
        workspace_rect.center() + pan,
        document_size * scale.max(0.05),
    )
}

fn visible_canvas_region(canvas_rect: Rect, workspace_rect: Rect) -> Option<(Rect, [f32; 4])> {
    let visible = canvas_rect.intersect(workspace_rect);
    if visible.width() <= 0.0 || visible.height() <= 0.0 {
        return None;
    }

    let width = canvas_rect.width().max(1.0);
    let height = canvas_rect.height().max(1.0);
    let uv_rect = [
        ((visible.min.x - canvas_rect.min.x) / width).clamp(0.0, 1.0),
        ((visible.min.y - canvas_rect.min.y) / height).clamp(0.0, 1.0),
        ((visible.max.x - canvas_rect.min.x) / width).clamp(0.0, 1.0),
        ((visible.max.y - canvas_rect.min.y) / height).clamp(0.0, 1.0),
    ];
    Some((visible, uv_rect))
}

fn configure_ui_style(ctx: &egui::Context) {
    let (interact_size, item_spacing, button_padding, slider_width, window_margin, menu_margin) =
        if cfg!(target_os = "ios") {
            (
                44.0,
                egui::vec2(8.0, 7.0),
                egui::vec2(11.0, 6.0),
                128.0,
                egui::Margin::same(12),
                egui::Margin::same(8),
            )
        } else {
            (
                24.0,
                egui::vec2(5.0, 3.0),
                egui::vec2(7.0, 2.0),
                96.0,
                egui::Margin::same(6),
                egui::Margin::same(4),
            )
        };

    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.spacing.item_spacing = item_spacing;
        style.spacing.button_padding = button_padding;
        style.spacing.interact_size = egui::vec2(interact_size, interact_size);
        style.spacing.slider_width = slider_width;
        style.spacing.window_margin = window_margin;
        style.spacing.menu_margin = menu_margin;

        let visuals = &mut style.visuals;
        visuals.panel_fill = APP_CHROME;
        visuals.window_fill = APP_CHROME_RAISED;
        visuals.window_stroke = EguiStroke::new(1.0, APP_BORDER);
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(10);
        visuals.weak_text_color = Some(Color32::from_rgb(170, 178, 190));
        visuals.faint_bg_color = Color32::from_rgb(28, 33, 41);
        visuals.extreme_bg_color = Color32::from_rgb(15, 18, 23);
        visuals.selection.bg_fill = APP_ACCENT;
        visuals.selection.stroke = EguiStroke::new(1.0, Color32::WHITE);
        visuals.slider_trailing_fill = true;

        visuals.widgets.noninteractive.bg_stroke = EguiStroke::new(1.0, APP_BORDER);
        visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(38, 44, 54);
        visuals.widgets.inactive.bg_stroke = EguiStroke::new(1.0, APP_BORDER);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 62, 76);
        visuals.widgets.hovered.bg_stroke = EguiStroke::new(1.0, Color32::from_rgb(92, 112, 142));
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.active.weak_bg_fill = Color32::from_rgb(61, 74, 93);
        visuals.widgets.active.bg_stroke = EguiStroke::new(1.0, APP_ACCENT);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.open.weak_bg_fill = Color32::from_rgb(47, 57, 71);
        visuals.widgets.open.bg_stroke = EguiStroke::new(1.0, APP_ACCENT);
        visuals.widgets.open.corner_radius = egui::CornerRadius::same(8);
    });
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

fn color_from_linear(color: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn file_name_or_default(path: &str, default: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(default)
        .to_owned()
}

#[cfg(target_os = "ios")]
fn default_file_name(dialog: FileDialog) -> &'static str {
    match dialog {
        FileDialog::SaveProject | FileDialog::ImportProject => "Inkline.inkline",
        FileDialog::ExportPng => "untitled.png",
        FileDialog::ExportJpeg => "untitled.jpg",
        FileDialog::ExportSvg => "untitled.svg",
        FileDialog::None => "Inkline",
    }
}

fn default_action_path(dialog: FileDialog) -> String {
    #[cfg(target_os = "ios")]
    {
        return ios_action_path(dialog, default_file_name(dialog))
            .to_string_lossy()
            .into_owned();
    }

    #[cfg(not(target_os = "ios"))]
    {
        let _ = dialog;
        String::new()
    }
}

#[cfg(target_os = "ios")]
fn ios_documents_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Documents"))
}

#[cfg(target_os = "ios")]
fn ios_action_path(dialog: FileDialog, current_path: &str) -> PathBuf {
    let default_name = default_file_name(dialog);
    let mut file_name = PathBuf::from(file_name_or_default(current_path, default_name));
    let extension = match dialog {
        FileDialog::SaveProject | FileDialog::ImportProject => "inkline",
        FileDialog::ExportPng => "png",
        FileDialog::ExportJpeg => "jpg",
        FileDialog::ExportSvg => "svg",
        FileDialog::None => "",
    };
    if !extension.is_empty()
        && file_name.extension().and_then(|value| value.to_str()) != Some(extension)
    {
        file_name.set_extension(extension);
    }

    ios_documents_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(file_name)
}

#[cfg(target_os = "ios")]
fn ios_project_files() -> Vec<PathBuf> {
    let Some(directory) = ios_documents_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut projects: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("inkline"))
        .collect();
    projects.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    projects
}

#[cfg(not(target_os = "ios"))]
fn choose_file_dialog(
    dialog: FileDialog,
    current_path: &str,
    starting_directory: Option<PathBuf>,
) -> Option<String> {
    let mut file_dialog = RfdFileDialog::new();
    if let Some(directory) = starting_directory {
        file_dialog = file_dialog.set_directory(directory);
    }

    let result = match dialog {
        FileDialog::SaveProject => file_dialog
            .set_title("Save Project")
            .add_filter("Inkline Project", &["inkline"])
            .set_file_name(file_name_or_default(current_path, "Inkline.inkline"))
            .save_file(),
        FileDialog::ImportProject => file_dialog
            .set_title("Open Project")
            .add_filter("Inkline Project", &["inkline"])
            .add_filter("All files", &["*"])
            .pick_file(),
        FileDialog::ExportPng => file_dialog
            .set_title("Export PNG")
            .add_filter("PNG Image", &["png"])
            .set_file_name(file_name_or_default(current_path, "untitled.png"))
            .save_file(),
        FileDialog::ExportJpeg => file_dialog
            .set_title("Export JPEG")
            .add_filter("JPEG Image", &["jpg", "jpeg"])
            .set_file_name(file_name_or_default(current_path, "untitled.jpg"))
            .save_file(),
        FileDialog::ExportSvg => file_dialog
            .set_title("Export SVG")
            .add_filter("SVG Image", &["svg"])
            .set_file_name(file_name_or_default(current_path, "untitled.svg"))
            .save_file(),
        FileDialog::None => return None,
    };

    result.map(|path| path.to_string_lossy().into_owned())
}

fn recent_projects_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));

    #[cfg(target_os = "ios")]
    let base = ios_documents_dir().map(|documents| documents.join(".inkline"));

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios")))]
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

fn ui_preferences_path() -> Option<PathBuf> {
    recent_projects_path().map(|path| path.with_file_name("ui_preferences"))
}

fn parse_layers_open(contents: &str) -> Option<bool> {
    let value = contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("layers_open="))?;
    match value.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn load_layers_open() -> Option<bool> {
    let contents = std::fs::read_to_string(ui_preferences_path()?).ok()?;
    parse_layers_open(&contents)
}

fn save_layers_open(open: bool) {
    let Some(path) = ui_preferences_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("layers_open={open}\n"));
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

    #[test]
    fn layers_open_preference_parses_valid_values() {
        assert_eq!(parse_layers_open("layers_open=true\n"), Some(true));
        assert_eq!(parse_layers_open("layers_open=false\n"), Some(false));
        assert_eq!(parse_layers_open("layers_open=maybe\n"), None);
        assert_eq!(parse_layers_open("unrelated=true\n"), None);
    }

    #[test]
    fn canvas_keeps_document_aspect_ratio_when_workspace_changes() {
        let document_size = egui::vec2(1280.0, 800.0);
        let first_workspace = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let second_workspace = Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let scale = fit_canvas_scale(first_workspace.size(), document_size);

        let first_canvas =
            document_canvas_rect(first_workspace, document_size, scale, egui::Vec2::ZERO);
        let second_canvas =
            document_canvas_rect(second_workspace, document_size, scale, egui::Vec2::ZERO);

        assert!((first_canvas.width() - second_canvas.width()).abs() < f32::EPSILON);
        assert!((first_canvas.height() - second_canvas.height()).abs() < f32::EPSILON);
        assert!((first_canvas.width() - first_workspace.width()).abs() < f32::EPSILON);
        assert!((first_canvas.aspect_ratio() - 1.6).abs() < 0.001);
    }

    #[test]
    fn initial_canvas_can_scale_up_to_fill_the_workspace() {
        let document_size = egui::vec2(1280.0, 800.0);
        let workspace = Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let scale = fit_canvas_scale(workspace.size(), document_size);
        let canvas = document_canvas_rect(workspace, document_size, scale, egui::Vec2::ZERO);

        assert_eq!(scale, 1.25);
        assert_eq!(canvas, workspace);
    }

    #[test]
    fn new_document_uses_the_exact_workspace_size() {
        assert_eq!(
            initial_document_size(egui::vec2(390.0, 712.0)),
            egui::vec2(390.0, 712.0)
        );
        assert_eq!(
            initial_document_size(egui::vec2(0.0, -20.0)),
            egui::vec2(1.0, 1.0)
        );
    }

    #[test]
    fn canvas_pan_moves_the_document_without_resizing_it() {
        let workspace = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let document_size = egui::vec2(1280.0, 800.0);
        let centered = document_canvas_rect(workspace, document_size, 0.5, egui::Vec2::ZERO);
        let moved = document_canvas_rect(workspace, document_size, 0.5, egui::vec2(35.0, -20.0));

        assert_eq!(centered.size(), moved.size());
        assert_eq!(moved.center() - centered.center(), egui::vec2(35.0, -20.0));
    }

    #[test]
    fn clipped_canvas_uses_matching_texture_region() {
        let canvas = Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0));
        let workspace = Rect::from_min_max(Pos2::new(25.0, 20.0), Pos2::new(75.0, 60.0));
        let (visible, uv) = visible_canvas_region(canvas, workspace).unwrap();

        assert_eq!(visible, workspace);
        assert_eq!(uv, [0.25, 0.25, 0.75, 0.75]);
    }
}
