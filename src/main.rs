use color_eyre::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols,
    text::Line,
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    },
};
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;
use walkdir::WalkDir;

/*
Gaurav Sablok
codeprog@icloud.com
*/

#[derive(Default)]
struct App {
    picker_open: bool,
    picker_path: PathBuf,
    picker_entries: Vec<PathBuf>,
    picker_state: ListState,
    table_rows: Vec<Vec<String>>,
    table_state: TableState,
    table_scroll: (u16, u16),
    search_open: bool,
    search_input: String,
    search_results: Vec<usize>,

    loader_tx: Option<Sender<LoaderMsg>>,
    loader_rx: Option<Receiver<LoaderMsg>>,
}

enum LoaderMsg {
    Files(Vec<PathBuf>),
    SamRows(Vec<Vec<String>>),
    Quit,
}

impl App {
    fn new() -> Self {
        let mut s = App {
            picker_path: std::env::current_dir().unwrap(),
            search_input: String::new(),
            search_results: Vec::new(),
            ..Default::default()
        };
        s.picker_state.select(Some(0));
        s.table_state.select(Some(0));
        s.spawn_loader();
        s
    }

    fn spawn_loader(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.loader_tx = Some(tx);
        self.loader_rx = Some(rx);
    }

    fn send(&self, msg: LoaderMsg) {
        if let Some(tx) = &self.loader_tx {
            let _ = tx.send(msg);
        }
    }

    fn recv(&mut self) {
        if let Some(rx) = &self.loader_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    LoaderMsg::Files(list) => {
                        self.picker_entries = list;
                        self.picker_state.select(Some(0));
                    }
                    LoaderMsg::SamRows(rows) => {
                        self.table_rows = rows;
                        self.table_state.select(Some(0));
                        self.table_scroll = (0, 0);
                        self.search_results.clear(); // clear old search
                    }
                    LoaderMsg::Quit => {}
                }
            }
        }
    }

    fn refresh_picker(&mut self) {
        let path = self.picker_path.clone();
        let tx = self.loader_tx.clone().unwrap();
        thread::spawn(move || {
            let mut entries: Vec<PathBuf> = vec![];

            if let Some(parent) = path.parent() {
                entries.push(parent.to_path_buf());
            }

            for entry in WalkDir::new(&path)
                .max_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let p = entry.path().to_path_buf();
                if p.is_dir()
                    || p.extension()
                        .map(|e| e == "sam" || e == "bam")
                        .unwrap_or(false)
                {
                    entries.push(p);
                }
            }
            entries.sort_by_key(|p| (p.is_file(), p.to_str().map(|s| s.to_lowercase())));
            let _ = tx.send(LoaderMsg::Files(entries));
        });
    }

    fn load_sam(&mut self, path: PathBuf) {
        let tx = self.loader_tx.clone().unwrap();
        thread::spawn(move || {
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(_) => return,
            };
            let reader = BufReader::new(file);
            let mut rows = vec![];

            for line in reader.lines().flatten() {
                if line.starts_with('@') {
                    continue;
                }
                let fields: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();
                if fields.len() >= 11 {
                    rows.push(fields);
                }
            }
            let _ = tx.send(LoaderMsg::SamRows(rows));
        });
    }

    fn perform_search(&mut self) {
        let needle = self.search_input.trim();
        if needle.is_empty() {
            self.search_results.clear();
            return;
        }

        self.search_results = self
            .table_rows
            .iter()
            .enumerate()
            .filter_map(|(i, fields)| {
                if fields.get(0).map(|q| q.contains(needle)).unwrap_or(false) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        if let Some(&first) = self.search_results.first() {
            self.table_state.select(Some(first));
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new();
    app.refresh_picker();

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => break,

                    // Open search modal
                    KeyCode::Char('/') if !app.picker_open && !app.search_open => {
                        app.search_open = true;
                        app.search_input.clear();
                    }

                    KeyCode::Tab => {
                        app.picker_open = !app.picker_open;
                        if app.picker_open {
                            app.refresh_picker();
                        }
                    }

                    _ if app.picker_open => match key.code {
                        KeyCode::Esc => app.picker_open = false,
                        KeyCode::Up => {
                            let i = app.picker_state.selected().unwrap_or(0);
                            let i = i.saturating_sub(1);
                            app.picker_state.select(Some(i));
                        }
                        KeyCode::Down => {
                            let i = app.picker_state.selected().unwrap_or(0);
                            let len = app.picker_entries.len();
                            let i = if i + 1 >= len { 0 } else { i + 1 };
                            app.picker_state.select(Some(i));
                        }
                        KeyCode::Enter => {
                            if let Some(idx) = app.picker_state.selected() {
                                let selected = &app.picker_entries[idx];
                                if selected.is_dir() {
                                    app.picker_path = selected.clone();
                                    app.refresh_picker();
                                } else {
                                    app.picker_open = false;
                                    app.load_sam(selected.clone());
                                }
                            }
                        }
                        _ => {}
                    },

                    _ if !app.picker_open => match key.code {
                        // Search modal handling
                        _ if app.search_open => match key.code {
                            KeyCode::Esc => app.search_open = false,
                            KeyCode::Enter => {
                                app.perform_search();
                                app.search_open = false;
                            }
                            KeyCode::Backspace => {
                                app.search_input.pop();
                            }
                            KeyCode::Char(c) => {
                                app.search_input.push(c);
                            }
                            _ => {}
                        },

                        KeyCode::Up => {
                            let i = app.table_state.selected().unwrap_or(0);
                            app.table_state.select(Some(i.saturating_sub(1)));
                        }
                        KeyCode::Down => {
                            let i = app.table_state.selected().unwrap_or(0);
                            let max = app.table_rows.len().saturating_sub(1);
                            let i = if i >= max { max } else { i + 1 };
                            app.table_state.select(Some(i));
                        }
                        KeyCode::Left => {
                            let (h, _) = app.table_scroll;
                            app.table_scroll.0 = h.saturating_sub(5);
                        }
                        KeyCode::Right => {
                            app.table_scroll.0 = app.table_scroll.0.saturating_add(5);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            app.recv();
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn ui(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();

    // Main table
    if !app.table_rows.is_empty() {
        let header_cells = [
            "QNAME", "FLAG", "RNAME", "POS", "MAPQ", "CIGAR", "RNEXT", "PNEXT", "TLEN", "SEQ",
            "QUAL",
        ]
        .iter()
        .map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        });

        let header = Row::new(header_cells)
            .style(Style::default().bg(Color::DarkGray))
            .height(1);

        let rows: Vec<Row> = app
            .table_rows
            .iter()
            .enumerate()
            .map(|(i, fields)| {
                let style = if app.search_results.contains(&i) {
                    Style::default().bg(Color::LightGreen)
                } else {
                    Style::default()
                };
                Row::new(fields.iter().take(11).map(|s| Cell::from(s.clone())))
                    .style(style)
                    .height(1)
            })
            .collect();

        let widths = (0..11).map(|_| Constraint::Length(12)).collect::<Vec<_>>();

        let table = Table::new(rows, widths)
            .header(header)
            .block(
                Block::default()
                    .title(format!("SAM – {} rows", app.table_rows.len()))
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().bg(Color::LightBlue))
            .highlight_symbol(">> ")
            .column_spacing(1);

        let mut table_state = app.table_state.clone();
        f.render_stateful_widget(table, area, &mut table_state);

        // Info bar
        let info = format!(
            "Row {}/{}  H-scroll: {}  {} match(es)",
            app.table_state.selected().map(|s| s + 1).unwrap_or(0),
            app.table_rows.len(),
            app.table_scroll.0,
            app.search_results.len()
        );
        let info_par = Paragraph::new(info).style(Style::default().fg(Color::Cyan));
        let info_area = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(area)[0];
        f.render_widget(
            info_par,
            Rect {
                y: info_area.y,
                ..info_area
            },
        );
    } else {
        let placeholder = Paragraph::new("No file loaded – press <Tab> to open file picker")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title("SAM Viewer"));
        f.render_widget(placeholder, area);
    }

    // File picker modal
    if app.picker_open {
        let popup_area = centered_rect(70, 70, area);
        f.render_widget(Clear, popup_area);

        let inner = popup_area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        let title = Block::default()
            .title(format!("File Picker – {}", app.picker_path.display()))
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::DarkGray));

        let list_items: Vec<ListItem> = app
            .picker_entries
            .iter()
            .map(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                let prefix = if p.is_dir() { "[DIR] " } else { "      " };
                ListItem::new(Line::from(format!("{}{}", prefix, name)))
            })
            .collect();

        let list = List::new(list_items)
            .block(title)
            .highlight_style(Style::default().bg(Color::Yellow))
            .highlight_symbol(symbols::block::FULL);

        let mut list_state = app.picker_state.clone();
        f.render_stateful_widget(list, inner, &mut list_state);
    }

    // Search modal
    if app.search_open {
        let popup = centered_rect(60, 20, area);
        f.render_widget(Clear, popup);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(popup);

        let input = Paragraph::new(format!("QNAME: {}", app.search_input))
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Search QNAME (Enter to confirm, Esc to cancel)"),
            );
        f.render_widget(input, chunks[0]);

        // Cursor position
        let cursor_x = chunks[0].x + 8 + UnicodeWidthStr::width(app.search_input.as_str()) as u16;
        let cursor_y = chunks[0].y + 1;
        f.set_cursor_position((cursor_x, cursor_y));

        // Live result preview
        if !app.search_input.trim().is_empty() {
            let preview_text = if app.search_results.is_empty() {
                "No matches yet...".to_string()
            } else {
                format!(
                    "Found {} match(es). First → row {}",
                    app.search_results.len(),
                    app.search_results[0] + 1
                )
            };
            let preview = Paragraph::new(preview_text).style(Style::default().fg(Color::Green));
            f.render_widget(preview, chunks[1]);
        }
    }
}
