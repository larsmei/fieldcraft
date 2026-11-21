//! Wall-clock phases of one analysis, recorded on the thread that runs it.
//!
//! The engine installs a [`Guard`] around a solve. The solver calls [`prepared`] when the
//! matrices and boundary conditions are ready, and [`solved`] when the unknowns are known and
//! what remains is stresses, fluxes and the result arrays. A nested solve (a buckling preload,
//! the heat pass of a coupled step) calls [`suppress`] so its marks do not split the outer run.
//! With no guard installed every call is a no-op, which is how tests and scripts solve.

use std::cell::Cell;
use std::time::Instant;

/// Seconds of wall-clock time in each phase of one analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Phases {
    /// Resolving the model, the mesh, assembly and boundary conditions.
    pub prepare_secs: f64,
    /// Factorization and the solve itself: backsolve, Newton, time steps, Lanczos or the flow.
    pub solve_secs: f64,
    /// Stresses, fluxes, reactions and the result arrays after the unknowns are known.
    pub results_secs: f64,
}

impl Phases {
    /// The three phases added.
    pub fn total_secs(self) -> f64 {
        self.prepare_secs + self.solve_secs + self.results_secs
    }
}

#[derive(Clone, Copy)]
struct Rec {
    id: u64,
    start: Instant,
    prepared: Option<Instant>,
    solved: Option<Instant>,
}

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(1) };
    static REC: Cell<Option<Rec>> = const { Cell::new(None) };
    static SUPPRESS: Cell<u32> = const { Cell::new(0) };
}

/// Installed around one analysis. Dropping it discards a record [`Guard::finish`] did not take.
pub struct Guard {
    id: u64,
    live: bool,
}

impl Guard {
    /// Start timing on this thread. Replaces a record a previous guard left behind.
    pub fn install() -> Self {
        let id = NEXT.with(|n| {
            let id = n.get();
            n.set(id.wrapping_add(1));
            id
        });
        REC.set(Some(Rec { id, start: Instant::now(), prepared: None, solved: None }));
        Guard { id, live: true }
    }

    /// The phases so far. The guard then leaves the thread with no record.
    pub fn finish(mut self) -> Phases {
        let phases = phases_of(self.id);
        self.live = false;
        phases
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        REC.with(|c| {
            if c.get().is_some_and(|r| r.id == self.id) {
                c.set(None);
            }
        });
    }
}

/// Restores the suppress depth even when `f` returns early.
struct Suppress;

impl Suppress {
    fn enter() -> Self {
        SUPPRESS.with(|s| s.set(s.get().saturating_add(1)));
        Suppress
    }
}

impl Drop for Suppress {
    fn drop(&mut self) {
        SUPPRESS.with(|s| s.set(s.get().saturating_sub(1)));
    }
}

/// Ignore [`prepared`] and [`solved`] while `f` runs, so a nested solve does not split the
/// outer analysis. The outer clock keeps running.
pub fn suppress<R>(f: impl FnOnce() -> R) -> R {
    let _guard = Suppress::enter();
    f()
}

/// The preparation phase has ended and the solver proper starts. The first call wins.
pub fn prepared() {
    mark(|rec| {
        if rec.prepared.is_none() {
            rec.prepared = Some(Instant::now());
        }
    });
}

/// The unknowns are known and result recovery starts. The first call wins.
pub fn solved() {
    mark(|rec| {
        if rec.solved.is_none() {
            rec.solved = Some(Instant::now());
        }
    });
}

fn mark(update: impl FnOnce(&mut Rec)) {
    if SUPPRESS.with(|s| s.get()) > 0 {
        return;
    }
    REC.with(|c| {
        let Some(mut rec) = c.get() else { return };
        update(&mut rec);
        c.set(Some(rec));
    });
}

fn phases_of(id: u64) -> Phases {
    let Some(rec) = REC.with(|c| {
        let rec = c.get().filter(|r| r.id == id);
        if rec.is_some() {
            c.set(None);
        }
        rec
    }) else {
        return Phases::default();
    };
    let end = Instant::now();
    let prepared_at = rec.prepared.unwrap_or(end);
    let solved_at = rec.solved.map_or(end, |t| if t < prepared_at { prepared_at } else { t });
    Phases {
        prepare_secs: prepared_at.saturating_duration_since(rec.start).as_secs_f64(),
        solve_secs: solved_at.saturating_duration_since(prepared_at).as_secs_f64(),
        results_secs: end.saturating_duration_since(solved_at).as_secs_f64(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Guard, prepared, solved, suppress};

    #[test]
    fn unmarked_time_is_preparation() {
        let guard = Guard::install();
        let phases = guard.finish();
        assert_eq!(phases.solve_secs, 0.0);
        assert_eq!(phases.results_secs, 0.0);
        assert!(phases.prepare_secs >= 0.0);
    }

    #[test]
    fn a_nested_solve_does_not_take_the_phases() {
        let guard = Guard::install();
        suppress(|| {
            prepared();
            solved();
        });
        prepared();
        solved();
        let phases = guard.finish();
        assert!(phases.total_secs() >= 0.0);
        let again = Guard::install();
        suppress(|| prepared());
        let nested = again.finish();
        assert_eq!(nested.solve_secs, 0.0, "a mark inside suppress is not a solver phase");
        assert_eq!(nested.results_secs, 0.0);
    }
}
