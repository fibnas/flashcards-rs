use anyhow::{Context, Result};
use chrono::Local;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEventKind,
    },
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
    fs::{self, create_dir_all, File},
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    time::Duration,
};



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
            writeln!(
                f,
                "Your answer:\n{}",
                self.responses.get(idx).unwrap_or(&"(none)".into())
            )?;
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
    TopicSelect,
    TopicCreate,
    MainMenu,
    CardList,
    ConfirmQuit,
}

struct App {
    eng: Option<FlashCardEngine>,
    screen: Screen,
    input: String,
    cursor: usize,
    review_scroll: u16,
    topics: Vec<String>,
    selected_topic: usize,
    topic_input: String,
    current_topic: Option<String>,
    in_edit_mode: bool,
    selected_card: usize,
    prev_screen: Option<Screen>,
}

impl App {
    fn new() -> Self {
        Self {
            eng: None,
            screen: Screen::TopicSelect,
            input: String::new(),
            cursor: 0,
            review_scroll: 0,
            topics: Vec::new(),
            selected_topic: 0,
            topic_input: String::new(),
            current_topic: None,
            in_edit_mode: false,
            selected_card: 0,
            prev_screen: None,
        }
    }

    fn load_topics(&mut self) -> Result<()> {
        create_dir_all("topics")?;
        self.topics = fs::read_dir("topics")?
            .filter_map(|res| res.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        self.topics.sort();
        Ok(())
    }

    fn load_eng(&mut self, topic: &str) -> Result<()> {
        let path = PathBuf::from("topics").join(topic);
        create_dir_all(&path)?;
        let q = path.join("questions.txt");
        let a = path.join("answers.txt");
        if !q.exists() {
            File::create(&q)?;
        }
        if !a.exists() {
            File::create(&a)?;
        }
        self.eng = Some(FlashCardEngine::from_files(&q, &a)?);
        self.current_topic = Some(topic.to_string());
        Ok(())
    }
}

fn main() -> Result<()> {
    let mut app = App::new();
    app.load_topics()?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let res = run_app(&mut term, &mut app);

    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    term.show_cursor()?;

    if app.screen == Screen::Done {
        if let Some(eng) = &app.eng {
            if !eng.responses.is_empty() {
                if let Ok(p) = eng.save_session() {
                    eprintln!("Saved session: {}", p.display());
                }
            }
        }
    }

    res
}

fn run_app(
    term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
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
    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.prev_screen = Some(app.screen);
        app.screen = Screen::ConfirmQuit;
    }

    match app.screen {
        Screen::TopicSelect => match key.code {
            KeyCode::Up => app.selected_topic = app.selected_topic.saturating_sub(1),
            KeyCode::Down => {
                app.selected_topic =
                    (app.selected_topic + 1).min(app.topics.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if !app.topics.is_empty() {
                    let topic = app.topics[app.selected_topic].clone();
                    app.load_eng(&topic)?;
                    app.screen = Screen::MainMenu;
                }
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                app.topic_input.clear();
                app.cursor = 0;
                app.screen = Screen::TopicCreate;
            }
            _ => {}
        },

        Screen::TopicCreate => match key.code {
            KeyCode::Enter => {
                let name = std::mem::take(&mut app.topic_input);
                if !name.is_empty() {
                    app.load_eng(&name)?;
                    app.topics.push(name);
                    app.topics.sort();
                    app.screen = Screen::MainMenu;
                } else {
                    app.screen = Screen::TopicSelect;
                }
            }
            KeyCode::Esc => app.screen = Screen::TopicSelect,
            KeyCode::Char(c) => {
                app.topic_input.insert(app.cursor, c);
                app.cursor += 1;
            }
            KeyCode::Backspace => {
                if app.cursor > 0 {
                    app.topic_input.remove(app.cursor - 1);
                    app.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if app.cursor < app.topic_input.len() {
                    app.topic_input.remove(app.cursor);
                }
            }
            KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
            KeyCode::Right => app.cursor = (app.cursor + 1).min(app.topic_input.len()),
            _ => {}
        },

        Screen::MainMenu => match key.code {
            KeyCode::Char('s') | KeyCode::Char('S') => {
                app.in_edit_mode = false;
                app.screen = Screen::Mode;
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                app.in_edit_mode = true;
                app.screen = Screen::CardList;
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                app.eng = None;
                app.current_topic = None;
                app.screen = Screen::TopicSelect;
            }
            _ => {}
        },

        Screen::CardList => match key.code {
            KeyCode::Up => app.selected_card = app.selected_card.saturating_sub(1),
            KeyCode::Down => {
                if let Some(eng) = &app.eng {
                    app.selected_card =
                        (app.selected_card + 1).min(eng.questions.len().saturating_sub(1))
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if let Some(eng) = &mut app.eng {
                    if app.selected_card < eng.questions.len() {
                        eng.current = app.selected_card;
                        app.input = eng.questions[app.selected_card].clone();
                        app.cursor = app.input.len();
                        app.screen = Screen::EditQuestion;
                    }
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if let Some(eng) = &mut app.eng {
                    if app.selected_card < eng.questions.len() {
                        eng.current = app.selected_card;
                        app.input = eng.answers[app.selected_card].clone();
                        app.cursor = app.input.len();
                        app.screen = Screen::EditAnswer;
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if let Some(eng) = &mut app.eng {
                    eng.questions.push(String::new());
                    eng.answers.push(String::new());
                    eng.order = (0..eng.questions.len()).collect();
                    let new_idx = eng.questions.len() - 1;
                    eng.current = new_idx;
                    app.selected_card = new_idx;
                    app.input.clear();
                    app.cursor = 0;
                    app.screen = Screen::EditQuestion;
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if let Some(eng) = &mut app.eng {
                    if app.selected_card < eng.questions.len() {
                        eng.questions.remove(app.selected_card);
                        eng.answers.remove(app.selected_card);
                        eng.order = (0..eng.questions.len()).collect();
                    }
                    if app.selected_card >= eng.questions.len() {
                        app.selected_card = eng.questions.len().saturating_sub(1);
                    }
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if let Some(eng) = &mut app.eng {
                    eng.persist_edits()?;
                }
            }
            KeyCode::Char('b') | KeyCode::Char('B') => app.screen = Screen::MainMenu,
            _ => {}
        },

        Screen::Mode => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(eng) = &mut app.eng {
                    eng.set_random(true);
                    app.screen = Screen::Ask;
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if let Some(eng) = &mut app.eng {
                    eng.set_random(false);
                    app.screen = Screen::Ask;
                }
            }
            _ => {}
        },

        Screen::Ask => match key.code {
            KeyCode::Enter => {
                if let Some(eng) = &mut app.eng {
                    if let Some((idx, _, _)) = eng.current_card() {
                        let resp = std::mem::take(&mut app.input);
                        eng.record(idx, resp);
                        app.cursor = 0;
                        app.screen = Screen::Reveal;
                    }
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.screen = Screen::Review
            }
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
            KeyCode::Delete => {
                if app.cursor < app.input.len() {
                    app.input.remove(app.cursor);
                }
            }
            KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
            KeyCode::Right => app.cursor = (app.cursor + 1).min(app.input.len()),
            _ => {}
        },

        Screen::Reveal => match key.code {
            KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char('N') => {
                if let Some(eng) = &mut app.eng {
                    eng.next();
                    if eng.done() {
                        app.screen = Screen::Done;
                    } else {
                        app.input.clear();
                        app.screen = Screen::Ask;
                    }
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.screen = Screen::Review
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(eng) = &mut app.eng {
                    if let Some((_, q, _)) = eng.current_card() {
                        app.input = q.to_string();
                        app.cursor = app.input.len();
                        app.screen = Screen::EditQuestion;
                    }
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(eng) = &mut app.eng {
                    if let Some((_, _, a)) = eng.current_card() {
                        app.input = a.to_string();
                        app.cursor = app.input.len();
                        app.screen = Screen::EditAnswer;
                    }
                }
            }
            _ => {}
        },

        Screen::EditQuestion => match key.code {
            KeyCode::Enter => {
                if let Some(eng) = &mut app.eng {
                    let idx = eng.current;
                    eng.questions[idx] = app.input.clone();
                }
                app.screen = if app.in_edit_mode {
                    Screen::CardList
                } else {
                    Screen::Reveal
                };
            }
            KeyCode::Esc => {
                app.screen = if app.in_edit_mode {
                    Screen::CardList
                } else {
                    Screen::Reveal
                }
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(eng) = &mut app.eng {
                    eng.persist_edits()?;
                }
            }
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
            KeyCode::Delete => {
                if app.cursor < app.input.len() {
                    app.input.remove(app.cursor);
                }
            }
            KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
            KeyCode::Right => app.cursor = (app.cursor + 1).min(app.input.len()),
            _ => {}
        },

        Screen::EditAnswer => match key.code {
            KeyCode::Enter => {
                if let Some(eng) = &mut app.eng {
                    let idx = eng.current;
                    eng.answers[idx] = app.input.clone();
                }
                app.screen = if app.in_edit_mode {
                    Screen::CardList
                } else {
                    Screen::Reveal
                };
            }
            KeyCode::Esc => {
                app.screen = if app.in_edit_mode {
                    Screen::CardList
                } else {
                    Screen::Reveal
                }
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(eng) = &mut app.eng {
                    eng.persist_edits()?;
                }
            }
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
            KeyCode::Delete => {
                if app.cursor < app.input.len() {
                    app.input.remove(app.cursor);
                }
            }
            KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
            KeyCode::Right => app.cursor = (app.cursor + 1).min(app.input.len()),
            _ => {}
        },

        Screen::Review => match key.code {
            KeyCode::Up => app.review_scroll = app.review_scroll.saturating_sub(1),
            KeyCode::Down => app.review_scroll = app.review_scroll.saturating_add(1),
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.screen = if let Some(eng) = &app.eng {
                    if eng.done() {
                        Screen::Done
                    } else {
                        Screen::Ask
                    }
                } else {
                    Screen::Ask
                };
            }
            KeyCode::Esc => {
                app.screen = if let Some(eng) = &app.eng {
                    if eng.done() {
                        Screen::Done
                    } else {
                        Screen::Ask
                    }
                } else {
                    Screen::Ask
                };
            }
            _ => {}
        },

        Screen::Done => match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => app.screen = Screen::Review,
            _ => {}
        },

        Screen::ConfirmQuit => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if let Some(prev) = app.prev_screen {
                    app.screen = prev;
                }
                app.prev_screen = None;
            }
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
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    f.render_widget(title, layout[0]);

    match app.screen {
        Screen::TopicSelect => {
            let block = Block::default().borders(Borders::ALL).title("Select Topic");
            let inner = block.inner(layout[1]);
            f.render_widget(block, layout[1]);
            let text: Vec<Line> = if app.topics.is_empty() {
                vec![Line::from(Span::raw("No topics yet. Press C to create."))]
            } else {
                app.topics
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let style = if i == app.selected_topic {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        Line::from(Span::styled(t.clone(), style))
                    })
                    .collect()
            };
            let para = Paragraph::new(text)
                .alignment(Alignment::Left)
                .wrap(Wrap::default());
            f.render_widget(para, inner);
        }
        Screen::TopicCreate => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title("Create New Topic");
            let inner = block.inner(layout[1]);
            f.render_widget(block, layout[1]);
            let hint = Paragraph::new("Enter topic name, then press Enter")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(hint, inner);
        }
        Screen::MainMenu => {
            let msg = if let Some(topic) = &app.current_topic {
                format!(
                    "Selected topic: {}\n\nS: Start Quiz\nE: Edit Cards\nB: Back to topics",
                    topic
                )
            } else {
                "Error: No topic selected".to_string()
            };
            draw_modal(f, size, &msg, "Main Menu");
        }
        Screen::CardList => {
            let block = Block::default().borders(Borders::ALL).title("Edit Cards");
            let inner = block.inner(layout[1]);
            f.render_widget(block, layout[1]);
            let text: Vec<Line> = if let Some(eng) = &app.eng {
                eng.questions
                    .iter()
                    .enumerate()
                    .map(|(i, q)| {
                        let style = if i == app.selected_card {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        Line::from(Span::styled(format!("Card {}: {}", i + 1, q), style))
                    })
                    .collect()
            } else {
                vec![]
            };
            let para = Paragraph::new(text)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true });
            f.render_widget(para, inner);
        }
        Screen::Mode => draw_modal(f, size, "Study in random order? (Y/N)", "Mode Select"),
        Screen::Ask => draw_ask(f, layout[1], app),
        Screen::Reveal => draw_reveal(f, layout[1], app),
        Screen::Review => draw_review(f, layout[1], app),
        Screen::EditQuestion => draw_editor(f, layout[1], app, true),
        Screen::EditAnswer => draw_editor(f, layout[1], app, false),
        Screen::Done => draw_modal(f, size, "Session Complete! 🎯\nR: Review • Ctrl+Q: Quit", "Done"),
        Screen::ConfirmQuit => draw_modal(f, size, "Are you sure you want to exit? (Y/N)", "Confirm Exit"),
    }

    let (pct, cur, total) = if let Some(eng) = &app.eng {
        (eng.progress(), eng.current, eng.order.len())
    } else {
        (0.0, 0, 0)
    };
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .ratio(pct)
        .label(Span::styled(
            format!("{:.0}% ({}/{})", pct * 100.0, cur, total),
            Style::default().fg(Color::White),
        ));
    f.render_widget(gauge, layout[3]);

    if app.screen == Screen::Ask
        || app.screen == Screen::TopicCreate
        || matches!(app.screen, Screen::EditQuestion | Screen::EditAnswer)
    {
        let title = if app.screen == Screen::TopicCreate {
            "Topic Name"
        } else {
            "Input"
        };
        let input = Paragraph::new(app.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });
        f.render_widget(input, layout[2]);
        f.set_cursor(layout[2].x + app.cursor as u16 + 1, layout[2].y + 1);
    } else {
        let hint_text = match app.screen {
            Screen::TopicSelect => "Up/Down: select • Enter: open • C: create • Ctrl+Q: quit",
            Screen::CardList => "Up/Down: select • E: edit question • A: edit answer • N: add • D: delete • S: save • B: back",
            Screen::Reveal => "Ctrl+Q: Quit • N: Next • R: Review • Ctrl+E/A: Edit • Ctrl+S: Save",
            _ => "Ctrl+Q: Quit • Ctrl+R: Review • Ctrl+S: Save",
        };
        let hint = Paragraph::new(hint_text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, layout[2]);
    }
}

fn draw_ask(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Question");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some(eng) = &app.eng {
        if let Some((_i, q, _a)) = eng.current_card() {
            let text = Paragraph::new(q)
                .wrap(Wrap { trim: true })
                .alignment(Alignment::Left);
            f.render_widget(text, inner);
        }
    }
}

fn draw_reveal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Answer");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some(eng) = &app.eng {
        if let Some((_i, q, a)) = eng.current_card() {
            let lines = vec![
                Line::from(vec![
                    Span::styled("Question: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(q),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Answer: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(a),
                ]),
                Line::from(""),
                Line::from("Press N: next • E: edit question • A: edit answer"),
            ];
            let para = Paragraph::new(lines).wrap(Wrap { trim: true });
            f.render_widget(para, inner);
        }
    }
}

fn draw_review(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Review");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let text: String = if let Some(eng) = &app.eng {
        eng.responses
            .keys()
            .map(|i| {
                let q = &eng.questions[*i];
                let a = &eng.answers[*i];
                let y = eng.responses.get(i).map(|s| s.as_str()).unwrap_or("(none)");
                format!(
                    "Q#{}\n{}\n\nYou: {}\nCorrect: {}\n{}\n",
                    i + 1,
                    q,
                    y,
                    a,
                    "-".repeat(40)
                )
            })
            .collect()
    } else {
        String::new()
    };
    let para = Paragraph::new(text)
        .scroll((app.review_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(para, inner);
}

fn draw_editor(f: &mut ratatui::Frame, area: Rect, app: &App, editing_question: bool) {
    let title = if editing_question {
        "Edit Question"
    } else {
        "Edit Answer"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let p = Paragraph::new(app.input.as_str())
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);
    f.render_widget(p, inner);
    let hint = Paragraph::new("Enter: Save • Esc: Cancel • Ctrl+S: Save to file")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        hint,
        Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(2),
            width: area.width,
            height: 1,
        },
    );
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
