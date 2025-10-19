# Flashcards · Rust Edition

A fast, fully terminal-based flashcard application written in Rust using **ratatui** and **crossterm**.
Create topics, edit cards, and study directly from your terminal — with progress tracking, randomization, and persistent storage.

---
[![Crates.io](https://img.shields.io/crates/v/flashcards-rs.svg)](https://crates.io/crates/flashcards-rs)


## Features

- **Topic-based organization**
  - Each topic lives in `/topics/<topic_name>/`
  - Each topic contains `questions.txt` and `answers.txt`
- **Study mode**
  - Sequential or randomized question order
  - Input answers interactively
  - Review your responses at the end
-  **Edit mode**
  - Add, remove, or edit flashcards from inside the TUI
  - Changes persist automatically to disk
- **Progress tracking**
  - Visual progress gauge
  - Saves your session with timestamps (`flashcard_responses_YYYYMMDD-HHMMSS.txt`)
- **Atomic file writes**
  - Safe saving through temporary files to prevent corruption

---

## Folder Structure

```
flashcards-rs/
├── Cargo.toml
├── src/
│   └── main.rs
└── topics/
    ├── AI/
    │   ├── questions.txt
    │   └── answers.txt
    └── Earth/
        ├── questions.txt
        └── answers.txt
```

Each topic folder must contain:
- `questions.txt` — one question per line
- `answers.txt` — one answer per line, in the same order as the questions

---

## Keyboard Shortcuts

### Global
| Key | Action |
|-----|--------|
| **Ctrl+Q** | Quit |
| **Ctrl+R** | Review responses |
| **Ctrl+S** | Save edits to file |

### Topic Select
| Key | Action |
|-----|--------|
| ↑ / ↓ | Move between topics |
| **Enter** | Open selected topic |
| **C** | Create a new topic |

### Main Menu
| Key | Action |
|-----|--------|
| **S** | Start quiz mode |
| **E** | Edit cards |
| **B** | Back to topic select |

### Study Mode
| Key | Action |
|-----|--------|
| **Y / N** | Choose random order or sequential |
| **Enter** | Submit answer or continue |
| **N** | Next card |
| **Ctrl+R** | Review all responses |

### Edit Mode
| Key | Action |
|-----|--------|
| ↑ / ↓ | Move between cards |
| **E** | Edit question |
| **A** | Edit answer |
| **N** | Add new card |
| **D** | Delete selected card |
| **S** | Save |
| **B** | Back to menu |

---

## Running the App

### 1. Clone and build
```bash
git clone https://github.com/fibnas/flashcards-rs.git
cd flashcards-rs
cargo run
```
OR

```bash
gh repo clone fibnas/flashcards-rs
cargo run
```

OR

```bash
cargo install flashcards-rs
```
[![Crates.io](https://img.shields.io/crates/v/flashcards-rs.svg)](https://crates.io/crates/flashcards-rs)


### 2. Add Topics
The app automatically scans `/topics/` for folders.
To create new topics manually:
```bash
mkdir -p topics/Space
echo "What is a black hole?" > topics/Space/questions.txt
echo "A region of spacetime with gravity so strong nothing can escape it." > topics/Space/answers.txt
```

Or create them directly inside the app with `C`.

---

## Saved Sessions

When you finish a quiz, your results are saved automatically:
```
flashcard_responses_20251019-225918.txt
```
Each session log includes:
- Question number and text
- Your answer
- The correct answer

---

## Dependencies

- [`ratatui`](https://crates.io/crates/ratatui) — Terminal UI library
- [`crossterm`](https://crates.io/crates/crossterm) — Terminal event handling
- [`anyhow`](https://crates.io/crates/anyhow) — Simple error management
- [`chrono`](https://crates.io/crates/chrono) — Timestamps
- [`rand`](https://crates.io/crates/rand) — Randomized order support

---

## Example Study Session

```
Topic: AI
Q1: What does AI stand for?
> Artificial Intelligence
Correct!
```

Progress bar updates live as you advance through cards.

---

## Development Notes

### Screen State Machine

The app operates as a **finite state machine** with distinct UI screens managed by the `Screen` enum.
Each state defines a unique interaction context. Transitions are event-driven (keyboard input).

| State | Description | Key Transitions |
|--------|--------------|----------------|
| `TopicSelect` | Browse or create topics. | `Enter` → `MainMenu`, `C` → `TopicCreate` |
| `TopicCreate` | Input new topic name. | `Enter` → `MainMenu`, `Esc` → `TopicSelect` |
| `MainMenu` | Choose between study or edit modes. | `S` → `Mode`, `E` → `CardList` |
| `CardList` | Edit existing cards. | `E` → `EditQuestion`, `A` → `EditAnswer`, `N` → add new |
| `Mode` | Select random or sequential order. | `Y`/`N` → `Ask` |
| `Ask` | Display current question and take input. | `Enter` → `Reveal`, `Ctrl+R` → `Review` |
| `Reveal` | Show correct answer and next-step options. | `N` → next, `Ctrl+E/A` → edit, `Ctrl+R` → review |
| `EditQuestion` / `EditAnswer` | Edit text of a card. | `Enter` → save and return |
| `Review` | Scrollable list of all responses. | `Esc` / `Ctrl+B` → return |
| `Done` | Quiz finished summary screen. | `R` → review |
| `ConfirmQuit` | Exit confirmation modal. | `Y` → exit, `N` → return |

This modular architecture simplifies adding new screens or features (e.g., timed quizzes or import/export support).

---

## License

MIT License.
Built with curiosity, caffeine, and the occasional panic over `unwrap()`.

---

## Author

**Frank Stallion**
Fedora 42
