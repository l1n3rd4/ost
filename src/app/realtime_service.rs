//! Realtime use-case service.
//!
//! `RealtimeService` orchestrates the [`RealtimePort`] and [`PresenterPort`]:
//! it subscribes to the realtime event stream, forwards each
//! [`RealtimeEvent`](crate::domain::RealtimeEvent) to the presenter, and — when
//! the connection is lost — reconnects with exponential backoff. This is the
//! hexagonal home of the old `trouter::connect_and_run` reconnect loop, now
//! expressed over ports so it runs with zero infra dependencies in tests. Ports
//! are injected as `Arc<dyn Port>` (Requirement 6.7).
//!
//! ## Reconnect policy (Requirements 6.5, 6.6)
//!
//! WHILE a subscription is active AND the connection is lost, the service
//! retries reconnect with exponential backoff **1s → 60s**, capped at **≤10
//! attempts**. The delay schedule is 1, 2, 4, 8, 16, 32, 60, 60, 60, 60
//! seconds (doubling, clamped to 60s). IF the reconnect attempts reach the
//! 10-attempt maximum, THEN the service ends the subscription returning a
//! [`DomainError`]. The service never calls `println!`/`eprintln!`
//! (Requirement 5.2); output is routed through the [`PresenterPort`] and
//! failures propagate as [`DomainError`].
//!
//! ## Determinism / testability
//!
//! The backoff schedule is a pure function ([`backoff_delay`]) and the delay is
//! injected through the [`Sleeper`] trait. The default [`TokioSleeper`] uses
//! `tokio::time::sleep`; tests inject a no-op/recording sleeper so the
//! backoff-behaviour tests (task 10.4) run instantly and deterministically with
//! 0 network calls and 0 real waits.
//!
//! [`RealtimePort`]: crate::ports::RealtimePort
//! [`PresenterPort`]: crate::ports::PresenterPort
//! [`DomainError`]: crate::domain::DomainError

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use crate::domain::{DomainError, DomainResult};
use crate::ports::{PresenterPort, RealtimePort};

/// Maximum number of reconnect attempts before the subscription is ended
/// (Requirement 6.6).
pub const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Lower bound of the exponential backoff, in seconds (Requirement 6.5).
pub const MIN_BACKOFF_SECS: u64 = 1;

/// Upper bound (cap) of the exponential backoff, in seconds (Requirement 6.5).
pub const MAX_BACKOFF_SECS: u64 = 60;

/// Pure backoff schedule: the delay before reconnect attempt number `attempt`.
///
/// `attempt` is 1-based (the delay taken *before* the Nth reconnect). The
/// schedule doubles from [`MIN_BACKOFF_SECS`] and is clamped to
/// [`MAX_BACKOFF_SECS`], giving 1, 2, 4, 8, 16, 32, 60, 60, ... seconds
/// (Requirement 6.5). `attempt = 0` is treated as the first delay.
///
/// This function is deterministic and does not sleep, so tests can assert the
/// schedule directly without waiting.
pub fn backoff_delay(attempt: u32) -> Duration {
    // 1s doubled `attempt - 1` times, saturating, then clamped to the 60s cap.
    let shift = attempt.saturating_sub(1);
    let secs = MIN_BACKOFF_SECS
        .checked_shl(shift)
        .unwrap_or(MAX_BACKOFF_SECS)
        .min(MAX_BACKOFF_SECS);
    Duration::from_secs(secs)
}

/// Injectable delay seam so backoff behaviour is testable without real waits.
///
/// The production implementation ([`TokioSleeper`]) delegates to
/// `tokio::time::sleep`; tests provide a no-op/recording implementation to keep
/// the reconnect tests fast and deterministic.
#[async_trait]
pub trait Sleeper: Send + Sync {
    /// Sleep for `duration`.
    async fn sleep(&self, duration: Duration);
}

/// Default [`Sleeper`] backed by `tokio::time::sleep`.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Use-case service for the realtime event subscription and reconnect loop.
///
/// Holds a [`RealtimePort`], a [`PresenterPort`] and a [`Sleeper`], all injected
/// as `Arc<dyn _>` (Requirement 6.7).
pub struct RealtimeService {
    realtime: Arc<dyn RealtimePort>,
    presenter: Arc<dyn PresenterPort>,
    sleeper: Arc<dyn Sleeper>,
}

impl RealtimeService {
    /// Construct a `RealtimeService` with the default [`TokioSleeper`].
    pub fn new(realtime: Arc<dyn RealtimePort>, presenter: Arc<dyn PresenterPort>) -> Self {
        Self::with_sleeper(realtime, presenter, Arc::new(TokioSleeper))
    }

    /// Construct a `RealtimeService` with a custom [`Sleeper`].
    ///
    /// This is the seam the backoff tests (task 10.4) use to inject a no-op or
    /// recording sleeper so the reconnect schedule is exercised instantly.
    pub fn with_sleeper(
        realtime: Arc<dyn RealtimePort>,
        presenter: Arc<dyn PresenterPort>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Self {
        Self {
            realtime,
            presenter,
            sleeper,
        }
    }

    /// Subscribe and run the realtime loop until it ends.
    ///
    /// Subscribes via [`RealtimePort::subscribe`] and forwards every
    /// [`RealtimeEvent`](crate::domain::RealtimeEvent) to the presenter via
    /// [`PresenterPort::notify_event`]. When the stream ends (connection lost)
    /// the service reconnects with exponential backoff (Requirement 6.5); if the
    /// reconnect attempts reach [`MAX_RECONNECT_ATTEMPTS`] the subscription ends
    /// and a [`DomainError`] is returned (Requirement 6.6).
    ///
    /// A successful (re)subscription that delivers at least one event resets the
    /// attempt counter, matching the legacy "stable session resets backoff"
    /// behaviour of `connect_and_run`.
    pub async fn run(&self) -> DomainResult<()> {
        let mut attempts: u32 = 0;

        loop {
            match self.realtime.subscribe().await {
                Ok(mut stream) => {
                    let mut delivered = 0u64;
                    while let Some(event) = stream.next().await {
                        self.presenter.notify_event(&event);
                        delivered += 1;
                    }
                    // Stream ended: the connection was lost. A session that
                    // delivered events was "stable", so reset the backoff.
                    if delivered > 0 {
                        attempts = 0;
                    }
                }
                Err(err) => {
                    // A subscribe failure is a lost connection too; forward the
                    // error to the presenter (no stdout/stderr) and retry.
                    self.presenter.show_error(&err);
                }
            }

            attempts += 1;
            if attempts >= MAX_RECONNECT_ATTEMPTS {
                let err = DomainError::Transport(format!(
                    "realtime subscription ended after {MAX_RECONNECT_ATTEMPTS} reconnect attempts"
                ));
                self.presenter.show_error(&err);
                return Err(err);
            }

            self.sleeper.sleep(backoff_delay(attempts)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for `RealtimeService`.
    //!
    //! Property 6: Testability — these run with 0 network calls, 0 filesystem
    //! operations and 0 real waits (the sleeper is a no-op recorder). Fakes
    //! return in-memory streams and record presenter calls so we can assert
    //! event forwarding (Requirement 6.3 / notify_event) and the backoff/
    //! attempt-cap policy (Requirements 6.5, 6.6). The dedicated backoff tests
    //! live in task 10.4; these are the basic coverage for this task.

    use super::*;
    use crate::domain::{ChatId, Message, Presence, RealtimeEvent};
    use futures::stream;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A `RealtimePort` fake that yields a scripted sequence of streams, one per
    /// `subscribe` call. Each inner `Vec<RealtimeEvent>` becomes a finite stream
    /// (an ended stream models a lost connection). When the script is exhausted
    /// it yields empty streams, so the service keeps "reconnecting" until the
    /// attempt cap is hit. Records how many times `subscribe` was called.
    struct ScriptedRealtime {
        scripts: Mutex<std::collections::VecDeque<Vec<RealtimeEvent>>>,
        subscribe_calls: AtomicUsize,
    }

    impl ScriptedRealtime {
        fn new(scripts: Vec<Vec<RealtimeEvent>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into_iter().collect()),
                subscribe_calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.subscribe_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RealtimePort for ScriptedRealtime {
        async fn subscribe(
            &self,
        ) -> DomainResult<futures_core::stream::BoxStream<'static, RealtimeEvent>> {
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            let events = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
            Ok(stream::iter(events).boxed())
        }
    }

    /// Presenter fake that records forwarded events and errors instead of
    /// printing.
    #[derive(Default)]
    struct RecordingPresenter {
        events: Mutex<Vec<RealtimeEvent>>,
        errors: Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn event_count(&self) -> usize {
            self.events.lock().unwrap().len()
        }
        fn error_count(&self) -> usize {
            self.errors.lock().unwrap().len()
        }
    }

    impl PresenterPort for RecordingPresenter {
        fn show_chats(&self, _chats: &[crate::domain::Chat]) {
            unreachable!("RealtimeService must not call show_chats");
        }
        fn show_messages(&self, _messages: &[Message]) {
            unreachable!("RealtimeService must not call show_messages");
        }
        fn message_sent(&self, _sent: &crate::domain::SentMessage) {
            unreachable!("RealtimeService must not call message_sent");
        }
        fn show_presence(&self, _presence: &Presence) {
            unreachable!("RealtimeService must not call show_presence");
        }
        fn show_user(&self, _user: &crate::domain::User) {
            unreachable!("RealtimeService must not call show_user");
        }
        fn show_teams(&self, _teams: &[crate::domain::Team]) {
            unreachable!("RealtimeService must not call show_teams");
        }
        fn notify_event(&self, event: &RealtimeEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        fn show_error(&self, error: &DomainError) {
            self.errors.lock().unwrap().push(error.to_string());
        }
    }

    /// No-op sleeper recording every requested delay, so tests exercise the
    /// backoff schedule with no real waiting.
    #[derive(Default)]
    struct RecordingSleeper {
        delays: Mutex<Vec<Duration>>,
    }

    impl RecordingSleeper {
        fn delays(&self) -> Vec<Duration> {
            self.delays.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Sleeper for RecordingSleeper {
        async fn sleep(&self, duration: Duration) {
            self.delays.lock().unwrap().push(duration);
        }
    }

    fn sample_event() -> RealtimeEvent {
        RealtimeEvent::PresenceChanged {
            user_id: "u1".to_string(),
            presence: Presence::Available,
        }
    }

    fn message_event() -> RealtimeEvent {
        RealtimeEvent::MessageReceived {
            chat_id: ChatId::new("19:a@thread.v2").unwrap(),
            message: Message {
                sender: "Alice".to_string(),
                timestamp: None,
                content: "hi".to_string(),
            },
        }
    }

    #[test]
    fn backoff_schedule_is_1s_to_60s_capped() {
        // 1, 2, 4, 8, 16, 32, then clamped at 60 (Requirement 6.5).
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(8));
        assert_eq!(backoff_delay(5), Duration::from_secs(16));
        assert_eq!(backoff_delay(6), Duration::from_secs(32));
        assert_eq!(backoff_delay(7), Duration::from_secs(60));
        assert_eq!(backoff_delay(8), Duration::from_secs(60));
        assert_eq!(backoff_delay(100), Duration::from_secs(60));
        // Never below the 1s floor and never above the 60s cap.
        for a in 0..64 {
            let d = backoff_delay(a).as_secs();
            assert!((MIN_BACKOFF_SECS..=MAX_BACKOFF_SECS).contains(&d));
        }
    }

    #[tokio::test]
    async fn forwards_each_event_to_presenter() {
        // One subscription delivering two events, then it ends and the service
        // reconnects until the cap. Both events must reach notify_event.
        let realtime = Arc::new(ScriptedRealtime::new(vec![vec![
            sample_event(),
            message_event(),
        ]]));
        let presenter = Arc::new(RecordingPresenter::default());
        let sleeper = Arc::new(RecordingSleeper::default());
        let service =
            RealtimeService::with_sleeper(realtime, presenter.clone(), sleeper);

        let result = service.run().await;

        assert!(matches!(result, Err(DomainError::Transport(_))));
        assert_eq!(presenter.event_count(), 2);
    }

    #[tokio::test]
    async fn ends_subscription_with_domain_error_after_max_attempts() {
        // Every subscription yields an empty (immediately-lost) stream, so the
        // service must reconnect up to the cap and then end with a DomainError.
        let realtime = Arc::new(ScriptedRealtime::new(vec![]));
        let presenter = Arc::new(RecordingPresenter::default());
        let sleeper = Arc::new(RecordingSleeper::default());
        let service =
            RealtimeService::with_sleeper(realtime.clone(), presenter.clone(), sleeper.clone());

        let result = service.run().await;

        assert!(
            matches!(result, Err(DomainError::Transport(_))),
            "expected Transport error on exhaustion, got {result:?}"
        );
        // Exactly MAX_RECONNECT_ATTEMPTS subscribe attempts were made.
        assert_eq!(realtime.calls(), MAX_RECONNECT_ATTEMPTS as usize);
        // One sleep between each attempt: MAX - 1 delays, following the schedule.
        let delays = sleeper.delays();
        assert_eq!(delays.len(), (MAX_RECONNECT_ATTEMPTS - 1) as usize);
        assert_eq!(delays[0], Duration::from_secs(1));
        assert_eq!(delays[1], Duration::from_secs(2));
        assert_eq!(*delays.last().unwrap(), Duration::from_secs(MAX_BACKOFF_SECS));
        // The terminal error was also forwarded to the presenter.
        assert!(presenter.error_count() >= 1);
    }

    // ---------------------------------------------------------------------
    // Task 10.4: dedicated backoff tests (Requirements 6.5, 6.6).
    //
    // These strengthen coverage beyond the basic tests above:
    //  - the FULL recorded delay schedule matches the exact expected
    //    sequence and stays within [1s, 60s] (Req 6.5),
    //  - subscribe is called exactly MAX_RECONNECT_ATTEMPTS times on
    //    continuous drops (never an 11th attempt) and the terminal result is
    //    the expected `DomainError::Transport` variant (Req 6.6),
    //  - a subscribe-error drop is treated like a lost connection and still
    //    counts toward the attempt cap,
    //  - a stable session that delivers events resets the attempt counter, so
    //    the backoff schedule restarts at 1s (backoff recovery).
    // ---------------------------------------------------------------------

    /// A `RealtimePort` fake that forces `drops` lost connections (each
    /// modelled as a `subscribe` returning an immediately-ended empty stream)
    /// before yielding a single "stable" session that delivers `stable_events`.
    /// After that stable session it keeps dropping (empty streams) forever, so
    /// the service reconnects until the attempt cap. Records every `subscribe`.
    struct DroppingRealtime {
        remaining_initial_drops: AtomicUsize,
        stable_delivered: std::sync::atomic::AtomicBool,
        stable_events: Vec<RealtimeEvent>,
        subscribe_calls: AtomicUsize,
    }

    impl DroppingRealtime {
        fn new(drops: usize, stable_events: Vec<RealtimeEvent>) -> Self {
            Self {
                remaining_initial_drops: AtomicUsize::new(drops),
                stable_delivered: std::sync::atomic::AtomicBool::new(false),
                stable_events,
                subscribe_calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.subscribe_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RealtimePort for DroppingRealtime {
        async fn subscribe(
            &self,
        ) -> DomainResult<futures_core::stream::BoxStream<'static, RealtimeEvent>> {
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            // Still owe some initial drops -> immediately-lost (empty) stream.
            let prev = self.remaining_initial_drops.load(Ordering::SeqCst);
            if prev > 0 {
                self.remaining_initial_drops.store(prev - 1, Ordering::SeqCst);
                return Ok(stream::iter(Vec::<RealtimeEvent>::new()).boxed());
            }
            // Initial drops exhausted: deliver the one stable session once, then
            // drop forever after.
            if !self.stable_delivered.swap(true, Ordering::SeqCst) {
                return Ok(stream::iter(self.stable_events.clone()).boxed());
            }
            Ok(stream::iter(Vec::<RealtimeEvent>::new()).boxed())
        }
    }

    /// A `RealtimePort` fake whose `subscribe` always fails with a
    /// `DomainError`, modelling a connection that can never be established.
    struct AlwaysFailingRealtime {
        subscribe_calls: AtomicUsize,
    }

    impl AlwaysFailingRealtime {
        fn new() -> Self {
            Self {
                subscribe_calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.subscribe_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RealtimePort for AlwaysFailingRealtime {
        async fn subscribe(
            &self,
        ) -> DomainResult<futures_core::stream::BoxStream<'static, RealtimeEvent>> {
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            Err(DomainError::Transport("connection refused".to_string()))
        }
    }

    /// The exact backoff schedule the service takes between the
    /// `MAX_RECONNECT_ATTEMPTS` continuous-drop attempts: one delay before each
    /// reconnect after the first, i.e. `backoff_delay(1..=MAX-1)`.
    fn expected_continuous_drop_schedule() -> Vec<Duration> {
        (1..MAX_RECONNECT_ATTEMPTS).map(backoff_delay).collect()
    }

    #[tokio::test]
    async fn continuous_drops_take_exact_backoff_schedule_within_bounds() {
        // Every subscription drops immediately, so the service reconnects up to
        // the cap. Assert the FULL recorded delay schedule equals the exact
        // expected 1,2,4,8,16,32,60,60,60 sequence and every delay is within
        // [1s, 60s] (Requirement 6.5).
        let realtime = Arc::new(ScriptedRealtime::new(vec![]));
        let presenter = Arc::new(RecordingPresenter::default());
        let sleeper = Arc::new(RecordingSleeper::default());
        let service = RealtimeService::with_sleeper(
            realtime.clone(),
            presenter.clone(),
            sleeper.clone(),
        );

        let result = service.run().await;

        assert!(matches!(result, Err(DomainError::Transport(_))));

        let delays = sleeper.delays();
        // Full schedule matches exactly: 1, 2, 4, 8, 16, 32, 60, 60, 60.
        let expected = expected_continuous_drop_schedule();
        assert_eq!(
            delays, expected,
            "recorded backoff schedule must match the exact 1s->60s doubling-capped sequence"
        );
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(32),
                Duration::from_secs(60),
                Duration::from_secs(60),
                Duration::from_secs(60),
            ]
        );
        // Every delay is within the [1s, 60s] bounds (Requirement 6.5).
        for d in &delays {
            let secs = d.as_secs();
            assert!(
                (MIN_BACKOFF_SECS..=MAX_BACKOFF_SECS).contains(&secs),
                "delay {secs}s out of [{MIN_BACKOFF_SECS}s, {MAX_BACKOFF_SECS}s] bounds"
            );
        }
    }

    #[tokio::test]
    async fn subscribe_called_exactly_max_attempts_never_an_eleventh() {
        // On continuous drops, subscribe must be called exactly
        // MAX_RECONNECT_ATTEMPTS times — never an 11th attempt — and terminate
        // with `DomainError::Transport` (Requirement 6.6).
        let realtime = Arc::new(ScriptedRealtime::new(vec![]));
        let presenter = Arc::new(RecordingPresenter::default());
        let sleeper = Arc::new(RecordingSleeper::default());
        let service = RealtimeService::with_sleeper(
            realtime.clone(),
            presenter.clone(),
            sleeper.clone(),
        );

        let result = service.run().await;

        match result {
            Err(DomainError::Transport(_)) => {}
            other => panic!("expected DomainError::Transport on exhaustion, got {other:?}"),
        }
        assert_eq!(
            realtime.calls(),
            MAX_RECONNECT_ATTEMPTS as usize,
            "subscribe must be called exactly the cap, never an 11th time"
        );
        // Exactly one fewer sleep than attempts (a sleep sits between attempts,
        // and none after the terminal attempt).
        assert_eq!(
            sleeper.delays().len(),
            (MAX_RECONNECT_ATTEMPTS - 1) as usize
        );
    }

    #[tokio::test]
    async fn subscribe_errors_count_toward_the_attempt_cap() {
        // A `subscribe` that fails is a lost connection too: it is forwarded to
        // the presenter and still counts toward the cap. The service must stop
        // at exactly MAX_RECONNECT_ATTEMPTS and return `DomainError::Transport`.
        let realtime = Arc::new(AlwaysFailingRealtime::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let sleeper = Arc::new(RecordingSleeper::default());
        let service = RealtimeService::with_sleeper(
            realtime.clone(),
            presenter.clone(),
            sleeper.clone(),
        );

        let result = service.run().await;

        assert!(matches!(result, Err(DomainError::Transport(_))));
        assert_eq!(realtime.calls(), MAX_RECONNECT_ATTEMPTS as usize);
        // Each failed subscribe forwarded an error, plus the terminal error:
        // MAX failures + 1 terminal.
        assert_eq!(
            presenter.error_count(),
            MAX_RECONNECT_ATTEMPTS as usize + 1
        );
        // No event was ever delivered.
        assert_eq!(presenter.event_count(), 0);
    }

    #[tokio::test]
    async fn stable_session_resets_backoff_schedule() {
        // Two drops, then a stable session that delivers an event, then drops
        // forever. The delivering session resets the attempt counter, so the
        // backoff schedule restarts at 1s afterwards (backoff recovery).
        //
        // Timeline of `run()`:
        //   sub1 empty  -> attempts=1 -> sleep(1s)
        //   sub2 empty  -> attempts=2 -> sleep(2s)
        //   sub3 event  -> delivered>0 -> attempts reset to 0 -> attempts=1 -> sleep(1s)
        //   sub4..sub10 empty -> attempts 2..10 -> sleep(2s,4s,...) then terminate.
        let realtime = Arc::new(DroppingRealtime::new(2, vec![sample_event()]));
        let presenter = Arc::new(RecordingPresenter::default());
        let sleeper = Arc::new(RecordingSleeper::default());
        let service = RealtimeService::with_sleeper(
            realtime.clone(),
            presenter.clone(),
            sleeper.clone(),
        );

        let result = service.run().await;

        assert!(matches!(result, Err(DomainError::Transport(_))));
        // The stable session delivered its one event.
        assert_eq!(presenter.event_count(), 1);

        let delays = sleeper.delays();
        // The delivering session (sub 3) resets the attempt counter, so the
        // backoff schedule restarts at 1s afterwards. Expected timeline:
        //   drop 1 -> 1s, drop 2 -> 2s, [stable session: reset], then the
        //   post-reset drops climb 1s,2s,4s,...,32s until attempts hits the cap.
        // The first delay after the reset is 1s (the recovery signal); assert
        // the concrete prefix explicitly.
        assert_eq!(delays[0], Duration::from_secs(1), "drop 1");
        assert_eq!(delays[1], Duration::from_secs(2), "drop 2");
        assert_eq!(
            delays[2],
            Duration::from_secs(1),
            "the delivering session must reset the backoff so it restarts at 1s"
        );
        // The full schedule must equal what the counter logic produces: two
        // pre-reset drops, then the attempt counter climbs from 1 to the cap.
        // Post-reset there are (MAX_RECONNECT_ATTEMPTS - 1) sleeps for attempts
        // 1..=MAX-1, following the pure `backoff_delay` schedule.
        let mut expected: Vec<Duration> = vec![backoff_delay(1), backoff_delay(2)];
        expected.extend((1..MAX_RECONNECT_ATTEMPTS).map(backoff_delay));
        assert_eq!(
            delays, expected,
            "recovered schedule = 2 pre-reset drops then a fresh 1s->60s climb to the cap"
        );
        // Every recorded delay stays within the [1s, 60s] bounds (Req 6.5).
        for d in &delays {
            let secs = d.as_secs();
            assert!((MIN_BACKOFF_SECS..=MAX_BACKOFF_SECS).contains(&secs));
        }
    }
}
