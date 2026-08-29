use chrono::NaiveDate;
use late_core::models::le_word::{DailyWord, Game, GameParams};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::svc::LeWordService;

/// Mirrors the `le_word_daily_daily_win` reward template. Update both together.
pub const DAILY_WIN_REWARD_CHIPS: i64 = 250;
pub const WORD_LEN: usize = 5;
pub const MAX_GUESSES: usize = 6;
pub const DAILY_DIFFICULTY_KEY: &str = "daily";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LetterScore {
    Correct,
    Present,
    Absent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Daily,
    Replay,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Replay => "replay",
        }
    }
}

#[derive(Clone, Debug)]
struct Snapshot {
    puzzle_date: Option<NaiveDate>,
    answer: String,
    guesses: Vec<String>,
    current_guess: String,
    is_game_over: bool,
    won: bool,
}

pub struct State {
    pub user_id: Uuid,
    pub mode: Mode,
    pub puzzle_date: Option<NaiveDate>,
    pub answer: String,
    pub daily_word_loaded: bool,
    pub guesses: Vec<String>,
    pub current_guess: String,
    pub is_game_over: bool,
    pub won: bool,
    pub show_rules: bool,
    pub reset_pending: bool,
    pub message: String,
    daily_snapshot: Option<Snapshot>,
    replay_snapshot: Option<Snapshot>,
    /// In flight only while a rolled-over day is fetching its word; see
    /// `ensure_current_daily`.
    word_reload_rx: Option<tokio::sync::oneshot::Receiver<Option<DailyWord>>>,
    /// Set when a rollover fetch failed; `ensure_current_daily` holds off
    /// until this passes, then tries again. Without it a transient DB error
    /// at midnight left the board dead for the rest of the session.
    word_reload_backoff_until: Option<std::time::Instant>,
    pub svc: LeWordService,
}

/// How long a failed rollover word fetch waits before the next attempt.
const WORD_RELOAD_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

impl State {
    pub fn new(
        user_id: Uuid,
        svc: LeWordService,
        daily_word: Option<DailyWord>,
        saved_games: Vec<Game>,
    ) -> Self {
        let daily_snapshot = daily_word.map(|word| {
            saved_games
                .iter()
                .find(|game| {
                    game.mode == "daily"
                        && game.puzzle_date == Some(word.puzzle_date)
                        && game.answer_word == word.answer_word
                })
                .map(snapshot_from_game)
                .unwrap_or_else(|| fresh_snapshot(Some(word.puzzle_date), word.answer_word))
        });
        let replay_snapshot = saved_games
            .iter()
            .find(|game| game.mode == "replay" && game.puzzle_date.is_none())
            .map(snapshot_from_game);
        let daily_word_loaded = daily_snapshot.is_some();
        let mut state = Self {
            user_id,
            mode: Mode::Daily,
            puzzle_date: None,
            answer: String::new(),
            daily_word_loaded,
            guesses: Vec::new(),
            current_guess: String::new(),
            is_game_over: false,
            won: false,
            show_rules: false,
            reset_pending: false,
            message: String::new(),
            daily_snapshot,
            replay_snapshot,
            word_reload_rx: None,
            word_reload_backoff_until: None,
            svc,
        };
        state.load_mode_snapshot();
        state
    }

    /// The date of the daily board this session holds, whichever mode is on
    /// screen: the live board in daily mode, the parked snapshot in replay.
    /// `None` means no daily word has landed yet.
    fn daily_puzzle_date(&self) -> Option<NaiveDate> {
        match self.mode {
            Mode::Daily => self.puzzle_date,
            Mode::Replay => self
                .daily_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.puzzle_date),
        }
    }

    /// Roll the daily over when the UTC date changes under a live session.
    /// The word itself lives in the database (the session's first one arrives
    /// with the bootstrap), so this drops the stale daily immediately and
    /// fetches the new word in the background; `poll_word_reload` installs it.
    /// The daily date only advances once the word lands, so a failed fetch is
    /// retried here (after `WORD_RELOAD_RETRY`) instead of leaving the board
    /// dead until reconnect. A replay board on screen is left alone: only the
    /// parked daily snapshot rolls. Returns true when anything changed.
    pub fn ensure_current_daily(&mut self) -> bool {
        let today = self.svc.today();
        if self.daily_puzzle_date() == Some(today) {
            return false;
        }
        if self.word_reload_rx.is_some() {
            return false;
        }
        if let Some(until) = self.word_reload_backoff_until
            && std::time::Instant::now() < until
        {
            return false;
        }
        self.word_reload_backoff_until = None;

        // Yesterday's word must stop scoring guesses right away, whether or
        // not the fetch succeeds.
        self.daily_snapshot = None;
        self.daily_word_loaded = false;
        if self.mode == Mode::Daily {
            self.clear_reset_pending();
            self.load_mode_snapshot();
            self.message = "Loading today's Le Word.".to_string();
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.word_reload_rx = Some(rx);
        let svc = self.svc.clone();
        // Pure state tests drive this without a runtime; the state is already
        // cleared, and the poll below installs nothing when nothing arrives.
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                let word = match svc.ensure_daily_word().await {
                    Ok(word) => Some(word),
                    Err(error) => {
                        tracing::error!(error = ?error, "failed to load the rolled-over Le Word");
                        None
                    }
                };
                let _ = tx.send(word);
            });
        }
        true
    }

    /// Install a word fetched by `ensure_current_daily`. Returns true when
    /// anything changed.
    pub fn poll_word_reload(&mut self) -> bool {
        let Some(rx) = self.word_reload_rx.as_mut() else {
            return false;
        };
        match rx.try_recv() {
            Ok(Some(word)) => {
                self.word_reload_rx = None;
                self.word_reload_backoff_until = None;
                self.daily_snapshot =
                    Some(fresh_snapshot(Some(word.puzzle_date), word.answer_word));
                self.daily_word_loaded = true;
                if self.mode == Mode::Daily {
                    self.load_mode_snapshot();
                }
                true
            }
            Ok(None) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.word_reload_rx = None;
                // The daily date was not advanced, so `ensure_current_daily`
                // tries again once the backoff passes.
                self.word_reload_backoff_until =
                    Some(std::time::Instant::now() + WORD_RELOAD_RETRY);
                if self.mode == Mode::Daily {
                    self.message = "Le Word is unavailable. Retrying soon.".to_string();
                }
                true
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => false,
        }
    }

    /// Today's word has at least one submitted guess and the run is not over.
    pub fn has_unfinished_daily(&self) -> bool {
        if !self.daily_word_loaded {
            return false;
        }
        let (guesses, is_game_over, puzzle_date) = if self.mode == Mode::Daily {
            (&self.guesses, self.is_game_over, self.puzzle_date)
        } else if let Some(snapshot) = &self.daily_snapshot {
            (
                &snapshot.guesses,
                snapshot.is_game_over,
                snapshot.puzzle_date,
            )
        } else {
            return false;
        };
        !guesses.is_empty() && !is_game_over && puzzle_date == Some(self.svc.today())
    }

    pub fn guess_number(&self) -> usize {
        self.guesses
            .len()
            .saturating_add((!self.is_game_over) as usize)
    }

    pub fn is_daily_active(&self) -> bool {
        self.mode == Mode::Daily
    }

    pub fn show_daily(&mut self) {
        self.clear_reset_pending();
        if self.mode == Mode::Daily {
            return;
        }
        self.store_active_snapshot();
        self.save_async();
        self.mode = Mode::Daily;
        self.load_mode_snapshot();
    }

    pub fn show_replay(&mut self) {
        self.clear_reset_pending();
        if self.mode == Mode::Replay {
            return;
        }
        self.store_active_snapshot();
        self.save_async();
        self.mode = Mode::Replay;
        let replay_conflicts_with_daily = self
            .replay_snapshot
            .as_ref()
            .zip(self.daily_snapshot.as_ref())
            .is_some_and(|(replay, daily)| replay.answer == daily.answer);
        if self.replay_snapshot.is_none() || replay_conflicts_with_daily {
            self.replay_snapshot = Some(fresh_snapshot(None, self.next_replay_answer()));
        }
        self.load_mode_snapshot();
        self.save_async();
    }

    pub fn new_replay(&mut self) {
        self.clear_reset_pending();
        if self.mode == Mode::Daily {
            self.store_active_snapshot();
            self.save_async();
        }
        let snapshot = fresh_snapshot(None, self.next_replay_answer());
        self.replay_snapshot = Some(snapshot.clone());
        self.mode = Mode::Replay;
        self.apply_snapshot(snapshot);
        self.save_async();
    }

    pub fn request_replay_reset(&mut self) -> bool {
        if self.reset_pending {
            self.reset_pending = false;
            return true;
        }
        self.reset_pending = true;
        self.message = "Press 0 again for a new random word.".to_string();
        false
    }

    pub fn clear_reset_pending(&mut self) {
        if std::mem::take(&mut self.reset_pending) {
            self.message.clear();
        }
    }

    pub fn submit_guess(&mut self) -> bool {
        self.clear_reset_pending();
        if self.answer.is_empty() {
            self.message = "Le Word is unavailable. Try again soon.".to_string();
            return true;
        }
        if self.is_game_over {
            return false;
        }
        if self.current_guess.len() != WORD_LEN {
            self.message = "Not enough letters.".to_string();
            return true;
        }
        if !self.svc.is_valid_guess(&self.current_guess) {
            self.message = "Not in word list.".to_string();
            return true;
        }

        let guess = std::mem::take(&mut self.current_guess);
        self.guesses.push(guess.clone());
        if guess == self.answer {
            self.won = true;
            self.is_game_over = true;
            self.message = format!("Solved in {}.", self.guesses.len());
            self.store_active_snapshot();
            self.save_async();
            if self.mode == Mode::Daily
                && let Some(puzzle_date) = self.puzzle_date
            {
                self.svc
                    .record_win_task(self.user_id, puzzle_date, self.guesses.len());
            }
            return true;
        }

        if self.guesses.len() >= MAX_GUESSES {
            self.is_game_over = true;
            self.message = format!("The word was {}.", self.answer.to_uppercase());
        } else {
            self.message = "Try again.".to_string();
        }
        self.store_active_snapshot();
        self.save_async();
        true
    }

    pub fn push_letter(&mut self, ch: char) -> bool {
        self.clear_reset_pending();
        if self.answer.is_empty()
            || self.is_game_over
            || self.current_guess.len() >= WORD_LEN
            || !ch.is_ascii_alphabetic()
        {
            return false;
        }
        self.current_guess.push(ch.to_ascii_lowercase());
        self.message.clear();
        true
    }

    pub fn pop_letter(&mut self) -> bool {
        self.clear_reset_pending();
        if self.answer.is_empty() || self.is_game_over {
            return false;
        }
        let changed = self.current_guess.pop().is_some();
        if changed {
            self.message.clear();
        }
        changed
    }

    pub fn scores_for_guess(&self, guess: &str) -> [LetterScore; WORD_LEN] {
        score_guess(guess, &self.answer)
    }

    pub fn score_for_keyboard_letter(&self, letter: char) -> Option<LetterScore> {
        score_letter_from_guesses(&self.guesses, &self.answer, letter)
    }

    pub fn open_rules(&mut self) {
        self.clear_reset_pending();
        self.show_rules = true;
    }

    pub fn close_rules(&mut self) {
        self.show_rules = false;
    }

    fn load_mode_snapshot(&mut self) {
        let snapshot = match self.mode {
            Mode::Daily => self.daily_snapshot.clone(),
            Mode::Replay => self.replay_snapshot.clone(),
        };
        if let Some(snapshot) = snapshot {
            self.apply_snapshot(snapshot);
        } else {
            self.puzzle_date = None;
            self.answer.clear();
            self.guesses.clear();
            self.current_guess.clear();
            self.is_game_over = false;
            self.won = false;
            self.message = "Le Word is unavailable. Try again soon.".to_string();
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.puzzle_date = snapshot.puzzle_date;
        self.answer = snapshot.answer;
        self.guesses = snapshot.guesses;
        self.current_guess = snapshot.current_guess;
        self.is_game_over = snapshot.is_game_over;
        self.won = snapshot.won;
        self.message = snapshot_message(self.mode, self);
    }

    fn store_active_snapshot(&mut self) {
        if self.answer.is_empty() {
            return;
        }
        let snapshot = snapshot_from_state(self);
        match self.mode {
            Mode::Daily => self.daily_snapshot = Some(snapshot),
            Mode::Replay => self.replay_snapshot = Some(snapshot),
        }
    }

    fn next_replay_answer(&self) -> String {
        let daily_answer = self
            .daily_snapshot
            .as_ref()
            .map(|snapshot| snapshot.answer.as_str());
        self.svc
            .replay_answer(&self.answer, daily_answer)
            .to_string()
    }

    fn save_async(&self) {
        if self.answer.is_empty() {
            return;
        }
        self.svc.save_game_task(GameParams {
            user_id: self.user_id,
            mode: self.mode.as_str().to_string(),
            puzzle_date: self.puzzle_date,
            answer_word: self.answer.clone(),
            guesses: serde_json::to_value(&self.guesses).unwrap_or_default(),
            current_guess: self.current_guess.clone(),
            is_game_over: self.is_game_over,
            won: self.won,
        });
    }
}

fn fresh_snapshot(puzzle_date: Option<NaiveDate>, answer: String) -> Snapshot {
    Snapshot {
        puzzle_date,
        answer,
        guesses: Vec::new(),
        current_guess: String::new(),
        is_game_over: false,
        won: false,
    }
}

fn snapshot_from_game(game: &Game) -> Snapshot {
    Snapshot {
        puzzle_date: game.puzzle_date,
        answer: game.answer_word.clone(),
        guesses: serde_json::from_value(game.guesses.clone()).unwrap_or_default(),
        current_guess: game.current_guess.clone(),
        is_game_over: game.is_game_over,
        won: game.won,
    }
}

fn snapshot_from_state(state: &State) -> Snapshot {
    Snapshot {
        puzzle_date: state.puzzle_date,
        answer: state.answer.clone(),
        guesses: state.guesses.clone(),
        current_guess: state.current_guess.clone(),
        is_game_over: state.is_game_over,
        won: state.won,
    }
}

fn snapshot_message(mode: Mode, state: &State) -> String {
    if state.won {
        format!("Solved in {}.", state.guesses.len())
    } else if state.is_game_over {
        format!("The word was {}.", state.answer.to_uppercase())
    } else if state.guesses.is_empty() && state.current_guess.is_empty() {
        match mode {
            Mode::Daily => "Guess today's Le Word.".to_string(),
            Mode::Replay => "Guess a random Le Word.".to_string(),
        }
    } else {
        "Keep going.".to_string()
    }
}

pub fn score_guess(guess: &str, answer: &str) -> [LetterScore; WORD_LEN] {
    let guess = guess.as_bytes();
    let answer = answer.as_bytes();
    let mut scores = [LetterScore::Absent; WORD_LEN];
    let mut remaining = [0u8; 26];

    for (idx, score) in scores.iter_mut().enumerate() {
        if guess.get(idx) == answer.get(idx) {
            *score = LetterScore::Correct;
        } else if let Some(&b) = answer.get(idx)
            && b.is_ascii_lowercase()
        {
            remaining[(b - b'a') as usize] += 1;
        }
    }

    for (idx, score) in scores.iter_mut().enumerate() {
        if *score == LetterScore::Correct {
            continue;
        }
        let Some(&b) = guess.get(idx) else {
            continue;
        };
        if !b.is_ascii_lowercase() {
            continue;
        }
        let count = &mut remaining[(b - b'a') as usize];
        if *count > 0 {
            *score = LetterScore::Present;
            *count -= 1;
        }
    }

    scores
}

pub fn score_letter_from_guesses(
    guesses: &[String],
    answer: &str,
    letter: char,
) -> Option<LetterScore> {
    let letter = letter.to_ascii_lowercase();
    if !letter.is_ascii_lowercase() {
        return None;
    }

    let mut best = None;
    for guess in guesses {
        let scores = score_guess(guess, answer);
        for (idx, ch) in guess.chars().enumerate().take(WORD_LEN) {
            if ch.to_ascii_lowercase() != letter {
                continue;
            }
            if best.is_none_or(|score| score_rank(scores[idx]) > score_rank(score)) {
                best = Some(scores[idx]);
            }
        }
    }
    best
}

fn score_rank(score: LetterScore) -> u8 {
    match score {
        LetterScore::Correct => 3,
        LetterScore::Present => 2,
        LetterScore::Absent => 1,
    }
}

// A child of this module (not a sibling in mod.rs) so the rollover tests can
// drive the private reload channel and backoff directly.
#[cfg(test)]
#[path = "state_test.rs"]
mod state_test;
