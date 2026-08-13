use anyhow::Result;
use clean_my_mac::{
    CandidateKind, CandidateSpec, FullDiskAccess, ScanResult, clean_candidate, default_candidates,
    detect_full_disk_access, display_path, format_bytes, home_dir, open_full_disk_access_settings,
    scan_candidate, volume_usage,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

struct Item {
    spec: CandidateSpec,
    scan: ScanResult,
    selected: bool,
}

struct App {
    home: PathBuf,
    items: Vec<Item>,
    table_state: TableState,
    full_disk_access: FullDiskAccess,
    volume: clean_my_mac::VolumeUsage,
    confirm: bool,
    message: String,
}

impl App {
    fn scan(home: PathBuf) -> Self {
        let specs = default_candidates(&home);
        let items = specs
            .into_iter()
            .map(|spec| {
                let scan = scan_candidate(&spec);
                Item {
                    spec,
                    scan,
                    selected: false,
                }
            })
            .collect();
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self {
            full_disk_access: detect_full_disk_access(&home),
            volume: volume_usage().unwrap_or_default(),
            home,
            items,
            table_state,
            confirm: false,
            message: "扫描完成；所有项目默认未选择。".into(),
        }
    }

    fn rescan(&mut self) {
        for item in &mut self.items {
            item.scan = scan_candidate(&item.spec);
            if !item.scan.accessible() || item.scan.reclaimable_bytes == 0 {
                item.selected = false;
            }
        }
        self.full_disk_access = detect_full_disk_access(&self.home);
        self.volume = volume_usage().unwrap_or_default();
        self.message = "已重新统计。".into();
    }

    fn selected_index(&self) -> usize {
        self.table_state.selected().unwrap_or(0)
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        let current = self.selected_index() as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle(&mut self) {
        let index = self.selected_index();
        let item = &mut self.items[index];
        if !item.spec.kind.selectable() {
            self.message = "该目录是受保护用户数据，程序不会清理。".into();
        } else if !item.scan.accessible() {
            self.message = "无法读取该目录；请检查“完全磁盘访问”权限。".into();
        } else if item.scan.open_handles {
            self.message = "该目录正被进程使用，不能选择。".into();
        } else if item.scan.reclaimable_bytes == 0 {
            self.message = "该项目当前没有可清理内容。".into();
        } else {
            item.selected = !item.selected;
        }
    }

    fn selected_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| item.selected)
            .map(|item| item.scan.reclaimable_bytes)
            .sum()
    }

    fn clean_selected(&mut self) {
        let mut reclaimed = 0_u64;
        let mut failures = Vec::new();
        for item in self.items.iter_mut().filter(|item| item.selected) {
            match clean_candidate(&item.spec, &self.home) {
                Ok(bytes) => reclaimed = reclaimed.saturating_add(bytes),
                Err(error) => failures.push(format!("{}：{error:#}", item.spec.name)),
            }
            item.selected = false;
        }
        self.rescan();
        self.message = if failures.is_empty() {
            format!("清理完成，实际释放约 {}。", format_bytes(reclaimed))
        } else {
            format!(
                "释放约 {}；{} 个项目失败：{}",
                format_bytes(reclaimed),
                failures.len(),
                failures.join(" | ")
            )
        };
    }
}

fn main() -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("clean-my-mac 仅支持 macOS");
    }

    let home = home_dir()?;
    println!("正在以只读方式统计候选目录，请稍候…");
    io::stdout().flush()?;
    let mut app = App::scan(home);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &mut app);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if app.confirm {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    app.confirm = false;
                    app.message = "正在清理，请勿关闭程序…".into();
                    terminal.draw(|frame| draw(frame, app))?;
                    app.clean_selected();
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    app.confirm = false;
                    app.message = "已取消清理。".into();
                }
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
            KeyCode::Char(' ') => app.toggle(),
            KeyCode::Char('r') => {
                app.message = "正在重新统计…".into();
                terminal.draw(|frame| draw(frame, app))?;
                app.rescan();
            }
            KeyCode::Char('o') => match open_full_disk_access_settings() {
                Ok(()) => app.message = "已打开“完全磁盘访问”设置。授权后请重启终端。".into(),
                Err(error) => app.message = format!("无法打开设置：{error:#}"),
            },
            KeyCode::Char('c') => {
                if app.selected_bytes() == 0 {
                    app.message = "请先用空格选择至少一个可清理项目。".into();
                } else {
                    app.confirm = true;
                }
            }
            _ => {}
        }
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    let areas = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(10),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .split(frame.area());

    let permission_style = match app.full_disk_access {
        FullDiskAccess::Granted => Style::default().fg(Color::Green),
        FullDiskAccess::Denied => Style::default().fg(Color::Red),
        FullDiskAccess::Unknown => Style::default().fg(Color::Yellow),
    };
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " clean-my-mac ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  白名单扫描与交互式清理"),
        ]),
        Line::from(vec![
            Span::raw(format!(
                "Data 卷：已用 {} / {}，可用 {}  │  完全磁盘访问：",
                format_bytes(app.volume.used_bytes),
                format_bytes(app.volume.total_bytes),
                format_bytes(app.volume.available_bytes),
            )),
            Span::styled(app.full_disk_access.label(), permission_style),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, areas[0]);

    let rows = app.items.iter().map(|item| {
        let mark = if !item.spec.kind.selectable() {
            "—"
        } else if item.selected {
            "✓"
        } else {
            " "
        };
        let size = if item.scan.reclaimable_bytes > 0 {
            format_bytes(item.scan.reclaimable_bytes)
        } else if item.spec.kind == CandidateKind::Protected {
            format!("{}（保留）", format_bytes(item.scan.allocated_bytes))
        } else if item.scan.open_handles {
            "使用中".into()
        } else if !item.scan.accessible() {
            "无权限".into()
        } else {
            format_bytes(0)
        };
        let style = match item.spec.kind {
            CandidateKind::Recommended => Style::default().fg(Color::Green),
            CandidateKind::Optional => Style::default(),
            CandidateKind::Caution => Style::default().fg(Color::Yellow),
            CandidateKind::Protected => Style::default().fg(Color::DarkGray),
        };
        Row::new(vec![
            Cell::from(mark),
            Cell::from(item.spec.name),
            Cell::from(size),
            Cell::from(item.spec.kind.label()),
            Cell::from(item.spec.description),
        ])
        .style(style)
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(24),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Min(28),
        ],
    )
    .header(
        Row::new(["选", "项目", "可释放/保留", "类型", "说明"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ")
    .block(Block::default().title(" 扫描结果 ").borders(Borders::ALL));
    frame.render_stateful_widget(table, areas[1], &mut app.table_state);

    let selected = app.selected_bytes();
    let selected_item = &app.items[app.selected_index()];
    let detail = Paragraph::new(vec![
        Line::from(format!(
            "当前路径：{}",
            display_path(&selected_item.spec.path, &app.home).to_string_lossy()
        )),
        Line::from(format!(
            "已选预计释放：{}  │  {}",
            format_bytes(selected),
            app.message
        )),
    ])
    .wrap(Wrap { trim: true })
    .block(Block::default().title(" 状态 ").borders(Borders::ALL));
    frame.render_widget(detail, areas[2]);

    let help = Paragraph::new("↑/↓ 或 j/k 移动  Space 勾选  c 清理  r 重扫  o 权限设置  q 退出")
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(help, areas[3]);

    if app.confirm {
        let area = centered_rect(62, 9, frame.area());
        frame.render_widget(Clear, area);
        let warning = Paragraph::new(vec![
            Line::from(Span::styled(
                "确认永久清理？",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!(
                "预计释放 {}。目录本身会保留；删除的内容不会进入废纸篓。",
                format_bytes(app.selected_bytes())
            )),
            Line::from(""),
            Line::from("按 y 确认，按 n 或 Esc 取消。"),
        ])
        .alignment(ratatui::layout::Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::default().title(" 二次确认 ").borders(Borders::ALL));
        frame.render_widget(warning, area);
    }
}

fn centered_rect(width_percent: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height.min(area.height)),
        Constraint::Fill(1),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - width_percent) / 2),
        Constraint::Percentage(width_percent),
        Constraint::Percentage((100 - width_percent) / 2),
    ])
    .split(vertical[1])[1]
}
