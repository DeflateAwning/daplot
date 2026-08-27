//! Turns a `SubplotConfig` + the loaded `Table` (+ optional row filter mask)
//! into an actual `egui_plot` widget.

use crate::data::Table;
use crate::model::{AxisSide, ChartType, SubplotConfig};
use eframe::egui::{Color32, Id, TextStyle, Ui};
use egui_plot::{AxisHints, Bar, BarChart, Legend, Line, Placement, Plot, PlotPoints, Points};

/// Extra padding (beyond the widest tick-label text) reserved for the
/// left-side Y axis strip, matching the side margin `egui_plot` itself
/// applies to Y tick labels.
const Y_AXIS_WIDTH_PADDING: f32 = 10.0;

/// Widest left-Y-axis strip needed across `subplots`, so that when the X
/// axis is linked, every subplot can be told to reserve the same width for
/// its Y axis — keeping their plot areas (and hence X axes / X-axis titles)
/// aligned vertically. Estimated from each subplot's current Y bounds since
/// `egui_plot` doesn't expose actual tick text before it lays the plot out.
pub fn shared_y_axis_min_thickness(ui: &Ui, subplots: &[SubplotConfig]) -> f32 {
    let font_id = TextStyle::Body.resolve(ui.style());
    let mut widest: f32 = 0.0;
    for subplot in subplots {
        for value in [subplot.y_axis_min, subplot.y_axis_max] {
            if !value.is_finite() {
                continue;
            }
            let text = egui_plot::format_number(value, 5);
            let width = ui
                .painter()
                .layout_no_wrap(text, font_id.clone(), Color32::WHITE)
                .size()
                .x;
            widest = widest.max(width);
        }
    }
    if widest > 0.0 {
        widest + Y_AXIS_WIDTH_PADDING
    } else {
        14.0 // egui_plot's own default min thickness
    }
}

/// Render one subplot (title, plot area) into `ui`. Returns nothing; all
/// interaction happens through `subplot`'s own widgets drawn elsewhere.
pub fn render_subplot(
    ui: &mut Ui,
    table: &Table,
    row_mask: Option<&Vec<bool>>,
    subplot: &mut SubplotConfig,
    link_x_axis: bool,
    shared_y_axis_min_thickness: Option<f32>,
) {
    let Some(x_col_name) = subplot.x_column.clone() else {
        ui.colored_label(
            Color32::from_rgb(200, 120, 40),
            "Pick an X column below to see this plot.",
        );
        return;
    };
    let Some(x_idx) = table.column_index(&x_col_name) else {
        ui.colored_label(
            Color32::RED,
            format!("X column '{x_col_name}' not found in data."),
        );
        return;
    };

    let x_is_datetime = table.is_datetime(x_idx);
    let x_categories = table.text_labels(x_idx); // Some(labels) if X is categorical text
    let x_raw = table.as_f64(x_idx);

    let row_count = table.row_count;
    let included: Vec<usize> = (0..row_count)
        .filter(|&i| row_mask.is_none_or(|m| m.get(i).copied().unwrap_or(true)))
        .collect();

    // Pre-compute filtered, sorted (x, y) series for every visible series.
    struct Prepared {
        name: String,
        color: Color32,
        chart_type: ChartType,
        axis: AxisSide,
        line_width: f32,
        marker_radius: f32,
        points: Vec<[f64; 2]>,
    }

    let mut prepared: Vec<Prepared> = Vec::new();
    for series in &subplot.series {
        if !series.visible {
            continue;
        }
        let Some(y_idx) = table.column_index(&series.y_column) else {
            continue;
        };
        let y_raw = table.as_f64(y_idx);

        let mut pts: Vec<[f64; 2]> = Vec::with_capacity(included.len());
        for &i in &included {
            let x = *x_raw.get(i).unwrap_or(&f64::NAN);
            let y = *y_raw.get(i).unwrap_or(&f64::NAN);
            if x.is_finite() && y.is_finite() {
                pts.push([x, y]);
            }
        }
        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));

        prepared.push(Prepared {
            name: series.name.clone(),
            color: series.color,
            chart_type: series.chart_type,
            axis: series.axis,
            line_width: series.line_width,
            marker_radius: series.marker_radius,
            points: pts,
        });
    }

    if prepared.is_empty() {
        ui.colored_label(Color32::GRAY, "Add at least one series below to plot data.");
        return;
    }

    // ----- Dual-axis rescaling -----------------------------------------
    // egui_plot draws all items in one shared coordinate space, so a "true"
    // secondary axis is emulated by linearly remapping secondary-axis
    // series into the primary series' value range, then drawing a second
    // axis whose tick labels are computed with the inverse mapping.
    let primary_vals: Vec<f64> = prepared
        .iter()
        .filter(|p| p.axis == AxisSide::Primary)
        .flat_map(|p| p.points.iter().map(|pt| pt[1]))
        .collect();
    let secondary_vals: Vec<f64> = prepared
        .iter()
        .filter(|p| p.axis == AxisSide::Secondary)
        .flat_map(|p| p.points.iter().map(|pt| pt[1]))
        .collect();

    let has_secondary = !secondary_vals.is_empty();

    let (pmin, pmax) = min_max_padded(&primary_vals);
    let (smin, smax) = min_max_padded(&secondary_vals);

    let remap_secondary_to_primary = move |v: f64| -> f64 {
        if (smax - smin).abs() < f64::EPSILON {
            return (pmin + pmax) / 2.0;
        }
        pmin + (v - smin) / (smax - smin) * (pmax - pmin)
    };
    let remap_primary_to_secondary = move |v: f64| -> f64 {
        if (pmax - pmin).abs() < f64::EPSILON {
            return (smin + smax) / 2.0;
        }
        smin + (v - pmin) / (pmax - pmin) * (smax - smin)
    };

    for p in &mut prepared.iter_mut() {
        if p.axis == AxisSide::Secondary {
            for pt in &mut p.points {
                pt[1] = remap_secondary_to_primary(pt[1]);
            }
        }
    }

    // ----- Axis label / tick formatters ---------------------------------
    let x_fmt_categories = x_categories.clone();
    let x_formatter =
        move |mark: egui_plot::GridMark, _range: &std::ops::RangeInclusive<f64>| -> String {
            let tick = mark.value;
            if x_is_datetime {
                format_timestamp(tick)
            } else if let Some(labels) = &x_fmt_categories {
                let idx = tick.round() as i64;
                if idx >= 0 && (idx as usize) < labels.len() && (tick - tick.round()).abs() < 1e-6 {
                    labels[idx as usize].clone()
                } else {
                    String::new()
                }
            } else {
                egui_plot::format_number(tick, 5)
            }
        };

    let mut x_hints = AxisHints::new_x()
        .label(subplot.x_axis_title.clone())
        .placement(Placement::LeftBottom)
        .formatter(x_formatter);
    if x_categories.is_some() {
        x_hints = x_hints.min_thickness(28.0);
    }

    let mut y_left_hints = AxisHints::new_y()
        .label(subplot.y_axis_title.clone())
        .placement(Placement::LeftBottom);
    if let Some(width) = shared_y_axis_min_thickness {
        y_left_hints = y_left_hints.min_thickness(width);
    }

    // A fixed axis is clamped to its min/max every frame (below) rather than
    // being mouse-zoomable/draggable, so disable those interactions on it.
    let x_interactive = !subplot.x_axis_fixed;
    let y_interactive = !subplot.y_axis_fixed;

    let mut plot = Plot::new(subplot.id)
        .height(subplot.height)
        .allow_zoom([x_interactive, y_interactive])
        .allow_drag([x_interactive, y_interactive])
        .allow_scroll([x_interactive, y_interactive])
        .allow_boxed_zoom(x_interactive && y_interactive)
        .custom_x_axes(vec![x_hints]);

    if link_x_axis {
        plot = plot.link_axis(Id::new("daplot_shared_x_axis"), [true, false]);
    }

    if has_secondary {
        let y_right_hints = AxisHints::new_y()
            .label(subplot.y2_axis_title.clone())
            .placement(Placement::RightTop)
            .formatter(
                move |mark: egui_plot::GridMark, _range: &std::ops::RangeInclusive<f64>| {
                    egui_plot::format_number(remap_primary_to_secondary(mark.value), 4)
                },
            );
        plot = plot.custom_y_axes(vec![y_left_hints, y_right_hints]);
    } else {
        plot = plot.custom_y_axes(vec![y_left_hints]);
    }

    if subplot.show_legend {
        plot = plot.legend(Legend::default());
    }

    let reset_x = subplot.reset_x;
    let reset_y = subplot.reset_y;
    subplot.reset_x = false;
    subplot.reset_y = false;

    let x_axis_min = subplot.x_axis_min;
    let x_axis_max = subplot.x_axis_max;
    let y_axis_min = subplot.y_axis_min;
    let y_axis_max = subplot.y_axis_max;

    let plot_response = plot.show(ui, |plot_ui| {
        if reset_x || reset_y {
            let current = plot_ui.auto_bounds();
            plot_ui.set_auto_bounds([reset_x || current.x, reset_y || current.y]);
        }
        if !x_interactive {
            plot_ui.set_plot_bounds_x(x_axis_min..=x_axis_max);
        }
        if !y_interactive {
            plot_ui.set_plot_bounds_y(y_axis_min..=y_axis_max);
        }
        for p in &prepared {
            let points = PlotPoints::from(p.points.clone());
            match p.chart_type {
                ChartType::Line => {
                    let line = Line::new(p.name.clone(), points)
                        .color(p.color)
                        .width(p.line_width);
                    plot_ui.line(line);
                }
                ChartType::LineMarker => {
                    let line = Line::new(p.name.clone(), points)
                        .color(p.color)
                        .width(p.line_width);
                    plot_ui.line(line);
                    let pts = Points::new(p.name.clone(), PlotPoints::from(p.points.clone()))
                        .color(p.color)
                        .radius(p.marker_radius)
                        .filled(true);
                    plot_ui.points(pts);
                }
                ChartType::Scatter => {
                    let pts = Points::new(p.name.clone(), points)
                        .color(p.color)
                        .radius(p.marker_radius)
                        .filled(true);
                    plot_ui.points(pts);
                }
                ChartType::Bar => {
                    let width = bar_width(&p.points);
                    let bars: Vec<Bar> = p
                        .points
                        .iter()
                        .map(|pt| Bar::new(pt[0], pt[1]).width(width))
                        .collect();
                    let chart = BarChart::new(p.name.clone(), bars).color(p.color);
                    plot_ui.bar_chart(chart);
                }
            }
        }
        plot_ui.plot_bounds()
    });

    let bounds = plot_response.inner;
    subplot.last_x_bounds = (bounds.min()[0], bounds.max()[0]);
    subplot.last_y_bounds = (bounds.min()[1], bounds.max()[1]);

    // Keep the displayed min/max fields tracking the live view for any axis
    // that isn't locked, so they reflect the current pan/zoom rather than
    // going stale until the axis is locked.
    if !subplot.x_axis_fixed {
        (subplot.x_axis_min, subplot.x_axis_max) = subplot.last_x_bounds;
    }
    if !subplot.y_axis_fixed {
        (subplot.y_axis_min, subplot.y_axis_max) = subplot.last_y_bounds;
    }
}

fn min_max_padded(vals: &[f64]) -> (f64, f64) {
    if vals.is_empty() {
        return (0.0, 1.0);
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &v in vals {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }
    if (max - min).abs() < f64::EPSILON {
        min -= 1.0;
        max += 1.0;
    } else {
        let pad = (max - min) * 0.05;
        min -= pad;
        max += pad;
    }
    (min, max)
}

/// Reasonable bar width: a fraction of the smallest gap between consecutive
/// (sorted, deduplicated) x values.
fn bar_width(points: &[[f64; 2]]) -> f64 {
    if points.len() < 2 {
        return 0.8;
    }
    let mut xs: Vec<f64> = points.iter().map(|p| p[0]).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup();
    if xs.len() < 2 {
        return 0.8;
    }
    let mut min_gap = f64::INFINITY;
    for w in xs.windows(2) {
        let gap = w[1] - w[0];
        if gap > 0.0 && gap < min_gap {
            min_gap = gap;
        }
    }
    if !min_gap.is_finite() {
        0.8
    } else {
        min_gap * 0.7
    }
}

fn format_timestamp(secs: f64) -> String {
    use chrono::{TimeZone, Utc};
    if !secs.is_finite() {
        return String::new();
    }
    let whole = secs.floor() as i64;
    match Utc.timestamp_opt(whole, 0) {
        chrono::LocalResult::Single(dt) => {
            // If the visible span is large, showing just the date is
            // cleaner; egui_plot doesn't tell us the zoom level here, so we
            // include time-of-day only when it's non-midnight to reduce
            // clutter for pure-date data.
            if dt.format("%H:%M:%S").to_string() == "00:00:00" {
                dt.format("%Y-%m-%d").to_string()
            } else {
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            }
        }
        _ => String::new(),
    }
}
