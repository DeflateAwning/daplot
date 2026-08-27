//! Configuration structs describing what the user has asked to plot:
//! one or more subplots, each with one or more series.

use eframe::egui::Color32;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Color cycle used for new series. The first three match the standard
/// CAD convention for X/Y/Z axes (red/green/blue), since plotting three
/// axes of data is a common case; the rest continue an Excel-like cycle.
pub const PALETTE: [Color32; 10] = [
    Color32::from_rgb(0xe6, 0x1e, 0x25), // X - red
    Color32::from_rgb(0x3d, 0xa5, 0x35), // Y - green
    Color32::from_rgb(0x25, 0x63, 0xeb), // Z - blue
    Color32::from_rgb(0xff, 0x7f, 0x0e),
    Color32::from_rgb(0x94, 0x67, 0xbd),
    Color32::from_rgb(0x8c, 0x56, 0x4b),
    Color32::from_rgb(0xe3, 0x77, 0xc2),
    Color32::from_rgb(0x7f, 0x7f, 0x7f),
    Color32::from_rgb(0xbc, 0xbd, 0x22),
    Color32::from_rgb(0x17, 0xbe, 0xcf),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChartType {
    Line,
    LineMarker,
    Scatter,
    Bar,
}

impl ChartType {
    pub const ALL: [ChartType; 4] = [
        ChartType::Line,
        ChartType::LineMarker,
        ChartType::Scatter,
        ChartType::Bar,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            ChartType::Line => "Line",
            ChartType::LineMarker => "Line + Marker",
            ChartType::Scatter => "Scatter",
            ChartType::Bar => "Bar",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AxisSide {
    Primary,
    Secondary,
}

#[derive(Clone, Debug)]
pub struct SeriesConfig {
    pub id: u64,
    pub name: String,
    pub y_column: String,
    pub chart_type: ChartType,
    pub axis: AxisSide,
    pub color: Color32,
    pub visible: bool,
    pub line_width: f32,
    pub marker_radius: f32,
}

impl SeriesConfig {
    pub fn new(y_column: String, palette_idx: usize) -> Self {
        Self {
            id: next_id(),
            name: y_column.clone(),
            y_column,
            chart_type: ChartType::LineMarker,
            axis: AxisSide::Primary,
            color: PALETTE[palette_idx % PALETTE.len()],
            visible: true,
            line_width: 1.8,
            marker_radius: 2.5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubplotConfig {
    pub id: u64,
    pub title: String,
    pub x_column: Option<String>,
    pub x_axis_title: String,
    pub y_axis_title: String,
    pub y2_axis_title: String,
    pub series: Vec<SeriesConfig>,
    pub show_legend: bool,
    pub height: f32,
    #[allow(dead_code)]
    pub collapsed_settings: bool,
    pub reset_scale: bool,

    /// When set, the X/Y axis is clamped to `*_axis_min..=*_axis_max` every
    /// frame instead of being mouse-zoomable/draggable.
    pub x_axis_fixed: bool,
    pub x_axis_min: f64,
    pub x_axis_max: f64,
    pub y_axis_fixed: bool,
    pub y_axis_min: f64,
    pub y_axis_max: f64,
    /// The plot's actual X/Y bounds as of the last frame it was drawn (auto
    /// or fixed). Used to pre-fill the fixed-range fields above with
    /// something sensible, and to seed the time-range filter from the
    /// current view.
    pub last_x_bounds: (f64, f64),
    pub last_y_bounds: (f64, f64),
}

impl SubplotConfig {
    pub fn new(index: usize, x_column: Option<String>) -> Self {
        let x_axis_title = x_column.clone().unwrap_or_default();
        Self {
            id: next_id(),
            title: format!("Subplot {}", index + 1),
            x_column,
            x_axis_title,
            y_axis_title: String::new(),
            y2_axis_title: String::new(),
            series: Vec::new(),
            show_legend: true,
            height: 320.0,
            collapsed_settings: false,
            reset_scale: false,
            x_axis_fixed: false,
            x_axis_min: 0.0,
            x_axis_max: 1.0,
            y_axis_fixed: false,
            y_axis_min: 0.0,
            y_axis_max: 1.0,
            last_x_bounds: (0.0, 1.0),
            last_y_bounds: (0.0, 1.0),
        }
    }

    #[allow(dead_code)]
    pub fn has_secondary_series(&self) -> bool {
        self.series
            .iter()
            .any(|s| s.visible && s.axis == AxisSide::Secondary)
    }
}
