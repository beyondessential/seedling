//! The evaluation time limit: how much processor time one evaluation of a
//! script may take.
//!
//! One evaluation can span several calls into the engine, such as a pass of
//! an operation, which runs the definition and then the closure. Whoever
//! starts the evaluation holds an [`Evaluation`] for its whole length; the
//! engine's progress hook only checks it.

use std::cell::RefCell;
use std::time::Duration;

use cpu_time::ThreadTime;

/// The value the progress hook terminates an evaluation with when it runs out
/// of time.
const OUT_OF_TIME: &str = "evaluation time limit";

struct Running {
    started: Option<ThreadTime>,
    limit: Option<Duration>,
    exceeded: bool,
}

thread_local! {
    static RUNNING: RefCell<Option<Running>> = const { RefCell::new(None) };
}

/// One evaluation under the evaluation time limit, timed from when it starts
/// until it is dropped.
///
/// An evaluation started while another is running on the same thread is part
/// of that one, and shares its time.
// spec: LIM#measuring-evaluation-time
#[must_use = "the evaluation is timed only while this is held"]
pub struct Evaluation {
    outermost: bool,
}

impl Evaluation {
    /// Start timing an evaluation under `limits`, or join the one running.
    pub fn start(limits: &crate::ScriptLimits) -> Self {
        let outermost = RUNNING.with_borrow_mut(|running| {
            if running.is_some() {
                return false;
            }
            // spec: LIM#limits
            let limit = (limits.max_evaluation_time > 0)
                .then(|| Duration::from_secs(limits.max_evaluation_time));
            *running = Some(Running {
                // A thread whose processor time cannot be read is not timed:
                // its evaluation is still bounded by the operations limit.
                started: limit.and_then(|_| ThreadTime::try_now().ok()),
                limit,
                exceeded: false,
            });
            true
        });
        Self { outermost }
    }

    /// Whether the evaluation has run past the limit, checking now as an
    /// evaluation that returns is checked.
    // spec: LIM#measuring-evaluation-time
    pub fn exceeded(&self) -> bool {
        check()
    }
}

impl Drop for Evaluation {
    fn drop(&mut self) {
        if self.outermost {
            RUNNING.with_borrow_mut(|running| *running = None);
        }
    }
}

/// Whether the running evaluation has run past the limit, recording it if so.
/// No evaluation running has no limit.
fn check() -> bool {
    RUNNING.with_borrow_mut(|running| {
        let Some(running) = running else {
            return false;
        };
        if !running.exceeded
            && let (Some(started), Some(limit)) = (&running.started, running.limit)
        {
            running.exceeded = started.try_elapsed().is_ok_and(|spent| spent > limit);
        }
        running.exceeded
    })
}

/// Stop the engine at its next operation once the running evaluation has run
/// past the limit.
// spec: LIM#measuring-evaluation-time
pub(crate) fn install(engine: &mut rhai::Engine) {
    engine.on_progress(|_| check().then(|| OUT_OF_TIME.into()));
}

/// What an evaluation that ran past the limit reports.
pub fn out_of_time_message(limits: &crate::ScriptLimits) -> String {
    format!(
        "the evaluation ran past the evaluation time limit of {}s",
        limits.max_evaluation_time
    )
}
