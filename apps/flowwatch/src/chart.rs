use anyhow::{Result, bail};
use chrono::{Local, TimeZone};
use flowwatch_store::{TrafficSample, day_bucket};
use unicode_width::UnicodeWidthStr;

const SUPPORTED_INTERVALS: [i64; 14] = [
    60, 300, 600, 900, 1_800, 3_600, 10_800, 21_600, 43_200, 86_400, 604_800, 2_592_000, 7_776_000,
    31_536_000,
];
const UPLOAD: u8 = 1;
const DOWNLOAD: u8 = 2;
const TOTAL: u8 = 4;

#[derive(Debug, Clone, Copy, Default)]
struct PlotCell {
    mask: u8,
    directions: [LineDirection; 3],
}

#[derive(Debug, Clone, Copy, Default)]
enum LineDirection {
    #[default]
    Point,
    Horizontal,
    Rising,
    Falling,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotSeries {
    Upload,
    Download,
    Total,
    Overlap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlotGlyph {
    pub symbol: char,
    pub series: Option<PlotSeries>,
}

#[derive(Debug, Clone)]
pub struct TrafficPoint {
    pub bucket: i64,
    pub upload: Option<u64>,
    pub download: Option<u64>,
}

impl TrafficPoint {
    pub fn total(&self) -> Option<u64> {
        Some(self.upload?.saturating_add(self.download?))
    }
}

#[derive(Debug)]
pub struct PreparedChart {
    pub points: Vec<TrafficPoint>,
    pub interval_seconds: i64,
    pub adjusted_for_daily_data: bool,
}

impl PreparedChart {
    pub fn observed_count(&self) -> usize {
        self.points
            .iter()
            .filter(|point| point.upload.is_some() && point.download.is_some())
            .count()
    }

    pub fn totals(&self) -> (u64, u64) {
        self.points.iter().fold((0u64, 0u64), |totals, point| {
            (
                totals.0.saturating_add(point.upload.unwrap_or_default()),
                totals.1.saturating_add(point.download.unwrap_or_default()),
            )
        })
    }

    pub fn peak(&self) -> Option<&TrafficPoint> {
        self.points
            .iter()
            .filter(|point| point.total().is_some())
            .max_by_key(|point| point.total().unwrap_or_default())
    }
}

pub fn prepare_chart(
    samples: &[TrafficSample],
    requested_start: i64,
    requested_end: i64,
    requested_interval: Option<i64>,
    max_points: usize,
) -> Result<PreparedChart> {
    if requested_start >= requested_end {
        bail!("图表开始时间必须早于结束时间");
    }
    if max_points < 2 {
        bail!("图表宽度不足，至少需要两个数据点");
    }

    let sample_resolution = samples
        .iter()
        .map(|sample| sample.interval_seconds)
        .max()
        .unwrap_or(60)
        .max(60);
    let first_sample = samples.iter().map(|sample| sample.bucket).min();
    let start = if requested_start == 0 {
        first_sample.unwrap_or(requested_start)
    } else if sample_resolution >= 86_400 {
        first_sample
            .filter(|bucket| *bucket < requested_start)
            .unwrap_or(requested_start)
    } else {
        requested_start
    };
    let duration = requested_end.saturating_sub(start).max(1);
    let automatic_interval = choose_interval(duration, max_points, sample_resolution);
    let requested = requested_interval.unwrap_or(automatic_interval);
    let interval_seconds = requested.max(sample_resolution);
    let adjusted_for_daily_data = requested_interval.is_some() && requested < sample_resolution;
    let aligned_start = aligned_start(start, interval_seconds);
    let aligned_end = align_up_from(requested_end, aligned_start, interval_seconds);
    let point_count_i64 = aligned_end
        .saturating_sub(aligned_start)
        .checked_div(interval_seconds)
        .unwrap_or_default();
    let point_count = usize::try_from(point_count_i64).unwrap_or(usize::MAX);

    if point_count > max_points {
        let recommended = choose_interval(duration, max_points, sample_resolution);
        bail!(
            "所选范围按每 {}一个点会产生 {} 个点，当前宽度最多显示 {} 个；请改用 --interval {} 或增大 --width",
            interval_label(interval_seconds),
            point_count,
            max_points,
            interval_argument(recommended),
        );
    }

    let mut points: Vec<_> = (0..point_count)
        .map(|index| TrafficPoint {
            bucket: aligned_start.saturating_add((index as i64).saturating_mul(interval_seconds)),
            upload: None,
            download: None,
        })
        .collect();
    for sample in samples {
        let offset = sample.bucket.saturating_sub(aligned_start);
        if offset < 0 {
            continue;
        }
        let index = usize::try_from(offset / interval_seconds).unwrap_or(usize::MAX);
        let Some(point) = points.get_mut(index) else {
            continue;
        };
        point.upload = Some(
            point
                .upload
                .unwrap_or_default()
                .saturating_add(sample.upload),
        );
        point.download = Some(
            point
                .download
                .unwrap_or_default()
                .saturating_add(sample.download),
        );
    }

    Ok(PreparedChart {
        points,
        interval_seconds,
        adjusted_for_daily_data,
    })
}

pub fn render_chart(
    chart: &PreparedChart,
    height: usize,
    plot_width: usize,
    color: bool,
) -> String {
    let height = height.max(2);
    let plot_width = plot_width.max(2);
    let grid = build_grid(chart, height, plot_width);

    let mut output = String::new();
    for (row_index, row) in grid.iter().enumerate() {
        let tick = row_index == 0 || row_index == height / 2 || row_index + 1 == height;
        if tick {
            let value = plot_scale_max(chart).saturating_mul((height - 1 - row_index) as u64)
                / (height - 1) as u64;
            output.push_str(&pad_left(&axis_bytes(value), 9));
            output.push(' ');
            output.push('┤');
        } else {
            output.push_str("          │");
        }
        for cell in row {
            output.push_str(&render_cell(*cell, color));
        }
        output.push('\n');
    }
    output.push_str("          └");
    output.push_str(&"─".repeat(plot_width));
    output.push('\n');
    output.push_str("           ");
    output.push_str(&time_axis(&chart.points, plot_width));
    output
}

pub fn render_plot(chart: &PreparedChart, height: usize, plot_width: usize) -> Vec<Vec<PlotGlyph>> {
    build_grid(chart, height.max(2), plot_width.max(2))
        .iter()
        .map(|row| row.iter().copied().map(cell_glyph).collect())
        .collect()
}

fn build_grid(chart: &PreparedChart, height: usize, plot_width: usize) -> Vec<Vec<PlotCell>> {
    let scale_max = plot_scale_max(chart);
    let mut grid = vec![vec![PlotCell::default(); plot_width]; height];

    draw_series(
        &mut grid,
        &chart.points,
        plot_width,
        scale_max,
        |point| point.upload,
        UPLOAD,
        0,
    );
    draw_series(
        &mut grid,
        &chart.points,
        plot_width,
        scale_max,
        |point| point.download,
        DOWNLOAD,
        1,
    );
    draw_series(
        &mut grid,
        &chart.points,
        plot_width,
        scale_max,
        TrafficPoint::total,
        TOTAL,
        2,
    );
    grid
}

fn chart_maximum(chart: &PreparedChart) -> u64 {
    chart
        .points
        .iter()
        .filter_map(TrafficPoint::total)
        .max()
        .unwrap_or_default()
}

pub fn plot_scale_max(chart: &PreparedChart) -> u64 {
    nice_ceiling(chart_maximum(chart))
}

pub fn legend(color: bool) -> String {
    format!(
        "{} 上传   {} 下载   {} 合计   {} 交叠",
        styled("─", "96", color),
        styled("┄", "94", color),
        styled("━", "93", color),
        styled("┼", "97", color),
    )
}

pub fn interval_label(seconds: i64) -> String {
    match seconds {
        60 => "1 分钟".to_string(),
        value if value < 3_600 => format!("{} 分钟", value / 60),
        value if value < 86_400 => format!("{} 小时", value / 3_600),
        value => format!("{} 天", value / 86_400),
    }
}

fn choose_interval(duration: i64, max_points: usize, minimum: i64) -> i64 {
    let required = duration
        .saturating_add(max_points as i64 - 1)
        .checked_div(max_points as i64)
        .unwrap_or(duration)
        .max(minimum);
    SUPPORTED_INTERVALS
        .into_iter()
        .find(|interval| *interval >= required)
        .unwrap_or(*SUPPORTED_INTERVALS.last().expect("intervals are not empty"))
}

fn interval_argument(seconds: i64) -> String {
    match seconds {
        value if value < 3_600 => format!("{}m", value / 60),
        value if value < 86_400 => format!("{}h", value / 3_600),
        value => format!("{}d", value / 86_400),
    }
}

fn aligned_start(timestamp: i64, interval: i64) -> i64 {
    let local_day = day_bucket(timestamp);
    if interval >= 86_400 {
        local_day
    } else {
        local_day.saturating_add(
            timestamp
                .saturating_sub(local_day)
                .div_euclid(interval)
                .saturating_mul(interval),
        )
    }
}

fn align_up_from(timestamp: i64, anchor: i64, interval: i64) -> i64 {
    let offset = timestamp.saturating_sub(anchor);
    let intervals = offset.div_euclid(interval);
    let down = anchor.saturating_add(intervals.saturating_mul(interval));
    if down >= timestamp {
        down
    } else {
        down.saturating_add(interval)
    }
}

fn draw_series<F>(
    grid: &mut [Vec<PlotCell>],
    points: &[TrafficPoint],
    plot_width: usize,
    scale_max: u64,
    value: F,
    mask: u8,
    series_index: usize,
) where
    F: Fn(&TrafficPoint) -> Option<u64>,
{
    let mut previous: Option<(usize, usize, usize)> = None;
    for (index, point) in points.iter().enumerate() {
        let Some(value) = value(point) else {
            previous = None;
            continue;
        };
        let x = point_x(index, points.len(), plot_width);
        let y = point_y(value, scale_max, grid.len());
        if let Some((previous_index, previous_x, previous_y)) = previous
            && previous_index + 1 == index
        {
            draw_segment(grid, previous_x, previous_y, x, y, mask, series_index);
        } else {
            mark_cell(grid, x, y, mask, series_index, LineDirection::Point);
        }
        previous = Some((index, x, y));
    }
}

fn point_x(index: usize, count: usize, width: usize) -> usize {
    if count <= 1 {
        0
    } else {
        index.saturating_mul(width - 1) / (count - 1)
    }
}

fn point_y(value: u64, scale_max: u64, height: usize) -> usize {
    if scale_max == 0 || height <= 1 {
        return height.saturating_sub(1);
    }
    let scaled = (value as u128)
        .saturating_mul((height - 1) as u128)
        .saturating_add((scale_max / 2) as u128)
        / scale_max as u128;
    (height - 1).saturating_sub(usize::try_from(scaled).unwrap_or(height - 1))
}

fn draw_segment(
    grid: &mut [Vec<PlotCell>],
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    mask: u8,
    series_index: usize,
) {
    let dx = x1.abs_diff(x0);
    let dy = y1.abs_diff(y0);
    let steps = dx.max(dy).max(1);
    let direction = line_direction(x0, y0, x1, y1);
    for step in 0..=steps {
        let ratio = step as f64 / steps as f64;
        let x = (x0 as f64 + (x1 as f64 - x0 as f64) * ratio).round() as usize;
        let y = (y0 as f64 + (y1 as f64 - y0 as f64) * ratio).round() as usize;
        mark_cell(grid, x, y, mask, series_index, direction);
    }
}

fn mark_cell(
    grid: &mut [Vec<PlotCell>],
    x: usize,
    y: usize,
    mask: u8,
    series_index: usize,
    direction: LineDirection,
) {
    if let Some(cell) = grid.get_mut(y).and_then(|row| row.get_mut(x)) {
        cell.mask |= mask;
        if let Some(slot) = cell.directions.get_mut(series_index) {
            *slot = direction;
        }
    }
}

fn line_direction(x0: usize, y0: usize, x1: usize, y1: usize) -> LineDirection {
    match (x0 == x1, y0.cmp(&y1)) {
        (true, _) => LineDirection::Vertical,
        (_, std::cmp::Ordering::Equal) => LineDirection::Horizontal,
        (_, std::cmp::Ordering::Greater) => LineDirection::Rising,
        (_, std::cmp::Ordering::Less) => LineDirection::Falling,
    }
}

fn render_cell(cell: PlotCell, color: bool) -> String {
    let glyph = cell_glyph(cell);
    match glyph.series {
        None => " ".to_string(),
        Some(PlotSeries::Upload) => styled(&glyph.symbol.to_string(), "96", color),
        Some(PlotSeries::Download) => styled(&glyph.symbol.to_string(), "94", color),
        Some(PlotSeries::Total) => styled(&glyph.symbol.to_string(), "93", color),
        Some(PlotSeries::Overlap) => styled(&glyph.symbol.to_string(), "97", color),
    }
}

fn cell_glyph(cell: PlotCell) -> PlotGlyph {
    match cell.mask {
        0 => PlotGlyph {
            symbol: ' ',
            series: None,
        },
        UPLOAD => PlotGlyph {
            symbol: series_symbol(0, cell.directions[0]),
            series: Some(PlotSeries::Upload),
        },
        DOWNLOAD => PlotGlyph {
            symbol: series_symbol(1, cell.directions[1]),
            series: Some(PlotSeries::Download),
        },
        mask if mask & TOTAL != 0 => PlotGlyph {
            symbol: series_symbol(2, cell.directions[2]),
            series: Some(PlotSeries::Total),
        },
        _ => PlotGlyph {
            symbol: overlap_symbol(cell),
            series: Some(PlotSeries::Overlap),
        },
    }
}

fn overlap_symbol(cell: PlotCell) -> char {
    if cell
        .directions
        .iter()
        .any(|direction| matches!(direction, LineDirection::Rising | LineDirection::Falling))
    {
        '╳'
    } else if cell
        .directions
        .iter()
        .any(|direction| matches!(direction, LineDirection::Vertical))
    {
        '╂'
    } else {
        '┼'
    }
}

fn series_symbol(series_index: usize, direction: LineDirection) -> char {
    let (horizontal, vertical, rising, falling, point) = match series_index {
        0 => ('─', '│', '╱', '╲', '●'),
        1 => ('┄', '┆', '╱', '╲', '▪'),
        _ => ('━', '┃', '╱', '╲', '◆'),
    };
    match direction {
        LineDirection::Point => point,
        LineDirection::Horizontal => horizontal,
        LineDirection::Rising => rising,
        LineDirection::Falling => falling,
        LineDirection::Vertical => vertical,
    }
}

fn styled(value: &str, ansi_code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{ansi_code}m{value}\x1b[0m")
    } else {
        value.to_string()
    }
}

fn nice_ceiling(value: u64) -> u64 {
    if value == 0 {
        return 1;
    }
    let mut unit = 1u64;
    while unit <= value / 1024 {
        unit = unit.saturating_mul(1024);
    }
    let scaled = value as f64 / unit as f64;
    let multiplier = [1u64, 2, 5, 10, 20, 50, 100, 200, 500, 1024]
        .into_iter()
        .find(|candidate| *candidate as f64 >= scaled)
        .unwrap_or(1024);
    unit.saturating_mul(multiplier)
}

fn axis_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn pad_left(value: &str, width: usize) -> String {
    format!(
        "{}{value}",
        " ".repeat(width.saturating_sub(UnicodeWidthStr::width(value)))
    )
}

fn time_axis(points: &[TrafficPoint], width: usize) -> String {
    let mut axis = vec![' '; width];
    let Some(first) = points.first() else {
        return axis.into_iter().collect();
    };
    let last = points.last().unwrap_or(first);
    let duration = last.bucket.saturating_sub(first.bucket);
    let start = time_label(first.bucket, duration);
    let end = time_label(last.bucket, duration);
    place_label(&mut axis, 0, &start);
    place_label(&mut axis, width.saturating_sub(end.len()), &end);
    if points.len() >= 3 {
        let middle = &points[points.len() / 2];
        let label = time_label(middle.bucket, duration);
        let position = width
            .saturating_div(2)
            .saturating_sub(label.len().saturating_div(2));
        if position > start.len() + 1 && position + label.len() + 1 < width - end.len() {
            place_label(&mut axis, position, &label);
        }
    }
    axis.into_iter().collect()
}

fn place_label(axis: &mut [char], start: usize, label: &str) {
    for (offset, character) in label.chars().enumerate() {
        if let Some(target) = axis.get_mut(start + offset) {
            *target = character;
        }
    }
}

fn time_label(timestamp: i64, duration: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| {
            if duration <= 172_800 {
                value.format("%m-%d %H:%M").to_string()
            } else {
                value.format("%Y-%m-%d").to_string()
            }
        })
        .unwrap_or_else(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(bucket: i64, upload: u64, download: u64) -> TrafficSample {
        TrafficSample {
            bucket,
            upload,
            download,
            interval_seconds: 60,
        }
    }

    #[test]
    fn auto_interval_keeps_a_day_readable_in_a_narrow_terminal() {
        let chart = prepare_chart(&[], 0, 86_400, None, 68).unwrap();
        assert_eq!(chart.interval_seconds, 1_800);
        assert!(chart.points.len() <= 68);
    }

    #[test]
    fn aggregation_preserves_missing_time_buckets() {
        let chart = prepare_chart(
            &[sample(0, 10, 20), sample(60, 30, 40), sample(600, 50, 60)],
            0,
            900,
            Some(300),
            10,
        )
        .unwrap();
        assert_eq!(chart.points.len(), 3);
        assert_eq!(
            (chart.points[0].upload, chart.points[0].download),
            (Some(40), Some(60))
        );
        assert_eq!(
            (chart.points[1].upload, chart.points[1].download),
            (None, None)
        );
        assert_eq!(
            (chart.points[2].upload, chart.points[2].download),
            (Some(50), Some(60))
        );
        assert_eq!(chart.observed_count(), 2);
        assert_eq!(chart.totals(), (90, 120));
    }

    #[test]
    fn daily_samples_raise_an_explicit_interval() {
        let samples = [TrafficSample {
            bucket: 0,
            upload: 100,
            download: 200,
            interval_seconds: 86_400,
        }];
        let chart = prepare_chart(&samples, 0, 86_400, Some(3_600), 80).unwrap();
        assert_eq!(chart.interval_seconds, 86_400);
        assert!(chart.adjusted_for_daily_data);
    }

    #[test]
    fn explicit_interval_reports_when_the_chart_is_too_dense() {
        let error = prepare_chart(&[], 0, 86_400, Some(60), 80).unwrap_err();
        assert!(error.to_string().contains("请改用 --interval"));
    }

    #[test]
    fn renderer_has_axes_and_distinguishable_series_without_color() {
        let chart = prepare_chart(
            &[sample(0, 10, 20), sample(60, 20, 10), sample(120, 15, 15)],
            0,
            180,
            Some(60),
            20,
        )
        .unwrap();
        let output = render_chart(&chart, 8, 20, false);
        assert!(output.contains('┤'));
        assert!(output.contains('└'));
        assert!(output.contains('─') || output.contains('╱') || output.contains('╲'));
        assert!(legend(false).contains('┄'));
        assert!(legend(false).contains('━'));
        assert!(!output.contains("\x1b["));
        assert_eq!(output.lines().count(), 10);
        assert!(
            output
                .lines()
                .all(|line| UnicodeWidthStr::width(line) == 31)
        );
        assert!(legend(true).contains("\x1b[96m"));
    }

    #[test]
    fn plot_renderer_connects_points_and_marks_overlaps() {
        let chart = PreparedChart {
            points: vec![
                TrafficPoint {
                    bucket: 0,
                    upload: Some(10),
                    download: Some(20),
                },
                TrafficPoint {
                    bucket: 60,
                    upload: Some(20),
                    download: Some(10),
                },
            ],
            interval_seconds: 60,
            adjusted_for_daily_data: false,
        };
        let plot = render_plot(&chart, 8, 20);
        let glyphs = plot.iter().flatten().collect::<Vec<_>>();

        assert!(
            glyphs
                .iter()
                .any(|glyph| matches!(glyph.symbol, '─' | '┄' | '━' | '╱' | '╲' | '│' | '┆' | '┃'))
        );
        let overlap = cell_glyph(PlotCell {
            mask: UPLOAD | DOWNLOAD,
            directions: [
                LineDirection::Horizontal,
                LineDirection::Horizontal,
                LineDirection::Point,
            ],
        });
        assert_eq!(overlap.series, Some(PlotSeries::Overlap));
        assert_eq!(overlap.symbol, '┼');

        let total_wins_collision = cell_glyph(PlotCell {
            mask: UPLOAD | DOWNLOAD | TOTAL,
            directions: [LineDirection::Point; 3],
        });
        assert_eq!(total_wins_collision.series, Some(PlotSeries::Total));
    }
}
