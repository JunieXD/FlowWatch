use crate::chart::{PlotSeries, plot_scale_max, prepare_chart, render_plot};
use anyhow::{Context, Result, bail};
use chrono::{Local, TimeZone};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use flowwatch_core::UNKNOWN;
use flowwatch_store::{AppUsage, Database, TrafficSample};
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MINIMUM_WIDTH: u16 = 72;
const MINIMUM_HEIGHT: u16 = 20;
const AUTO_REFRESH: Duration = Duration::from_secs(5);
const DASHBOARD_HELP: &str = "操作：Tab/←→切换 · ↑↓选择 · Enter查看 · r刷新 · Esc关闭 · q退出";

pub struct DashboardRange {
    pub start: i64,
    pub end: i64,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Trend,
    Applications,
    Spikes,
}

impl View {
    const ALL: [Self; 4] = [
        Self::Overview,
        Self::Trend,
        Self::Applications,
        Self::Spikes,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone)]
struct Peak {
    bucket: i64,
    interval_seconds: i64,
    upload: u64,
    download: u64,
}

impl Peak {
    fn total(&self) -> u64 {
        self.upload.saturating_add(self.download)
    }
}

#[derive(Default)]
struct DashboardData {
    traffic: Vec<TrafficSample>,
    apps: Vec<AppUsage>,
    peaks: Vec<Peak>,
    actual_upload: u64,
    actual_download: u64,
    identified_upload: u64,
    identified_download: u64,
    collector_running: bool,
    last_flush_at: Option<i64>,
    loaded_at: i64,
}

impl DashboardData {
    fn load(database_path: &Path, range: &DashboardRange) -> Result<Self> {
        let database = Database::open(database_path)?;
        let traffic = database.query_traffic_samples(range.start, range.end)?;
        let interfaces = database.query_interfaces(range.start, range.end)?;
        let mut apps = database.query_display_apps(range.start, range.end)?;
        apps.sort_by_key(|app| std::cmp::Reverse(app.upload().saturating_add(app.download())));
        let actual_upload = interfaces
            .iter()
            .fold(0u64, |total, row| total.saturating_add(row.upload));
        let actual_download = interfaces
            .iter()
            .fold(0u64, |total, row| total.saturating_add(row.download));
        let (identified_upload, identified_download) = apps
            .iter()
            .filter(|app| {
                app.original_names
                    .iter()
                    .any(|name| name.as_str() != UNKNOWN)
            })
            .fold((0u64, 0u64), |total, app| {
                (
                    total.0.saturating_add(app.upload()),
                    total.1.saturating_add(app.download()),
                )
            });
        let mut peaks = database
            .query_spikes(range.start, range.end)?
            .into_iter()
            .map(|row| Peak {
                bucket: row.bucket,
                interval_seconds: 60,
                upload: row.upload,
                download: row.download,
            })
            .collect::<Vec<_>>();
        if peaks.is_empty() {
            peaks = traffic
                .iter()
                .map(|row| Peak {
                    bucket: row.bucket,
                    interval_seconds: row.interval_seconds,
                    upload: row.upload,
                    download: row.download,
                })
                .collect();
        }
        peaks.sort_by_key(|peak| std::cmp::Reverse(peak.total()));
        peaks.truncate(200);
        let meta = database.meta()?;
        let pid = meta
            .get("collector_pid")
            .and_then(|value| value.parse::<i32>().ok());
        let collector_running = pid.is_some_and(process_is_running);
        let last_flush_at = meta
            .get("last_flush_at")
            .and_then(|value| value.parse::<i64>().ok());
        Ok(Self {
            traffic,
            apps,
            peaks,
            actual_upload,
            actual_download,
            identified_upload,
            identified_download,
            collector_running,
            last_flush_at,
            loaded_at: Local::now().timestamp(),
        })
    }

    fn actual_total(&self) -> u64 {
        self.actual_upload.saturating_add(self.actual_download)
    }

    fn identified_total(&self) -> u64 {
        self.identified_upload
            .saturating_add(self.identified_download)
    }
}

struct App {
    database_path: PathBuf,
    range: DashboardRange,
    data: DashboardData,
    view: View,
    app_table: TableState,
    spike_table: TableState,
    detail_open: bool,
    error: Option<String>,
    last_refresh: Instant,
    palette: Palette,
}

impl App {
    fn new(database_path: &Path, range: DashboardRange, no_color: bool) -> Result<Self> {
        let data = DashboardData::load(database_path, &range)?;
        let mut app = Self {
            database_path: database_path.to_path_buf(),
            range,
            data,
            view: View::Overview,
            app_table: TableState::default(),
            spike_table: TableState::default(),
            detail_open: false,
            error: None,
            last_refresh: Instant::now(),
            palette: Palette::new(no_color),
        };
        app.clamp_selections();
        Ok(app)
    }

    fn refresh(&mut self) {
        match DashboardData::load(&self.database_path, &self.range) {
            Ok(data) => {
                self.data = data;
                self.error = None;
                self.clamp_selections();
            }
            Err(error) => self.error = Some(format!("刷新失败：{error:#}")),
        }
        self.last_refresh = Instant::now();
    }

    fn clamp_selections(&mut self) {
        clamp_selection(&mut self.app_table, self.data.apps.len());
        clamp_selection(&mut self.spike_table, self.data.peaks.len());
    }

    fn move_selection(&mut self, direction: i32) {
        let (state, length) = match self.view {
            View::Applications => (&mut self.app_table, self.data.apps.len()),
            View::Spikes => (&mut self.spike_table, self.data.peaks.len()),
            _ => return,
        };
        if length == 0 {
            state.select(None);
            return;
        }
        let current = state.selected().unwrap_or(0);
        let selected = if direction < 0 {
            current.saturating_sub(1)
        } else {
            current.saturating_add(1).min(length - 1)
        };
        state.select(Some(selected));
    }

    fn open_detail(&mut self) {
        self.detail_open = match self.view {
            View::Applications => self.app_table.selected().is_some(),
            View::Spikes => self.spike_table.selected().is_some(),
            _ => false,
        };
    }
}

#[derive(Clone, Copy)]
struct Palette {
    no_color: bool,
    accent: Color,
    upload: Color,
    download: Color,
    total: Color,
    muted: Color,
    warning: Color,
}

impl Palette {
    fn new(no_color: bool) -> Self {
        if no_color {
            return Self {
                no_color: true,
                accent: Color::Reset,
                upload: Color::Reset,
                download: Color::Reset,
                total: Color::Reset,
                muted: Color::Reset,
                warning: Color::Reset,
            };
        }
        Self {
            no_color: false,
            accent: Color::Cyan,
            upload: Color::Green,
            download: Color::Blue,
            total: Color::Yellow,
            muted: Color::DarkGray,
            warning: Color::Red,
        }
    }

    fn selected_tab_style(self) -> Style {
        if self.no_color {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(Color::Black)
                .bg(self.accent)
                .add_modifier(Modifier::BOLD)
        }
    }
}

enum Action {
    Continue,
    Refresh,
    Quit,
}

pub fn run(database_path: &Path, range: DashboardRange) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!(
            "dashboard 需要交互式终端；可改用 flowwatch report --period 24h、flowwatch chart --period 24h 和 flowwatch apps --period 24h"
        );
    }
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let mut app = App::new(database_path, range, no_color)?;
    let mut guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
    )?;
    terminal.clear()?;

    let result = loop {
        if let Err(error) = terminal.draw(|frame| draw(frame, &mut app)) {
            break Err(error.into());
        }
        if app.last_refresh.elapsed() >= AUTO_REFRESH {
            app.refresh();
        }
        match event::poll(Duration::from_millis(250)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    match handle_key(&mut app, key) {
                        Action::Continue => {}
                        Action::Refresh => app.refresh(),
                        Action::Quit => break Ok(()),
                    }
                }
                Ok(_) => {}
                Err(error) => break Err(error.into()),
            },
            Ok(false) => {}
            Err(error) => break Err(error.into()),
        }
    };
    terminal.show_cursor().ok();
    guard.leave();
    result
}

fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    if app.detail_open {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.detail_open = false,
            KeyCode::Char('q') => return Action::Quit,
            _ => {}
        }
        return Action::Continue;
    }
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Tab | KeyCode::Right => {
            app.view = app.view.next();
            Action::Continue
        }
        KeyCode::BackTab | KeyCode::Left => {
            app.view = app.view.previous();
            Action::Continue
        }
        KeyCode::Up => {
            app.move_selection(-1);
            Action::Continue
        }
        KeyCode::Down => {
            app.move_selection(1);
            Action::Continue
        }
        KeyCode::Enter => {
            app.open_detail();
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    if area.width < MINIMUM_WIDTH || area.height < MINIMUM_HEIGHT {
        let warning = Paragraph::new(format!(
            "终端窗口太小\n\n当前：{} x {}\n需要：至少 {} x {}",
            area.width, area.height, MINIMUM_WIDTH, MINIMUM_HEIGHT
        ))
        .alignment(Alignment::Center)
        .style(Style::default().fg(app.palette.warning))
        .block(Block::default().borders(Borders::ALL).title("FlowWatch"));
        frame.render_widget(warning, area);
        return;
    }
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);
    let titles = ["概览", "趋势", "应用", "异常"]
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let tabs = Tabs::new(titles)
        .select(app.view.index())
        .highlight_style(app.palette.selected_tab_style())
        .divider("  ")
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("FlowWatch · {}", app.range.label)),
        );
    frame.render_widget(tabs, layout[0]);
    match app.view {
        View::Overview => draw_overview(frame, layout[1], app),
        View::Trend => draw_trend(frame, layout[1], app),
        View::Applications => draw_apps(frame, layout[1], app),
        View::Spikes => draw_spikes(frame, layout[1], app),
    }
    let status = if let Some(error) = &app.error {
        Line::from(Span::styled(
            truncate(error, area.width.saturating_sub(1) as usize),
            Style::default().fg(app.palette.warning),
        ))
    } else {
        Line::from(Span::styled(
            format!("数据更新于 {}", format_clock(app.data.loaded_at)),
            Style::default().fg(app.palette.muted),
        ))
    };
    let help = Line::from(Span::styled(
        DASHBOARD_HELP,
        Style::default().fg(app.palette.muted),
    ));
    frame.render_widget(Paragraph::new(vec![status, help]), layout[2]);
    if app.detail_open {
        draw_detail(frame, app);
    }
}

fn draw_overview(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let data = &app.data;
    let actual = data.actual_total();
    let identified = data.identified_total();
    let unidentified = actual.saturating_sub(identified);
    let coverage = (actual > 0).then(|| identified as f64 * 100.0 / actual as f64);
    let peak = data.peaks.first();
    let top_app = data.apps.first();
    let lines = vec![
        Line::from(format!(
            "统计范围  {} 至 {}",
            format_timestamp(app.range.start),
            format_timestamp(app.range.end)
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("采集服务  "),
            Span::styled(
                if data.collector_running {
                    "运行中"
                } else {
                    "未运行"
                },
                Style::default().fg(if data.collector_running {
                    app.palette.upload
                } else {
                    app.palette.warning
                }),
            ),
            Span::raw("    最近保存  "),
            Span::raw(
                data.last_flush_at
                    .map(format_timestamp)
                    .unwrap_or_else(|| "没有记录".to_string()),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("实际流量  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "上传 {}    下载 {}    合计 {}",
                human_bytes(data.actual_upload),
                human_bytes(data.actual_download),
                human_bytes(actual)
            )),
        ]),
        Line::from(vec![
            Span::styled(
                "已识别应用  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "上传 {}    下载 {}    合计 {}",
                human_bytes(data.identified_upload),
                human_bytes(data.identified_download),
                human_bytes(identified)
            )),
        ]),
        Line::from(vec![
            Span::styled(
                "未找到应用  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}    识别率 {}",
                human_bytes(unidentified),
                coverage.map_or_else(|| "无实际流量".to_string(), |value| format!("{value:.1}%"))
            )),
        ]),
        Line::from(""),
        Line::from(format!(
            "主要应用  {}",
            top_app.map_or_else(
                || "没有应用记录".to_string(),
                |row| format!(
                    "{} · {}",
                    display_name(&row.app.name),
                    human_bytes(row.upload().saturating_add(row.download()))
                )
            )
        )),
        Line::from(format!(
            "最高时段  {}",
            peak.map_or_else(
                || "没有流量记录".to_string(),
                |row| format!(
                    "{} · {}",
                    format_timestamp(row.bucket),
                    human_bytes(row.total())
                )
            )
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("所选时间概况")),
        area,
    );
}

fn draw_trend(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.data.traffic.is_empty() {
        draw_empty(frame, area, "所选时间内没有网卡流量记录。", "流量趋势");
        return;
    }
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    let plot_width = inner.width.saturating_sub(12).max(2) as usize;
    let plot_height = inner.height.saturating_sub(3).max(2) as usize;
    let chart = match prepare_chart(
        &app.data.traffic,
        app.range.start,
        app.range.end,
        None,
        plot_width,
    ) {
        Ok(chart) => chart,
        Err(error) => {
            draw_empty(frame, area, &format!("无法绘制趋势图：{error}"), "流量趋势");
            return;
        }
    };
    let block = block.title(format!(
        "流量趋势 · 每{}的用量",
        interval_label(chart.interval_seconds)
    ));
    frame.render_widget(block, area);
    let glyphs = render_plot(&chart, plot_height, plot_width);
    let scale_max = plot_scale_max(&chart);
    let mut lines = Vec::with_capacity(glyphs.len() + 2);
    for (row_index, row) in glyphs.iter().enumerate() {
        let tick = row_index == 0 || row_index == glyphs.len() / 2 || row_index + 1 == glyphs.len();
        let mut spans = if tick {
            let value = scale_max.saturating_mul((glyphs.len() - 1 - row_index) as u64)
                / (glyphs.len().saturating_sub(1).max(1)) as u64;
            vec![Span::raw(format!("{:>9} ┤", human_bytes(value)))]
        } else {
            vec![Span::raw("          │")]
        };
        spans.extend(row.iter().map(|glyph| {
            let color = match glyph.series {
                Some(PlotSeries::Upload) => app.palette.upload,
                Some(PlotSeries::Download) => app.palette.download,
                Some(PlotSeries::Total) => app.palette.total,
                Some(PlotSeries::Overlap) => app.palette.accent,
                None => Color::Reset,
            };
            Span::styled(glyph.symbol.to_string(), Style::default().fg(color))
        }));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(format!("          └{}", "─".repeat(plot_width))));
    lines.push(Line::from(format!(
        "           {}{}{}",
        format_chart_time(app.range.start),
        " ".repeat(plot_width.saturating_sub(22)),
        format_chart_time(app.range.end)
    )));
    lines.push(Line::from(vec![
        Span::styled("─ 上传", Style::default().fg(app.palette.upload)),
        Span::raw("   "),
        Span::styled("┄ 下载", Style::default().fg(app.palette.download)),
        Span::raw("   "),
        Span::styled("━ 合计", Style::default().fg(app.palette.total)),
        Span::raw("   "),
        Span::styled("┼ 交叠", Style::default().fg(app.palette.accent)),
    ]));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_apps(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.data.apps.is_empty() {
        draw_empty(frame, area, "所选时间内没有应用流量记录。", "应用排行");
        return;
    }
    let rows = app.data.apps.iter().map(|usage| {
        Row::new(vec![
            Cell::from(display_name(&usage.app.name).to_string()),
            Cell::from(human_bytes(usage.upload())),
            Cell::from(human_bytes(usage.download())),
            Cell::from(human_bytes(usage.upload().saturating_add(usage.download()))),
            Cell::from(source_label(usage)),
        ])
    });
    let header = Row::new(["应用", "上传", "下载", "合计", "来源"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(36),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(11),
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("› ")
    .block(Block::default().borders(Borders::ALL).title("应用排行"));
    frame.render_stateful_widget(table, area, &mut app.app_table);
}

fn draw_spikes(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.data.peaks.is_empty() {
        draw_empty(frame, area, "所选时间内没有可显示的流量时段。", "异常时段");
        return;
    }
    let rows = app.data.peaks.iter().map(|peak| {
        Row::new(vec![
            Cell::from(format_timestamp(peak.bucket)),
            Cell::from(interval_label(peak.interval_seconds)),
            Cell::from(human_bytes(peak.upload)),
            Cell::from(human_bytes(peak.download)),
            Cell::from(human_bytes(peak.total())),
        ])
    });
    let header = Row::new(["开始时间", "跨度", "上传", "下载", "合计"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(12),
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("› ")
    .block(Block::default().borders(Borders::ALL).title("流量最高时段"));
    frame.render_stateful_widget(table, area, &mut app.spike_table);
}

fn draw_detail(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(78, 70, frame.area());
    frame.render_widget(Clear, area);
    let (title, lines) = match app.view {
        View::Applications => app
            .app_table
            .selected()
            .and_then(|index| app.data.apps.get(index))
            .map(|usage| {
                let mut lines = vec![
                    Line::from(format!("应用 ID：{}", usage.app.id)),
                    Line::from(format!(
                        "上传 {}    下载 {}    合计 {}",
                        human_bytes(usage.upload()),
                        human_bytes(usage.download()),
                        human_bytes(usage.upload().saturating_add(usage.download()))
                    )),
                    Line::from(format!(
                        "来源：{}    连接：{}",
                        source_label(usage),
                        usage.connections
                    )),
                    Line::from(format!(
                        "出现时间：{} 至 {}",
                        format_timestamp(usage.first_seen),
                        format_timestamp(usage.last_seen)
                    )),
                    Line::from(""),
                    Line::from("底层身份与路径"),
                ];
                for id in usage.identity_ids.iter().take(4) {
                    lines.push(Line::from(format!("  {id}")));
                }
                for path in usage.executable_paths.iter().take(4) {
                    lines.push(Line::from(format!("  {path}")));
                }
                (
                    format!("应用详情 · {}", display_name(&usage.app.name)),
                    lines,
                )
            })
            .unwrap_or_else(|| ("应用详情".to_string(), vec![Line::from("没有选中应用")])),
        View::Spikes => app
            .spike_table
            .selected()
            .and_then(|index| app.data.peaks.get(index))
            .map(|peak| {
                (
                    "时段详情".to_string(),
                    vec![
                        Line::from(format!("开始：{}", format_timestamp(peak.bucket))),
                        Line::from(format!("跨度：{}", interval_label(peak.interval_seconds))),
                        Line::from(format!("上传：{}", human_bytes(peak.upload))),
                        Line::from(format!("下载：{}", human_bytes(peak.download))),
                        Line::from(format!("合计：{}", human_bytes(peak.total()))),
                        Line::from(""),
                        Line::from(format!(
                            "flowwatch explain --at \"{}\"",
                            format_timestamp(peak.bucket)
                        )),
                    ],
                )
            })
            .unwrap_or_else(|| ("时段详情".to_string(), vec![Line::from("没有选中时段")])),
        _ => return,
    };
    let detail = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().bg(Color::Reset)),
    );
    frame.render_widget(detail, area);
}

fn draw_empty(frame: &mut Frame<'_>, area: Rect, message: &str, title: &str) {
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn clamp_selection(state: &mut TableState, length: usize) {
    if length == 0 {
        state.select(None);
    } else {
        state.select(Some(state.selected().unwrap_or(0).min(length - 1)));
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn source_label(usage: &AppUsage) -> String {
    let mut values = Vec::new();
    if usage.direct_upload > 0 || usage.direct_download > 0 {
        values.push("直连");
    }
    if usage.clash_upload > 0 || usage.clash_download > 0 {
        values.push("Clash");
    }
    if usage.enhanced_upload > 0 || usage.enhanced_download > 0 {
        values.push("增强");
    }
    if values.is_empty() {
        "未知".to_string()
    } else {
        values.join("+")
    }
}

fn display_name(value: &str) -> &str {
    if value == UNKNOWN {
        "未知应用"
    } else {
        value
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn interval_label(seconds: i64) -> String {
    if seconds < 3_600 {
        format!("{}分钟", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}小时", seconds / 3_600)
    } else {
        format!("{}天", seconds / 86_400)
    }
}

fn format_timestamp(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn format_chart_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn format_clock(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_string();
    }
    value
        .chars()
        .take(maximum.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn process_is_running(pid: i32) -> bool {
    // SAFETY: signal zero checks process existence and does not send a signal.
    pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("无法启用终端交互模式")?;
        let mut guard = Self { active: true };
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            guard.leave();
            return Err(error).context("无法打开终端交互界面");
        }
        Ok(guard)
    }

    fn leave(&mut self) {
        if self.active {
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowwatch_core::AppIdentity;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    fn empty_app(no_color: bool) -> App {
        App {
            database_path: PathBuf::from("/tmp/not-used"),
            range: DashboardRange {
                start: 100,
                end: 200,
                label: "测试范围".into(),
            },
            data: DashboardData::default(),
            view: View::Overview,
            app_table: TableState::default(),
            spike_table: TableState::default(),
            detail_open: false,
            error: None,
            last_refresh: Instant::now(),
            palette: Palette::new(no_color),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_navigation_wraps_and_selection_stays_in_bounds() {
        let mut app = empty_app(false);
        handle_key(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.view, View::Spikes);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Overview);

        app.view = View::Applications;
        app.data.apps = vec![
            AppUsage {
                app: AppIdentity::process("A", "/tmp/A"),
                ..AppUsage::default()
            },
            AppUsage {
                app: AppIdentity::process("B", "/tmp/B"),
                ..AppUsage::default()
            },
        ];
        app.clamp_selections();
        assert_eq!(app.app_table.selected(), Some(0));
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.app_table.selected(), Some(0));
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.app_table.selected(), Some(1));
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.detail_open);
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(!app.detail_open);
    }

    #[test]
    fn empty_and_small_views_render_clear_messages() {
        let mut app = empty_app(false);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let compact = content.replace(' ', "");
        assert!(compact.contains("实际流量"));
        assert!(compact.contains("没有流量记录"));
        assert!(compact.contains("q退出"));
        assert!(UnicodeWidthStr::width(DASHBOARD_HELP) <= MINIMUM_WIDTH as usize);

        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.replace(' ', "").contains("终端窗口太小"));
    }

    #[test]
    fn no_color_palette_has_visible_selected_tab() {
        let palette = Palette::new(true);
        assert_eq!(palette.accent, Color::Reset);
        assert_eq!(palette.warning, Color::Reset);
        assert!(
            palette
                .selected_tab_style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !(palette
                .selected_tab_style()
                .add_modifier
                .contains(Modifier::UNDERLINED))
        );
    }

    #[test]
    fn labels_and_overview_use_plain_chinese() {
        assert_eq!(interval_label(60), "1分钟");

        let mut app = empty_app(false);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .replace(' ', "");

        assert!(content.contains("统计范围"));
        assert!(content.contains("1970-01-0108:01至1970-01-0108:03"));
    }

    #[test]
    fn trend_view_shows_interval_and_line_legend() {
        let mut app = empty_app(true);
        app.view = View::Trend;
        app.data.traffic = vec![
            TrafficSample {
                bucket: 100,
                upload: 10,
                download: 20,
                interval_seconds: 60,
            },
            TrafficSample {
                bucket: 160,
                upload: 20,
                download: 10,
                interval_seconds: 60,
            },
        ];
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .replace(' ', "");

        assert!(content.contains("流量趋势·每1分钟的用量"));
        assert!(content.contains("─上传"));
        assert!(content.contains("┄下载"));
        assert!(content.contains("━合计"));
    }
}
