use crate::data::{ColumnKind, Table};
use crate::model::{AxisSide, ChartType, SeriesConfig, SubplotConfig};
use crate::plotting::render_subplot;
use chrono::{NaiveDate, TimeZone, Utc};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::rc::Rc;

/// What to do with a subplot screenshot once egui hands it back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScreenshotAction {
    SaveToFile,
    CopyToClipboard,
}

pub struct DaplotApp {
    table: Option<Rc<Table>>,
    file_path: Option<PathBuf>,
    file_path_input: String,
    load_error: Option<String>,

    subplots: Vec<SubplotConfig>,

    // Global time-range filter, applied (by row index) to every subplot.
    filter_enabled: bool,
    filter_column: Option<String>,
    filter_start: NaiveDate,
    filter_end: NaiveDate,
    data_min_date: NaiveDate,
    data_max_date: NaiveDate,

    // Hides editing chrome (settings, per-subplot controls, the export
    // buttons themselves) so subplots present cleanly — toggled by hand, and
    // also flipped on automatically for the duration of a PNG capture.
    presentation_mode: bool,

    // Set when the "Export as PNG" / "Copy as PNG" button is clicked; a
    // viewport screenshot is requested that frame, and the resulting image
    // is cropped to `subplots_rect` (covering every subplot together) and
    // handled once egui delivers it via `Event::Screenshot`.
    pending_screenshot: Option<ScreenshotAction>,
    // Presentation-mode state to restore once the pending capture completes.
    presentation_mode_before_capture: Option<bool>,
    subplots_rect: Option<egui::Rect>,
    screenshot_error: Option<String>,
}

impl Default for DaplotApp {
    fn default() -> Self {
        let today = Utc::now().date_naive();
        Self {
            table: None,
            file_path: None,
            file_path_input: String::new(),
            load_error: None,
            subplots: Vec::new(),
            filter_enabled: false,
            filter_column: None,
            filter_start: today,
            filter_end: today,
            data_min_date: today,
            data_max_date: today,
            presentation_mode: false,
            pending_screenshot: None,
            presentation_mode_before_capture: None,
            subplots_rect: None,
            screenshot_error: None,
        }
    }
}

impl DaplotApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    fn load_file(&mut self, path: PathBuf) {
        match Table::load(&path) {
            Ok(table) => {
                self.file_path = Some(path);
                self.load_error = None;

                // Reset everything derived from the old table.
                self.filter_column = table.first_datetime_column();
                self.filter_enabled = false;
                if let Some(col_name) = &self.filter_column
                    && let Some(idx) = table.column_index(col_name)
                    && let ColumnKind::DateTime(vals) = &table.columns[idx].kind
                {
                    let (mn, mx) = min_max(vals);
                    self.data_min_date = seconds_to_date(mn);
                    self.data_max_date = seconds_to_date(mx);
                    self.filter_start = self.data_min_date;
                    self.filter_end = self.data_max_date;
                }

                // Start the user off with one subplot using the detected x
                // column (or first column), with no series pre-selected —
                // the user picks which columns to plot.
                let default_x = self
                    .filter_column
                    .clone()
                    .or_else(|| table.column_names().first().cloned());
                let mut subplot = SubplotConfig::new(0, default_x);
                if let Some(stem) = self
                    .file_path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .and_then(|s| s.to_str())
                {
                    subplot.title = stem.to_string();
                }
                self.subplots = vec![subplot];
                self.table = Some(Rc::new(table));
            }
            Err(e) => {
                self.load_error = Some(format!("{e:#}"));
            }
        }
    }

    /// Pick up any file(s) the user just dragged onto the window.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().next() {
            let path = file.path().to_path_buf();
            self.file_path_input = path.to_string_lossy().to_string();
            self.load_file(path);
        }
    }

    fn row_mask(&self) -> Option<Vec<bool>> {
        let table = self.table.as_ref()?;
        if !self.filter_enabled {
            return None;
        }
        let col_name = self.filter_column.as_ref()?;
        let idx = table.column_index(col_name)?;
        let ColumnKind::DateTime(vals) = &table.columns[idx].kind else {
            return None;
        };
        let start = date_to_seconds(self.filter_start);
        // include the whole end day
        let end = date_to_seconds(self.filter_end) + 86_400.0;
        Some(
            vals.iter()
                .map(|v| v.is_finite() && *v >= start && *v < end)
                .collect(),
        )
    }

    fn top_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_panel").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("daplot").strong());
                ui.separator();

                if !self.presentation_mode {
                    ui.label("File path:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.file_path_input)
                            .desired_width(320.0)
                            .hint_text(
                                "/path/to/data.csv or .parquet — or drag & drop a file anywhere",
                            ),
                    );
                    let enter_pressed =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Load").clicked() || enter_pressed {
                        let path = PathBuf::from(self.file_path_input.trim());
                        self.load_file(path);
                    }
                    if ui.button("📂 Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("CSV / Parquet", &["csv", "parquet"])
                            .add_filter("All files", &["*"])
                            .pick_file()
                    {
                        self.file_path_input = path.to_string_lossy().to_string();
                        self.load_file(path);
                    }
                    if let Some(path) = &self.file_path {
                        ui.label(
                            RichText::new(format!(
                                "✔ {}",
                                path.file_name().and_then(|s| s.to_str()).unwrap_or("")
                            ))
                            .weak(),
                        );
                    }
                    ui.separator();
                }

                ui.checkbox(&mut self.presentation_mode, "🎥 Presentation mode");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::widgets::global_theme_preference_switch(ui);
                });
            });
            if !self.presentation_mode {
                if let Some(err) = &self.load_error {
                    ui.colored_label(Color32::RED, format!("⚠ {err}"));
                }
                if let Some(err) = &self.screenshot_error {
                    ui.colored_label(Color32::RED, format!("⚠ {err}"));
                }
            }
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        if self.presentation_mode {
            return;
        }
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading("Data");
                let Some(table) = &self.table else {
                    ui.label("Load a CSV or Parquet file to get started.");
                    return;
                };
                ui.label(format!(
                    "{} rows, {} columns",
                    table.row_count,
                    table.columns.len()
                ));
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .id_salt("columns_scroll")
                    .show(ui, |ui| {
                        for c in &table.columns {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&c.name).strong());
                                ui.label(RichText::new(c.type_label()).weak().small());
                            });
                        }
                    });

                ui.separator();
                ui.heading("Time range filter");
                let datetime_cols: Vec<String> = table
                    .columns
                    .iter()
                    .filter(|c| matches!(c.kind, ColumnKind::DateTime(_)))
                    .map(|c| c.name.clone())
                    .collect();

                if datetime_cols.is_empty() {
                    ui.label("No datetime column detected.");
                } else {
                    ui.checkbox(&mut self.filter_enabled, "Enable filter");
                    egui::ComboBox::from_label("Time column")
                        .selected_text(self.filter_column.clone().unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for name in &datetime_cols {
                                let selected = self.filter_column.as_deref() == Some(name.as_str());
                                if ui.selectable_label(selected, name).clicked() {
                                    self.filter_column = Some(name.clone());
                                    if let Some(idx) = table.column_index(name)
                                        && let ColumnKind::DateTime(vals) = &table.columns[idx].kind
                                    {
                                        let (mn, mx) = min_max(vals);
                                        self.data_min_date = seconds_to_date(mn);
                                        self.data_max_date = seconds_to_date(mx);
                                        self.filter_start = self.data_min_date;
                                        self.filter_end = self.data_max_date;
                                    }
                                }
                            }
                        });

                    ui.add_enabled_ui(self.filter_enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("From:");
                            let mut jiff_start = naive_to_jiff(self.filter_start);
                            if ui
                                .add(egui_extras::DatePickerButton::new(&mut jiff_start))
                                .changed()
                            {
                                self.filter_start = jiff_to_naive(jiff_start);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("To:");
                            let mut jiff_end = naive_to_jiff(self.filter_end);
                            if ui
                                .add(egui_extras::DatePickerButton::new(&mut jiff_end))
                                .changed()
                            {
                                self.filter_end = jiff_to_naive(jiff_end);
                            }
                        });
                        if ui.small_button("Reset to full range").clicked() {
                            self.filter_start = self.data_min_date;
                            self.filter_end = self.data_max_date;
                        }
                    });
                }
            });
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(table) = self.table.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("Open a CSV or Parquet file to start plotting.")
                            .size(16.0)
                            .weak(),
                    );
                });
                return;
            };

            let row_mask = self.row_mask();

            let mut remove_idx: Option<usize> = None;
            let subplot_count = self.subplots.len();

            if !self.presentation_mode {
                ui.horizontal(|ui| {
                    let can_capture = self.subplots_rect.is_some();
                    if ui
                        .add_enabled(can_capture, egui::Button::new("📷 Export as PNG"))
                        .clicked()
                    {
                        self.begin_capture(ui.ctx(), ScreenshotAction::SaveToFile);
                    }
                    if ui
                        .add_enabled(can_capture, egui::Button::new("📋 Copy as PNG"))
                        .clicked()
                    {
                        self.begin_capture(ui.ctx(), ScreenshotAction::CopyToClipboard);
                    }
                    ui.label(RichText::new("(captures all subplots together)").weak());
                });
                ui.add_space(4.0);
            }

            let scroll_output = egui::ScrollArea::vertical()
                .id_salt("subplots_scroll")
                .show(ui, |ui| {
                    ui.scope(|ui| {
                        for i in 0..subplot_count {
                            ui.push_id(self.subplots[i].id, |ui| {
                                egui::Frame::group(ui.style()).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if self.presentation_mode {
                                            ui.heading(self.subplots[i].title.clone());
                                        } else {
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.subplots[i].title)
                                                    .desired_width(220.0)
                                                    .font(egui::TextStyle::Heading),
                                            );
                                            ui.checkbox(&mut self.subplots[i].show_legend, "Legend");
                                            ui.add(
                                                egui::DragValue::new(&mut self.subplots[i].height)
                                                    .range(150.0..=900.0)
                                                    .prefix("height: "),
                                            );
                                            if ui.button("🔄 Reset scale").clicked() {
                                                self.subplots[i].reset_scale = true;
                                            }
                                            if ui.button("🗑 Remove subplot").clicked() {
                                                remove_idx = Some(i);
                                            }
                                        }
                                    });

                                if !self.presentation_mode {
                                    let header_id = ui.make_persistent_id("settings");
                                    egui::containers::collapsing_header::CollapsingState::load_with_default_open(
                                        ui.ctx(),
                                        header_id,
                                        false,
                                    )
                                    .show_header(ui, |ui| {
                                        ui.label("Series & axis settings");
                                    })
                                    .body(|ui| {
                                        subplot_settings_ui(ui, &table, &mut self.subplots[i]);
                                    });

                                    ui.add_space(4.0);
                                }
                                render_subplot(ui, &table, row_mask.as_ref(), &mut self.subplots[i]);
                            });
                        });
                        ui.add_space(8.0);
                        }

                        if !self.presentation_mode
                            && ui.button("➕ Add subplot").clicked()
                        {
                            let default_x = self
                                .filter_column
                                .clone()
                                .or_else(|| table.column_names().first().cloned());
                            self.subplots
                                .push(SubplotConfig::new(self.subplots.len(), default_x));
                        }
                    })
                    .response
                    .rect
                });

            self.subplots_rect = if subplot_count > 0 { Some(scroll_output.inner) } else { None };

            if let Some(idx) = remove_idx {
                self.subplots.remove(idx);
            }
        });
    }
}

/// Pick the column for a newly-added series: the one right after the most
/// recently added series' column (or after the X column, if there are no
/// series yet), so repeated adds step through `y_columns` in order. Wraps
/// around, and falls back to the first column once every column is used.
fn next_series_column(y_columns: &[String], subplot: &SubplotConfig) -> Option<String> {
    let start_col = subplot
        .series
        .last()
        .map(|s| s.y_column.clone())
        .or_else(|| subplot.x_column.clone());
    let start_idx = start_col
        .and_then(|c| y_columns.iter().position(|y| *y == c))
        .map(|i| i + 1)
        .unwrap_or(0);
    y_columns
        .iter()
        .cycle()
        .skip(start_idx)
        .take(y_columns.len())
        .find(|c| !subplot.series.iter().any(|s| &s.y_column == *c))
        .cloned()
        .or_else(|| y_columns.first().cloned())
}

fn subplot_settings_ui(ui: &mut egui::Ui, table: &Table, subplot: &mut SubplotConfig) {
    let all_columns = table.column_names();
    let y_columns = table.plottable_y_columns();

    egui::Grid::new("subplot_titles_grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("X column:");
            egui::ComboBox::from_id_salt("x_col")
                .selected_text(
                    subplot
                        .x_column
                        .clone()
                        .unwrap_or_else(|| "(choose)".into()),
                )
                .show_ui(ui, |ui| {
                    for name in &all_columns {
                        let selected = subplot.x_column.as_deref() == Some(name.as_str());
                        if ui.selectable_label(selected, name).clicked() {
                            subplot.x_column = Some(name.clone());
                            subplot.x_axis_title = name.clone();
                        }
                    }
                });
            ui.end_row();

            ui.label("X axis title:");
            ui.text_edit_singleline(&mut subplot.x_axis_title);
            ui.end_row();

            ui.label("Primary Y axis title:");
            ui.text_edit_singleline(&mut subplot.y_axis_title);
            ui.end_row();

            ui.label("Secondary Y axis title:");
            ui.text_edit_singleline(&mut subplot.y2_axis_title);
            ui.end_row();
        });

    ui.separator();
    ui.label(RichText::new("Series").strong());

    let mut remove_series: Option<usize> = None;
    for (si, series) in subplot.series.iter_mut().enumerate() {
        ui.push_id(series.id, |ui| {
            egui::Frame::NONE
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut series.visible, "");
                        ui.color_edit_button_srgba(&mut series.color);
                        ui.add(
                            egui::TextEdit::singleline(&mut series.name)
                                .desired_width(120.0)
                                .hint_text("Legend name"),
                        );

                        egui::ComboBox::from_id_salt("y_col")
                            .selected_text(series.y_column.clone())
                            .show_ui(ui, |ui| {
                                for name in &y_columns {
                                    let selected = &series.y_column == name;
                                    if ui.selectable_label(selected, name).clicked() {
                                        series.y_column = name.clone();
                                        series.name = name.clone();
                                    }
                                }
                            });

                        egui::ComboBox::from_id_salt("chart_type")
                            .selected_text(series.chart_type.label())
                            .show_ui(ui, |ui| {
                                for ct in ChartType::ALL {
                                    if ui
                                        .selectable_label(series.chart_type == ct, ct.label())
                                        .clicked()
                                    {
                                        series.chart_type = ct;
                                    }
                                }
                            });

                        egui::ComboBox::from_id_salt("axis_side")
                            .selected_text(match series.axis {
                                AxisSide::Primary => "Primary axis",
                                AxisSide::Secondary => "Secondary axis",
                            })
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        series.axis == AxisSide::Primary,
                                        "Primary axis",
                                    )
                                    .clicked()
                                {
                                    series.axis = AxisSide::Primary;
                                }
                                if ui
                                    .selectable_label(
                                        series.axis == AxisSide::Secondary,
                                        "Secondary axis",
                                    )
                                    .clicked()
                                {
                                    series.axis = AxisSide::Secondary;
                                }
                            });

                        if matches!(series.chart_type, ChartType::Line | ChartType::LineMarker) {
                            ui.add(
                                egui::DragValue::new(&mut series.line_width)
                                    .range(0.5..=8.0)
                                    .speed(0.1)
                                    .prefix("width "),
                            );
                        }
                        if matches!(
                            series.chart_type,
                            ChartType::Scatter | ChartType::LineMarker
                        ) {
                            ui.add(
                                egui::DragValue::new(&mut series.marker_radius)
                                    .range(0.5..=10.0)
                                    .speed(0.1)
                                    .prefix("radius "),
                            );
                        }

                        if ui.button("🗑").clicked() {
                            remove_series = Some(si);
                        }
                    });
                });
        });
    }
    if let Some(idx) = remove_series {
        subplot.series.remove(idx);
    }

    if ui.button("➕ Add series").clicked() {
        let used = subplot.series.len();
        if let Some(col) = next_series_column(&y_columns, subplot) {
            subplot.series.push(SeriesConfig::new(col, used));
        }
    }
}

impl eframe::App for DaplotApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.top_panel(ui);
        self.left_panel(ui);
        self.central_panel(ui);
        self.handle_screenshot_events(&ctx);
    }
}

impl DaplotApp {
    /// Kick off a PNG capture: remember whether presentation mode was
    /// already on, force it on so the screenshot comes out clean (no
    /// editing chrome), and request the screenshot itself. The rest of
    /// this frame's rendering (the subplot loop, drawn after the button
    /// that calls this) already reflects the forced-on presentation mode.
    fn begin_capture(&mut self, ctx: &egui::Context, action: ScreenshotAction) {
        self.presentation_mode_before_capture = Some(self.presentation_mode);
        self.presentation_mode = true;
        self.pending_screenshot = Some(action);
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
    }

    /// Look for a screenshot egui just captured (in response to a prior
    /// `ViewportCommand::Screenshot`) and, if the export/copy button
    /// requested it, crop it down to the combined subplots area and act on it.
    fn handle_screenshot_events(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_screenshot else {
            return;
        };

        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            return;
        };
        self.pending_screenshot = None;
        // The pixels are already captured; safe to drop back out of
        // presentation mode now, whatever happens with the image below.
        if let Some(saved) = self.presentation_mode_before_capture.take() {
            self.presentation_mode = saved;
        }

        let Some(rect) = self.subplots_rect else {
            self.screenshot_error = Some("No subplots are visible to capture.".into());
            return;
        };
        // Clamp to the actual screen, since the content rect can extend
        // past the visible viewport (e.g. when scrolled).
        let rect = rect.intersect(ctx.input(|i| i.viewport_rect()));
        if !rect.is_positive() {
            self.screenshot_error =
                Some("Nothing visible to capture — scroll the subplots into view.".into());
            return;
        }
        let title = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("daplot_export")
            .to_string();

        let cropped = image.region(&rect, Some(ctx.pixels_per_point()));
        let rgba: Vec<u8> = cropped.pixels.iter().flat_map(|c| c.to_array()).collect();
        let (width, height) = (cropped.size[0], cropped.size[1]);

        self.screenshot_error = None;
        match action {
            ScreenshotAction::SaveToFile => {
                let default_name = format!("{}.png", sanitize_filename(&title));
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PNG image", &["png"])
                    .set_file_name(&default_name)
                    .save_file()
                    && let Err(e) = image::save_buffer(
                        &path,
                        &rgba,
                        width as u32,
                        height as u32,
                        image::ColorType::Rgba8,
                    )
                {
                    self.screenshot_error = Some(format!("Failed to save PNG: {e}"));
                }
            }
            ScreenshotAction::CopyToClipboard => match arboard::Clipboard::new() {
                Ok(mut clipboard) => {
                    let image_data = arboard::ImageData {
                        width,
                        height,
                        bytes: std::borrow::Cow::Owned(rgba),
                    };
                    if let Err(e) = clipboard.set_image(image_data) {
                        self.screenshot_error = Some(format!("Failed to copy to clipboard: {e}"));
                    }
                }
                Err(e) => {
                    self.screenshot_error = Some(format!("Failed to access clipboard: {e}"));
                }
            },
        }
    }
}

/// Turn a subplot title into a filesystem-safe default filename stem.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "plot".to_string()
    } else {
        trimmed.to_string()
    }
}

fn min_max(vals: &[f64]) -> (f64, f64) {
    let mut mn = f64::INFINITY;
    let mut mx = f64::NEG_INFINITY;
    for &v in vals {
        if v.is_finite() {
            if v < mn {
                mn = v;
            }
            if v > mx {
                mx = v;
            }
        }
    }
    if !mn.is_finite() || !mx.is_finite() {
        (0.0, 0.0)
    } else {
        (mn, mx)
    }
}

fn seconds_to_date(secs: f64) -> NaiveDate {
    let whole = secs.floor() as i64;
    match Utc.timestamp_opt(whole, 0) {
        chrono::LocalResult::Single(dt) => dt.date_naive(),
        _ => Utc::now().date_naive(),
    }
}

fn date_to_seconds(date: NaiveDate) -> f64 {
    let dt = date.and_hms_opt(0, 0, 0).unwrap();
    Utc.from_utc_datetime(&dt).timestamp() as f64
}

/// egui_extras' DatePickerButton (0.36+) uses `jiff::civil::Date` rather than
/// `chrono::NaiveDate`. The rest of this app stays on chrono (used for CSV
/// parsing), so we convert at the UI boundary only.
fn naive_to_jiff(d: NaiveDate) -> jiff::civil::Date {
    use chrono::Datelike;
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8)
        .unwrap_or(jiff::civil::Date::constant(1970, 1, 1))
}

fn jiff_to_naive(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn first_add_skips_the_x_column() {
        let subplot = SubplotConfig::new(0, Some("X".to_string()));
        let y_columns = columns(&["X", "Y", "Z"]);
        assert_eq!(
            next_series_column(&y_columns, &subplot),
            Some("Y".to_string())
        );
    }

    #[test]
    fn subsequent_adds_step_through_remaining_columns() {
        let mut subplot = SubplotConfig::new(0, Some("X".to_string()));
        let y_columns = columns(&["X", "Y", "Z"]);

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Y");
        subplot.series.push(SeriesConfig::new(col, 0));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Z");
        subplot.series.push(SeriesConfig::new(col, 1));
    }

    #[test]
    fn wraps_around_and_reuses_columns_once_all_are_taken() {
        let mut subplot = SubplotConfig::new(0, Some("X".to_string()));
        let y_columns = columns(&["X", "Y", "Z"]);

        for _ in 0..2 {
            let col = next_series_column(&y_columns, &subplot).unwrap();
            let used = subplot.series.len();
            subplot.series.push(SeriesConfig::new(col, used));
        }
        // Y and Z are now used; the next pick wraps back to X.
        assert_eq!(
            next_series_column(&y_columns, &subplot),
            Some("X".to_string())
        );
    }

    #[test]
    fn no_x_column_and_no_series_picks_the_first_column() {
        let subplot = SubplotConfig::new(0, None);
        let y_columns = columns(&["X", "Y", "Z"]);
        assert_eq!(
            next_series_column(&y_columns, &subplot),
            Some("X".to_string())
        );
    }

    #[test]
    fn empty_column_list_yields_nothing() {
        let subplot = SubplotConfig::new(0, Some("X".to_string()));
        assert_eq!(next_series_column(&[], &subplot), None);
    }

    #[test]
    fn works_when_x_axis_is_the_first_timestamp_column() {
        // Mirrors a real file where the X axis is a timestamp column, not
        // literally the first column, with junk columns around it.
        let y_columns = columns(&["Timestamp1", "Timestamp2", "Random Garbage", "X", "Y", "Z"]);
        let mut subplot = SubplotConfig::new(0, Some("Timestamp1".to_string()));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Timestamp2");
        subplot.series.push(SeriesConfig::new(col, 0));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Random Garbage");
        subplot.series.push(SeriesConfig::new(col, 1));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "X");
    }

    #[test]
    fn works_when_x_axis_is_the_second_timestamp_column() {
        let y_columns = columns(&["Timestamp1", "Timestamp2", "Random Garbage", "X", "Y", "Z"]);
        let mut subplot = SubplotConfig::new(0, Some("Timestamp2".to_string()));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Random Garbage");
        subplot.series.push(SeriesConfig::new(col, 0));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "X");
        subplot.series.push(SeriesConfig::new(col, 1));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Y");
    }

    #[test]
    fn advances_from_a_manually_reassigned_series_column() {
        // The auto-picked column for a series can be overridden by hand via
        // the series' own y-column dropdown. The *next* add should advance
        // from that manually-picked column, not the one it replaced.
        let y_columns = columns(&["Timestamp1", "Timestamp2", "Random Garbage", "X", "Y", "Z"]);
        let mut subplot = SubplotConfig::new(0, Some("Timestamp1".to_string()));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Timestamp2");
        subplot.series.push(SeriesConfig::new(col, 0));

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Random Garbage");
        subplot.series.push(SeriesConfig::new(col, 1));

        // User manually reassigns that last series from "Random Garbage" to "X".
        subplot.series.last_mut().unwrap().y_column = "X".to_string();

        let col = next_series_column(&y_columns, &subplot).unwrap();
        assert_eq!(col, "Y");
    }

    #[test]
    fn works_when_x_axis_timestamp_is_the_last_column() {
        // If the timestamp used for the X axis happens to be the last
        // plottable column, "next after X" wraps to the first column.
        let y_columns = columns(&["Random Garbage", "Y", "Z", "X", "Timestamp1"]);
        let subplot = SubplotConfig::new(0, Some("Timestamp1".to_string()));
        assert_eq!(
            next_series_column(&y_columns, &subplot),
            Some("Random Garbage".to_string())
        );
    }
}
