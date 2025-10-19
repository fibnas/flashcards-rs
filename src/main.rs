use anyhow::{Context, Result};
use chrono::Local;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseEventKind, MouseButton},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::seq::SliceRandom;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap},
    Terminal,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Parser, Debug)]
#[command(version, about = "Terminal flashcards in Rust")]
struct Args {
    #[arg(default_value = "questions.txt")]
    questions: PathBuf,
    #[arg(default_value = "answers.txt")]
    answers: PathBuf,
}

#[derive(Debug, Clone)]
struct FlashCardEngine {
    questions_file: PathBuf,
    answers_file: PathBuf,
    questions: Vec<String>,
    answers: Vec<String>,
    order: Vec<usize>,
    current: usize,
    random: bool,
    responses: BTreeMap<usize, String>,
    seen: BTreeSet<usize>,
}

impl FlashCardEngine {
    fn from_files(q: &PathBuf, a: &PathBuf) -> Result<Self> {
        let questions = read_nonempty_lines(q)?;
        let answers = read_nonempty_lines(a)?;
        if questions.len() != answers.len() {
            anyhow::bail!(
                "Mismatched counts: {} questions vs {} answers",
                questions.len(),
                answers.len()
            );
        }

        let order = (0..questions.len()).collect();
        Ok(Self {
            questions_file: q.clone(),
            answers_file: a.clone(),
            questions,
            answers,
            order,
            current: 0,
            random: false,
            responses: BTreeMap::new(),
            seen: BTreeSet::new(),
        })
    }

    fn set_random(&mut self, mode: bool) {
        self.random = mode;
        self.order = (0..self.questions.len()).collect();
        if mode {
            let mut rng = rand::thread_rng();
            self.order.shuffle(&mut rng);
        }
        self.current = 0;
    }

    fn current_card(&self) -> Option<(usize, &str, &str)> {
        self.order
            .get(self.current)
            .map(|&i| (i, self.questions[i].as_str(), self.answers[i].as_str()))
    }

    fn record(&mut self, idx: usize, resp: String) {
        self.responses.insert(idx, resp);
        self.seen.insert(idx);
    }

    fn next(&mut self) {
        self.current += 1;
    }

    fn done(&self) -> bool {
        self.current >= self.order.len()
    }

    fn progress(&self) -> f64 {
        self.current as f64 / self.order.len().max(1) as f64
    }

    fn save_session(&self) -> Result<PathBuf> {
        let ts = Local::now().format("%Y%m%d-%H%M%S");
        let fname = format!("flashcard_responses_{ts}.txt");
        let mut f = File::create(&fname)?;
        for (i, idx) in self.order.iter().enumerate() {
            writeln!(f, "Q{} (#{})", i + 1, idx + 1)?;
            writeln!(f, "{}\n", self.questions[*idx])?;
            writeln!(f, "Your answer:\n{}", self.responses.get(idx).unwrap_or(&"(none)".into()))?;
            writeln!(f, "\nCorrect:\n{}", self.answers[*idx])?;
            writeln!(f, "\n{}\n", "-".repeat(60))?;
        }
        Ok(PathBuf::from(fname))
    }

    fn persist_edits(&self) -> Result<()> {
        write_atomic(&self.questions_file, &self.questions)?;
        write_atomic(&self.answers_file, &self.answers)?;
        Ok(())
    }
}

fn read_nonempty_lines(path: &PathBuf) -> Result<Vec<String>> {
    let f = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let reader = BufReader::new(f);
    Ok(reader
        .lines()
        .filter_map(|l| l.ok())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn write_atomic(path: &PathBuf, lines: &[String]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp)?;
        for l in lines {
            writeln!(f, "{l}")?;
        }
    }
    fs::rename(tmp, path)?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Mode,
    Ask,
    Reveal,
    Review,
    EditQuestion,
    EditAnswer,
    Done,
}

struct App {
    eng: FlashCardEngine,
    screen: Screen,
    input: String,
    cursor: usize,
    review_scroll: u16,
}

impl App {
    fn new(eng: FlashCardEngine) -> Self {
        Self {
            eng,
            screen: Screen::Mode,
            input: String::new(),
            cursor: 0,
            review_scroll: 0,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let eng = FlashCardEngine::from_files(&args.questions, &args.answers)?;
    let mut app = App::new(eng);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let res = run_app(&mut term, &mut app);

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    term.show_cursor()?;

    if app.screen == Screen::Done && !app.eng.responses.is_empty() {
        if let Ok(p) = app.eng.save_session() {
            eprintln!("Saved session: {}", p.display());
        }
    }

    res
}

fn run_app(term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        term.draw(|f| ui(f, app))?;
        if crossterm::event::poll(Duration::from_millis(100))? {
            let event = event::read()?;
            match event {
                Event::Key(key) => {
                    if handle(app, key)? {
                        break;
                    }
                }
                Event::Mouse(mouse) => {
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        let size = term.size()?;
                        let layout = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([
                                Constraint::Length(1),
                                Constraint::Min(10),
                                Constraint::Length(3),
                                Constraint::Length(3),
                            ])
                            .split(size);
                        if layout[3].contains(Position::new(mouse.column, mouse.row)) {
                            app.screen = Screen::Review;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle(app: &mut App, key: KeyEvent) -> Result<bool> {
    if key.code == KeyCode::Char('q') {
        return Ok(true);
    }

    match app.screen {
        Screen::Mode => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.eng.set_random(true);
                app.screen = Screen::Ask;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.eng.set_random(false);
                app.screen = Screen::Ask;
            }
            _ => {}
        },

        Screen::Ask => match key.code {
            KeyCode::Enter => {
                if let Some((idx, _, _)) = app.eng.current_card() {
                    let resp = std::mem::take(&mut app.input);
                    app.eng.record(idx, resp);
                    app.cursor = 0;
                    app.screen = Screen::Reveal;
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => app.screen = Screen::Review,
            KeyCode::Char(c) => {
                app.input.insert(app.cursor, c);
                app.cursor += 1;
            }
            KeyCode::Backspace => {
                if app.cursor > 0 {
                    app.input.remove(app.cursor - 1);
                    app.cursor -= 1;
                }
            }
            _ => {}
        },

        Screen::Reveal => match key.code {
            KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char('N') => {
                app.eng.next();
                if app.eng.done() {
                    app.screen = Screen::Done;
                } else {
                    app.input.clear();
                    app.screen = Screen::Ask;
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => app.screen = Screen::Review,
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.input.clear();
                if let Some((_i, q, _a)) = app.eng.current_card() {
                    app.input.push_str(q);
                    app.screen = Screen::EditQuestion;
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.input.clear();
                if let Some((_i, _q, a)) = app.eng.current_card() {
                    app.input.push_str(a);
                    app.screen = Screen::EditAnswer;
                }
            }
            _ => {}
        },

        Screen::EditQuestion => match key.code {
            KeyCode::Enter => {
                if let Some((idx, _, _)) = app.eng.current_card() {
                    app.eng.questions[idx] = app.input.clone();
                }
                app.screen = Screen::Reveal;
            }
            KeyCode::Esc => app.screen = Screen::Reveal,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.eng.persist_edits()?;
            }
            KeyCode::Char(c) => app.input.push(c),
            KeyCode::Backspace => {
                app.input.pop();
            }
            _ => {}
        },

        Screen::EditAnswer => match key.code {
            KeyCode::Enter => {
                if let Some((idx, _, _)) = app.eng.current_card() {
                    app.eng.answers[idx] = app.input.clone();
                }
                app.screen = Screen::Reveal;
            }
            KeyCode::Esc => app.screen = Screen::Reveal,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.eng.persist_edits()?;
            }
            KeyCode::Char(c) => app.input.push(c),
            KeyCode::Backspace => {
                app.input.pop();
            }
            _ => {}
        },

        Screen::Review => match key.code {
            KeyCode::Up => app.review_scroll = app.review_scroll.saturating_sub(1),
            KeyCode::Down => app.review_scroll = app.review_scroll.saturating_add(1),
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.screen = if app.eng.done() { Screen::Done } else { Screen::Ask };
            }
            KeyCode::Esc => {
                app.screen = if app.eng.done() { Screen::Done } else { Screen::Ask };
            }
            _ => {}
        },

        Screen::Done => match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => app.screen = Screen::Review,
            _ => {}
        },
    }
    Ok(false)
}

fn ui(f: &mut ratatui::Frame, app: &mut App) {
    let size = f.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(size);

    let title = Paragraph::new("Flashcards • Rust Edition")
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    f.render_widget(title, layout[0]);

    match app.screen {
        Screen::Mode => draw_modal(f, size, "Study in random order? (Y/N)", "Mode Select"),
        Screen::Ask => draw_ask(f, layout[1], app),
        Screen::Reveal => draw_reveal(f, layout[1], app),
        Screen::Review => draw_review(f, layout[1], app),
        Screen::EditQuestion => draw_editor(f, layout[1], app, true),
        Screen::EditAnswer => draw_editor(f, layout[1], app, false),
        Screen::Done => draw_modal(f, size, "Session Complete! 🎯\nR: Review • Q: Quit", "Done"),
    }

    let pct = app.eng.progress();
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black).add_modifier(Modifier::BOLD))
        .ratio(pct)
        .label(Span::styled(format!("{:.0}% ({}/{})", pct * 100.0, app.eng.current, app.eng.order.len()), Style::default().fg(Color::White)));
    f.render_widget(gauge, layout[3]);

    if app.screen == Screen::Ask || matches!(app.screen, Screen::EditQuestion | Screen::EditAnswer) {
        let input = Paragraph::new(app.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Input"))
            .wrap(Wrap { trim: false });
        f.render_widget(input, layout[2]);
        f.set_cursor(layout[2].x + app.cursor as u16 + 1, layout[2].y + 1);
    } else {
        let hint = Paragraph::new("Q: Quit • N: Next • R: Review • E/A: Edit • Ctrl+S: Save")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, layout[2]);
    }
}

fn draw_ask(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Question");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some((_i, q, _a)) = app.eng.current_card() {
        let text = Paragraph::new(q).wrap(Wrap { trim: true }).alignment(Alignment::Left);
        f.render_widget(text, inner);
    }
}

fn draw_reveal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Answer");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some((_i, q, a)) = app.eng.current_card() {
        let lines = vec![
            Line::from(vec![Span::styled("Question: ", Style::default().add_modifier(Modifier::BOLD)), Span::raw(q)]),
            Line::from(""),
            Line::from(vec![Span::styled("Answer: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)), Span::raw(a)]),
            Line::from(""),
            Line::from("Press N: next • E: edit question • A: edit answer"),
        ];
        let para = Paragraph::new(lines).wrap(Wrap { trim: true });
        f.render_widget(para, inner);
    }
}

fn draw_review(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Review");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let text: String = app
        .eng
        .responses
        .keys()
        .map(|i| {
            let q = &app.eng.questions[*i];
            let a = &app.eng.answers[*i];
            let y = app.eng.responses.get(i).map(|s| s.as_str()).unwrap_or("(none)");
            format!("Q#{}\n{}\n\nYou: {}\nCorrect: {}\n{}\n", i + 1, q, y, a, "-".repeat(40))
        })
        .collect();
    let para = Paragraph::new(text).scroll((app.review_scroll, 0)).wrap(Wrap { trim: false });
    f.render_widget(para, inner);
}

fn draw_editor(f: &mut ratatui::Frame, area: Rect, app: &App, editing_question: bool) {
    let title = if editing_question { "Edit Question" } else { "Edit Answer" };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let p = Paragraph::new(app.input.as_str()).wrap(Wrap { trim: false }).alignment(Alignment::Left);
    f.render_widget(p, inner);
    let hint = Paragraph::new("Enter: Save • Esc: Cancel • Ctrl+S: Save to file")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint, Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(2),
        width: area.width,
        height: 1,
    });
}

fn draw_modal(f: &mut ratatui::Frame, size: Rect, msg: &str, title: &str) {
    let area = centered_rect(60, 30, size);
    let block = Block::default().borders(Borders::ALL).title(title);
    let p = Paragraph::new(msg)
        .block(block)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, area);
    f.render_widget(p, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1]);
    h[1]
}

