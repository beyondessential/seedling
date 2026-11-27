//! The evaluation time limit: how much processor time one evaluation of a
//! script may take.
//!
//! One evaluation can span several calls into the engine, such as a pass of
//! an operation, which runs the definition and then the closure. Whoever
//! starts the evaluation holds an [`Evaluation`] for its whole length; the
//! engine's progress hook only checks it.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cpu_time::ThreadTime;

/// The value the progress hook terminates an evaluation with when it runs out
/// of time.
const OUT_OF_TIME: &str = "evaluation time limit";

struct Running {
    started: Option<ThreadTime>,
    began: Instant,
    /// When the processor-time clock was last read.
    read_at: Instant,
    limit: Option<Duration>,
    exceeded: bool,
}

thread_local! {
    static RUNNING: RefCell<Option<Running>> = const { RefCell::new(None) };
}

/// Whether reading a thread's processor time has ever failed in this process.
///
/// Whether a thread's processor time can be read is a property of the host,
/// so once it has failed no evaluation is taken to have been measured.
static CLOCK_UNREADABLE: AtomicBool = AtomicBool::new(false);

/// Note that a thread's processor time could not be read, saying so once.
// spec: LIM#measuring-evaluation-time
fn clock_failed(why: impl std::fmt::Display) {
    if !CLOCK_UNREADABLE.swap(true, Ordering::Relaxed) {
        tracing::error!(
            "cannot read a thread's processor time, so evaluations are not held to the \
             evaluation time limit and none is taken to have finished within it: {why}"
        );
    }
}

/// Whether the time evaluations take can be measured: false once reading a
/// thread's processor time has failed.
///
/// An evaluation that could not be timed did not finish within the limit as
/// far as anyone can tell, so nothing that rests on its having done so, such
/// as resetting a breaker, is done for it.
// spec: LIM#measuring-evaluation-time
pub fn time_measured() -> bool {
    !CLOCK_UNREADABLE.load(Ordering::Relaxed)
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
            let now = Instant::now();
            *running = Some(Running {
                // A thread whose processor time cannot be read is not timed:
                // its evaluation is still bounded by the operations limit.
                started: limit.and_then(|_| ThreadTime::try_now().map_err(clock_failed).ok()),
                began: now,
                read_at: now,
                limit,
                exceeded: false,
            });
            true
        });
        Self { outermost }
    }

    /// Whether the evaluation has run past the limit, reading the clock now
    /// as an evaluation that returns is checked.
    // spec: LIM#measuring-evaluation-time
    pub fn exceeded(&self) -> bool {
        check(Read::Now)
    }
}

impl Drop for Evaluation {
    fn drop(&mut self) {
        if self.outermost {
            RUNNING.with_borrow_mut(|running| *running = None);
        }
    }
}

/// How close together the hook may read the processor-time clock.
///
/// Reading a thread's processor time is a system call on every platform the
/// daemon runs on, while reading how much time has elapsed is not: that one
/// is served from the process itself. So the hook reads the elapsed time at
/// every operation and the processor-time clock only this far apart, which
/// bounds how far past the limit an evaluation runs however long each of its
/// operations takes.
const READ_CLOCK_EVERY: Duration = Duration::from_millis(50);

/// Whether to read the processor-time clock now, or only if it has not been
/// read for a while.
#[derive(PartialEq, Eq)]
enum Read {
    Now,
    Spaced,
}

/// Whether the running evaluation has run past the limit, recording it if so.
/// No evaluation running has no limit.
fn check(read: Read) -> bool {
    RUNNING.with_borrow_mut(|running| {
        let Some(running) = running else {
            return false;
        };
        if running.exceeded {
            return true;
        }
        let (Some(started), Some(limit)) = (&running.started, running.limit) else {
            return false;
        };
        if read == Read::Spaced {
            let now = Instant::now();
            // A thread cannot have spent more processor time than has
            // elapsed since it began, so until the limit has elapsed there
            // is nothing the clock could say.
            if now.duration_since(running.began) <= limit
                || now.duration_since(running.read_at) < READ_CLOCK_EVERY
            {
                return false;
            }
            running.read_at = now;
        }
        running.exceeded = match started.try_elapsed() {
            Ok(spent) => spent > limit,
            Err(why) => {
                clock_failed(why);
                false
            }
        };
        running.exceeded
    })
}

/// Stop the engine at one of its next operations once the running evaluation
/// has run past the limit.
// spec: LIM#measuring-evaluation-time
pub(crate) fn install(engine: &mut rhai::Engine) {
    engine.on_progress(|_| check(Read::Spaced).then(|| OUT_OF_TIME.into()));
}

/// What an evaluation that ran past the limit reports.
pub fn out_of_time_message(limits: &crate::ScriptLimits) -> String {
    format!(
        "the evaluation ran past the evaluation time limit of {}s",
        limits.max_evaluation_time
    )
}
