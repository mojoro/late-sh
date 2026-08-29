use crate::app::activity::event::ActivityEvent;
use crate::app::arcade::le_word::input::{handle_arrow, handle_key};
use crate::app::arcade::le_word::state::*;
use crate::app::arcade::le_word::svc::LeWordService;
use crate::test_helpers::new_test_db;
use late_core::models::le_word::{DailyWin, DailyWord, Game};
use late_core::test_utils::create_test_user;
use tokio::sync::broadcast;

#[test]
fn score_guess_handles_duplicate_letters() {
    assert_eq!(
        score_guess("allee", "apple"),
        [
            LetterScore::Correct,
            LetterScore::Present,
            LetterScore::Absent,
            LetterScore::Absent,
            LetterScore::Correct,
        ]
    );
    assert_eq!(
        score_guess("sassy", "abyss"),
        [
            LetterScore::Present,
            LetterScore::Present,
            LetterScore::Absent,
            LetterScore::Correct,
            LetterScore::Present,
        ]
    );
}

#[test]
fn score_guess_matches_shade_screenshot_case() {
    assert_eq!(
        score_guess("wormy", "shade"),
        [
            LetterScore::Absent,
            LetterScore::Absent,
            LetterScore::Absent,
            LetterScore::Absent,
            LetterScore::Absent,
        ]
    );
    assert_eq!(
        score_guess("adieu", "shade"),
        [
            LetterScore::Present,
            LetterScore::Present,
            LetterScore::Absent,
            LetterScore::Present,
            LetterScore::Absent,
        ]
    );
    assert_eq!(
        score_guess("adeem", "shade"),
        [
            LetterScore::Present,
            LetterScore::Present,
            LetterScore::Present,
            LetterScore::Absent,
            LetterScore::Absent,
        ]
    );
    assert_eq!(
        score_guess("house", "shade"),
        [
            LetterScore::Present,
            LetterScore::Absent,
            LetterScore::Absent,
            LetterScore::Present,
            LetterScore::Correct,
        ]
    );
}

#[test]
fn score_letter_from_guesses_keeps_best_keyboard_hint() {
    let guesses = vec!["allee".to_string(), "sassy".to_string()];

    assert_eq!(
        score_letter_from_guesses(&guesses, "apple", 'a'),
        Some(LetterScore::Correct)
    );
    assert_eq!(
        score_letter_from_guesses(&guesses, "apple", 'l'),
        Some(LetterScore::Present)
    );
    assert_eq!(
        score_letter_from_guesses(&guesses, "apple", 's'),
        Some(LetterScore::Absent)
    );
    assert_eq!(score_letter_from_guesses(&guesses, "apple", 'z'), None);
}

#[tokio::test]
async fn replay_restores_the_in_progress_daily_board() {
    let test_db = new_test_db().await;
    let user = create_test_user(&test_db.db, "le-word-state-modes").await;
    let (activity_tx, _) = broadcast::channel::<ActivityEvent>(8);
    let svc = LeWordService::new(test_db.db.clone(), activity_tx);
    let today = svc.today();
    let daily_word = DailyWord {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        puzzle_date: today,
        answer_word: "hunch".to_string(),
    };
    let mut state = State::new(user.id, svc, Some(daily_word), Vec::new());
    state.current_guess = "glass".to_string();

    state.show_replay();

    assert_eq!(state.mode, Mode::Replay);
    assert!(!state.is_daily_active());
    assert_ne!(state.answer, "hunch");
    assert!(state.current_guess.is_empty());
    let first_replay_answer = state.answer.clone();

    state.new_replay();

    assert_ne!(state.answer, first_replay_answer);
    state.show_daily();
    assert_eq!(state.mode, Mode::Daily);
    assert!(state.is_daily_active());
    assert_eq!(state.answer, "hunch");
    assert_eq!(state.current_guess, "glass");
}

#[tokio::test]
async fn saved_replay_board_is_restored_after_reconnect() {
    let test_db = new_test_db().await;
    let user = create_test_user(&test_db.db, "le-word-replay-restore").await;
    let (activity_tx, _) = broadcast::channel::<ActivityEvent>(8);
    let svc = LeWordService::new(test_db.db.clone(), activity_tx);
    let today = svc.today();
    let daily_word = DailyWord {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        puzzle_date: today,
        answer_word: "hunch".to_string(),
    };
    let replay_game = Game {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        user_id: user.id,
        mode: "replay".to_string(),
        puzzle_date: None,
        answer_word: "apple".to_string(),
        guesses: serde_json::json!(["shade"]),
        current_guess: "cl".to_string(),
        is_game_over: false,
        won: false,
    };
    let mut state = State::new(user.id, svc, Some(daily_word), vec![replay_game]);

    state.show_replay();

    assert_eq!(state.mode, Mode::Replay);
    assert_eq!(state.answer, "apple");
    assert_eq!(state.guesses, vec!["shade"]);
    assert_eq!(state.current_guess, "cl");
}

#[tokio::test]
async fn saved_replay_rotates_if_its_answer_matches_todays_daily() {
    let test_db = new_test_db().await;
    let user = create_test_user(&test_db.db, "le-word-replay-daily-collision").await;
    let (activity_tx, _) = broadcast::channel::<ActivityEvent>(8);
    let svc = LeWordService::new(test_db.db.clone(), activity_tx);
    let today = svc.today();
    let daily_word = DailyWord {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        puzzle_date: today,
        answer_word: "hunch".to_string(),
    };
    let replay_game = Game {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        user_id: user.id,
        mode: "replay".to_string(),
        puzzle_date: None,
        answer_word: "hunch".to_string(),
        guesses: serde_json::json!(["hunch"]),
        current_guess: String::new(),
        is_game_over: true,
        won: true,
    };
    let mut state = State::new(user.id, svc, Some(daily_word), vec![replay_game]);

    state.show_replay();

    assert_eq!(state.mode, Mode::Replay);
    assert_ne!(state.answer, "hunch");
    assert!(state.guesses.is_empty());
    assert!(!state.is_game_over);
}

#[tokio::test]
async fn random_replay_requires_a_double_press() {
    let test_db = new_test_db().await;
    let user = create_test_user(&test_db.db, "le-word-replay-confirm").await;
    let (activity_tx, _) = broadcast::channel::<ActivityEvent>(8);
    let svc = LeWordService::new(test_db.db.clone(), activity_tx);
    let today = svc.today();
    let daily_word = DailyWord {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        puzzle_date: today,
        answer_word: "hunch".to_string(),
    };
    let mut state = State::new(user.id, svc, Some(daily_word), Vec::new());

    assert!(handle_key(&mut state, b'0'));
    assert_eq!(state.mode, Mode::Daily);
    assert!(state.reset_pending);

    assert!(handle_arrow(&mut state, b'A'));
    assert!(!state.reset_pending);
    assert!(state.message.is_empty());

    assert!(handle_key(&mut state, b'0'));
    assert!(state.reset_pending);
    assert!(handle_key(&mut state, b'0'));
    assert_eq!(state.mode, Mode::Replay);
    assert!(!state.reset_pending);
    assert_ne!(state.answer, "hunch");
}

#[tokio::test]
async fn replay_win_does_not_record_a_daily_win() {
    let test_db = new_test_db().await;
    let user = create_test_user(&test_db.db, "le-word-replay-no-reward").await;
    let (activity_tx, mut activity_rx) = broadcast::channel::<ActivityEvent>(8);
    let svc = LeWordService::new(test_db.db.clone(), activity_tx);
    let today = svc.today();
    let daily_word = DailyWord {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        puzzle_date: today,
        answer_word: "hunch".to_string(),
    };
    let mut state = State::new(user.id, svc, Some(daily_word), Vec::new());
    state.show_replay();
    state.current_guess = state.answer.clone();

    assert!(state.submit_guess());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = test_db.db.get().await.expect("client");
    assert!(
        !DailyWin::has_won_today(&client, user.id, today)
            .await
            .expect("daily win lookup")
    );
    assert!(matches!(
        activity_rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

fn yesterdays_state() -> (State, chrono::NaiveDate, chrono::NaiveDate) {
    use crate::app::arcade::le_word::svc::LeWordService;
    use late_core::db::{Db, DbConfig};
    use late_core::models::le_word::DailyWord;
    use uuid::Uuid;

    let (activity_feed, _) = tokio::sync::broadcast::channel(1);
    let svc = LeWordService::new(
        Db::new(&DbConfig::default()).expect("test db pool"),
        activity_feed,
    );
    let today = svc.today();
    let yesterday = today.pred_opt().expect("yesterday");
    let now = chrono::Utc::now();
    let state = State::new(
        Uuid::now_v7(),
        svc,
        Some(DailyWord {
            id: Uuid::now_v7(),
            created: now,
            updated: now,
            puzzle_date: yesterday,
            answer_word: "crane".to_string(),
        }),
        Vec::new(),
    );
    (state, today, yesterday)
}

fn todays_word(today: chrono::NaiveDate) -> late_core::models::le_word::DailyWord {
    let now = chrono::Utc::now();
    late_core::models::le_word::DailyWord {
        id: uuid::Uuid::now_v7(),
        created: now,
        updated: now,
        puzzle_date: today,
        answer_word: "slate".to_string(),
    }
}

/// Yesterday's word stayed on the board of a session that never reconnected,
/// so guesses went on being scored against it after the day rolled over.
#[test]
fn rolling_over_the_day_clears_yesterdays_word() {
    let (mut state, today, yesterday) = yesterdays_state();
    state.guesses.push("slate".to_string());
    assert_eq!(state.puzzle_date, Some(yesterday));

    assert!(state.ensure_current_daily(), "the day should roll over");
    assert!(state.guesses.is_empty(), "yesterday's guesses are still up");
    assert!(
        !state.daily_word_loaded,
        "the board must not accept guesses until today's word lands"
    );

    // A fetch is in flight: no duplicate spawn on the next tick.
    assert!(!state.ensure_current_daily());

    // The word lands; only now does the round own today's date.
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.word_reload_rx = Some(rx);
    tx.send(Some(todays_word(today))).expect("deliver word");
    assert!(state.poll_word_reload());
    assert_eq!(state.puzzle_date, Some(today));
    assert!(state.daily_word_loaded);

    // Same day again: nothing to do.
    assert!(!state.ensure_current_daily());
}

/// A failed rollover fetch used to disable Le Word for the rest of the
/// session: the date had already advanced, so the rollover never re-ran and
/// input stayed gated on `daily_word_loaded`. The date now only advances when
/// the word lands, and a failure retries after a backoff.
#[test]
fn failed_word_fetch_retries_after_backoff() {
    let (mut state, today, _yesterday) = yesterdays_state();

    // Without a runtime the fetch sender drops, standing in for a DB error.
    assert!(state.ensure_current_daily());
    assert!(state.poll_word_reload(), "the dead fetch should be noticed");
    // The stale board is gone and today is not banked, so the rollover
    // still has a reason to run again.
    assert_eq!(state.puzzle_date, None, "a failure must not bank today");
    assert!(!state.daily_word_loaded);

    // Inside the backoff window: no hammering.
    assert!(!state.ensure_current_daily());

    // Once the backoff passes, the rollover tries again...
    state.word_reload_backoff_until = Some(std::time::Instant::now());
    assert!(state.ensure_current_daily(), "the fetch should retry");

    // ...and a successful retry brings the board back for good.
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.word_reload_rx = Some(rx);
    tx.send(Some(todays_word(today))).expect("deliver word");
    assert!(state.poll_word_reload());
    assert_eq!(state.puzzle_date, Some(today));
    assert!(state.daily_word_loaded, "the retried word should install");
    assert!(!state.ensure_current_daily());
}

/// The rollover must not touch a replay board on screen: only the parked
/// daily rolls, and the daily board is fresh when the player switches back.
#[tokio::test]
async fn rolling_over_the_day_leaves_the_replay_board_alone() {
    let test_db = new_test_db().await;
    let user = create_test_user(&test_db.db, "le-word-replay-rollover").await;
    let (activity_tx, _) = broadcast::channel::<ActivityEvent>(8);
    let svc = LeWordService::new(test_db.db.clone(), activity_tx);
    let today = svc.today();
    let yesterday = today.pred_opt().expect("yesterday");
    let stale_word = DailyWord {
        id: uuid::Uuid::now_v7(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        puzzle_date: yesterday,
        answer_word: "crane".to_string(),
    };
    let mut state = State::new(user.id, svc, Some(stale_word), Vec::new());
    state.guesses.push("slate".to_string());
    state.show_replay();
    let replay_answer = state.answer.clone();
    state.guesses.push("crate".to_string());
    state.current_guess = "sl".to_string();

    assert!(state.ensure_current_daily(), "the parked daily should roll");
    assert_eq!(state.mode, Mode::Replay);
    assert_eq!(state.answer, replay_answer);
    assert_eq!(state.guesses, vec!["crate"]);
    assert_eq!(state.current_guess, "sl");
    assert!(!state.has_unfinished_daily());
    assert!(!state.ensure_current_daily(), "a fetch is already in flight");

    // Stand in for the fetch landing today's word.
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.word_reload_rx = Some(rx);
    tx.send(Some(todays_word(today))).expect("deliver word");
    assert!(state.poll_word_reload());
    assert_eq!(state.mode, Mode::Replay);
    assert_eq!(state.answer, replay_answer);
    assert_eq!(state.guesses, vec!["crate"]);

    state.show_daily();
    assert_eq!(state.mode, Mode::Daily);
    assert_eq!(state.puzzle_date, Some(today));
    assert_eq!(state.answer, "slate");
    assert!(
        state.guesses.is_empty(),
        "yesterday's guesses must not carry over"
    );
    assert!(state.daily_word_loaded);
    assert!(!state.ensure_current_daily());
}
