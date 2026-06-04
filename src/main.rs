use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::Deserialize;
use std::env;
use std::io;
use std::time::Duration;
use std::{collections::BTreeMap, process::Command, time::Instant};

use arboard::Clipboard;
use textwrap::wrap;

const WRAP_WIDTH: usize = 60;

#[derive(Debug, Deserialize)]
struct Package {
    #[serde(default)]
    pname: String,

    #[serde(default)]
    version: String,

    #[serde(default)]
    description: String,
}

type SearchResults = BTreeMap<String, Package>;

struct App {
    results: SearchResults,
    scroll: u16,
    selected: usize,
    status_message: String,
}

impl App {
    fn scroll_for_index(&self, index: usize) -> u16 {
        let mut offset: u16 = 0;
        for (i, (_attr, pkg)) in self.results.iter().enumerate() {
            if i == index {
                break;
            }
            let desc_lines = wrap(&pkg.description, WRAP_WIDTH).len().max(1);
            offset += 1 + 1 + desc_lines as u16 + 1;
        }
        offset
    }
}

fn search_packages(query: &str) -> Result<SearchResults, Box<dyn std::error::Error>> {
    let output = Command::new("nix")
        .args(["search", "nixpkgs", query, "--json"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "nix search failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let package: SearchResults = serde_json::from_slice(&output.stdout)?;
    Ok(package)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <query>", args[0]);
        std::process::exit(1);
    }

    let query = &args[1];
    let start = Instant::now();
    let search_results = search_packages(query)?;
    let duration = start.elapsed();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App {
        results: search_results,
        scroll: 0,
        selected: 0,
        status_message: String::new(),
    };

    let result = run_app(&mut terminal, &mut app, duration.as_secs_f64());

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    duration: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cb = Clipboard::new().unwrap();

    loop {
        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(1),
                ])
                .split(area);

            let header_text = format!(
                "{} results fetched in {:.2} seconds",
                app.results.len(),
                duration
            );
            let header = Paragraph::new(header_text)
                .block(Block::default().borders(Borders::ALL).title("nixpkgs"))
                .style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_widget(header, chunks[0]);

            let mut lines: Vec<Line> = Vec::new();
            if app.results.is_empty() {
                lines.push(Line::from(Span::styled(
                    "no packages found for this query. maybe add one?",
                    Style::default().fg(Color::DarkGray),
                )));
            }

            for (i, (_attr, pkg)) in app.results.iter().enumerate() {
                let package_style = if i == app.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                };

                lines.push(Line::from(Span::styled(pkg.pname.as_str(), package_style)));
                lines.push(Line::from(Span::styled(
                    format!("v{}", pkg.version),
                    Style::default().fg(Color::DarkGray),
                )));
                for line in wrap(&pkg.description, WRAP_WIDTH) {
                    lines.push(Line::from(line.to_string()));
                }
                lines.push(Line::from(""));
            }

            let results = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title("Results"))
                .wrap(Wrap { trim: false })
                .scroll((app.scroll, 0));
            f.render_widget(results, chunks[1]);

            let help = Paragraph::new(format!(
                "j/k scroll • y copy package • q quit • {}",
                app.status_message
            ))
            .style(Style::default().fg(Color::DarkGray));
            f.render_widget(help, chunks[2]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,

                        KeyCode::Down | KeyCode::Char('j') => {
                            if app.selected + 1 < app.results.len() {
                                app.selected += 1;
                                app.scroll = app.scroll_for_index(app.selected);
                            }
                        }

                        KeyCode::Up | KeyCode::Char('k') => {
                            app.selected = app.selected.saturating_sub(1);
                            app.scroll = app.scroll_for_index(app.selected);
                        }

                        KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
                        KeyCode::PageUp => app.scroll = app.scroll.saturating_sub(10),

                        KeyCode::Char('y') => {
                            let packages: Vec<_> = app.results.iter().collect();
                            if let Some((attr, _pkg)) = packages.get(app.selected) {
                                let text = format!("nixpkgs#{}", attr);
                                if cb.set_text(text.clone()).is_ok() {
                                    app.status_message = format!("Copied {}", text);
                                } else {
                                    app.status_message = "Clipboard error".to_string();
                                }
                            }
                        }

                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
