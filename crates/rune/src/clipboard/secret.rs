//! A secret copy's life, as two machines. A [`Secret`] is the copy itself, shared by every clone
//! of its write: what it reports, and whether `clear_now` ran. A [`Hold`] is one clipboard's
//! record of the secret it wrote last, so a clone written to two clipboards is cleaned up on each
//! and neither's report can disarm the other.
use std::sync::{Arc, Mutex};
use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;
use super::{Notify, Request, Route, lock};

/// What became of a secret copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SecretState {
    /// Not written yet.
    #[default]
    Pending,
    /// On the clipboard: the last copy Rune wrote, and the clipboard hasn't changed since.
    Holding,
    /// Cleared by Rune: its delay ran out, or `clear_now` or `Tx::clear_secret_clipboard` ran.
    Cleared,
    /// Something else took its place: a later copy, a change by another app, or a newer copy
    /// before this one was written. Its countdown should end quietly.
    Replaced,
    /// The write failed; the clipboard didn't change.
    Failed(String),
}

/// A secret copy: the last thing reported about it, whether it may be written again, and how many
/// clipboards have a [`Hold`] on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Secret {
    /// `clear_now` hasn't run: a clone of its write can still be written, even after it expired.
    Live { report: SecretState, holds: u32 },
    /// `clear_now` ran: no clone of its write is written again, whatever it last reported.
    Cancelled { report: SecretState, holds: u32 },
    /// `clear_now` ran and a clipboard has been wiped, but others have yet to settle.
    Sweeping { report: SecretState, holds: u32 },
    /// `clear_now` ran, it wiped something, and every clipboard has settled: `Cleared`, even if
    /// one of them turned out to have been replaced.
    Swept,
}
impl Default for Secret {
    fn default() -> Self { Self::Live { report: SecretState::Pending, holds: 0 } }
}
impl Secret {
    fn report(&self) -> SecretState {
        match self {
            Self::Live { report, .. } | Self::Cancelled { report, .. } | Self::Sweeping { report, .. } => report.clone(),
            Self::Swept => SecretState::Cleared,
        }
    }
}

#[derive(Debug)]
pub(super) enum SecretEvent {
    /// `clear_now` ran.
    Cancel,
    /// A clipboard took a hold on this copy.
    Held,
    /// A clipboard's hold ended, with this outcome.
    Released(SecretState),
    /// A clipboard holding this copy was dropped.
    Abandoned,
    /// A write of this copy failed; the latest report stands.
    Report(SecretState),
    /// The last clone of its write was dropped.
    Dropped,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SecretEffect {
    /// Ask every clipboard this copy was written to for a clear.
    Clear,
}

pub(super) fn step(secret: &mut Secret, event: SecretEvent) -> Effects<SecretEffect> {
    use {Secret::*, SecretEvent::*};
    let ask = |holds: u32| if holds > 0 { Effects::one(SecretEffect::Clear) } else { Effects::none() };
    let less = |holds: u32| holds.saturating_sub(1);
    let none = Effects::none;
    let (next, effects) = match (std::mem::take(secret), event) {
        (Live { report, holds } | Cancelled { report, holds }, Cancel) => {
            // Another clipboard expiring doesn't make this one clear yet.
            let report = if holds > 0 && report == SecretState::Cleared { SecretState::Holding } else { report };
            (Cancelled { report, holds }, ask(holds))
        }
        (Sweeping { report, holds }, Cancel) => (Sweeping { report, holds }, ask(holds)),
        (Swept, Cancel) => (Swept, none()),
        (Live { holds, .. }, Held) => (Live { report: SecretState::Holding, holds: holds + 1 }, none()),
        (Cancelled { holds, .. }, Held) => (Cancelled { report: SecretState::Holding, holds: holds + 1 }, none()),
        (Sweeping { holds, .. }, Held) => (Sweeping { report: SecretState::Holding, holds: holds + 1 }, none()),
        (Swept, Held) => (Sweeping { report: SecretState::Holding, holds: 1 }, none()),
        (Live { holds, .. }, Released(report)) => (Live { report, holds: less(holds) }, none()),
        (Cancelled { report, holds }, Released(outcome)) => (match (less(holds), outcome) {
            (0, SecretState::Cleared) => Swept,
            (0, outcome) => Cancelled { report: outcome, holds: 0 },
            (holds, SecretState::Cleared) => Sweeping { report, holds },
            (holds, _) => Cancelled { report, holds },
        }, none()),
        (Sweeping { report, holds }, Released(_) | Abandoned) => (match less(holds) {
            0 => Swept,
            holds => Sweeping { report, holds },
        }, none()),
        (Swept, Released(_) | Abandoned) => (Swept, none()),
        (Live { report, holds }, Abandoned) => (Live { report, holds: less(holds) }, none()),
        (Cancelled { report, holds }, Abandoned) => (Cancelled { report, holds: less(holds) }, none()),
        (Live { holds, .. }, Report(report)) => (Live { report, holds }, none()),
        (Cancelled { holds, .. }, Report(report)) => (Cancelled { report, holds }, none()),
        (Sweeping { holds, .. }, Report(report)) => (Sweeping { report, holds }, none()),
        (Live { report: SecretState::Pending, holds }, Dropped) => (Live { report: SecretState::Replaced, holds }, none()),
        (Cancelled { report: SecretState::Pending, holds }, Dropped) => (Cancelled { report: SecretState::Cleared, holds }, none()),
        (unchanged, Report(_) | Dropped) => (unchanged, none()),
    };
    *secret = next;
    effects
}

#[derive(Default)]
pub(super) struct Slot {
    secret: Mutex<Secret>,
    /// Every clipboard a clone of its write was sent to, the first one first.
    routes: Mutex<Vec<Route>>,
}
impl Slot {
    fn step(&self, event: SecretEvent) -> Effects<SecretEffect> { step(&mut lock(&self.secret), event) }
    pub(super) fn set(&self, state: SecretState) { self.step(SecretEvent::Report(state)); }
    pub(super) fn held(&self) { self.step(SecretEvent::Held); }
    pub(super) fn released(&self, state: SecretState) { self.step(SecretEvent::Released(state)); }
    pub(super) fn abandoned(&self) { self.step(SecretEvent::Abandoned); }
    pub(super) fn cancelled(&self) -> bool { !matches!(*lock(&self.secret), Secret::Live { .. }) }
    pub(super) fn bind(&self, route: Route) {
        let mut routes = lock(&self.routes);
        routes.retain(|r| !r.gone());
        if routes.iter().any(|r| r.same(&route)) { return; }
        let route = if routes.is_empty() { route } else { route.later() };
        routes.push(route);
    }
    fn state(&self) -> SecretState { lock(&self.secret).report() }
}

/// Settles a secret write that was dropped before reaching a clipboard (replaced in its
/// handler, cancelled, or left in a headless runtime with no clipboard).
pub(super) struct Unwritten(pub(super) Arc<Slot>);
impl Drop for Unwritten {
    fn drop(&mut self) { self.0.step(SecretEvent::Dropped); }
}

/// A secret copy from `Tx::copy_secret`. It holds no text, and is cheap to clone and send.
/// Its state is updated by the clipboard worker; asking never reads the OS clipboard.
#[derive(Clone)]
pub struct SecretCopy(pub(super) Arc<Slot>);
impl SecretCopy {
    /// A copy that is never written and stays `Pending`, for scenes and tests that show a copy
    /// in flight without a `Tx`.
    pub fn pending() -> Self { Self(Arc::default()) }
    pub fn state(&self) -> SecretState {
        let first = lock(&self.0.routes).first().cloned();
        if let Some(Route::Memory(m)) = first { m.tick(); }
        self.0.state()
    }
    /// Still the last copy Rune wrote, and the clipboard hasn't changed since. The OS
    /// clipboard is checked a few times a second while a secret is held.
    pub fn holds(&self) -> bool { self.state() == SecretState::Holding }
    /// Clear this copy from every clipboard that still holds it, on the clipboard worker. A copy
    /// not yet written is never written. Idempotent.
    pub fn clear_now(&self) {
        for effect in self.0.step(SecretEvent::Cancel) {
            match effect {
                SecretEffect::Clear => {
                    let routes = lock(&self.0.routes).clone();
                    for route in routes { route.send(Request::Clear(Some(self.0.clone()))); }
                }
            }
        }
    }
}
impl std::fmt::Debug for SecretCopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecretCopy").field(&self.0.state()).finish()
    }
}

/// A timed clear the clipboard's owner is waiting on. It only counts while its attempt is the
/// one the [`Hold`] still has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Alarm {
    pub(super) attempt: Attempt,
    pub(super) due: Instant,
}

/// One clipboard's record of the secret Rune wrote to it last.
#[derive(Default)]
pub(super) enum Hold {
    /// Nothing of ours to clean up.
    #[default]
    Free,
    /// The clipboard may still hold this secret.
    Holding {
        attempt: Attempt,
        slot: Arc<Slot>,
        notify: Option<Notify>,
        /// `None` only when the delay is too long for the clock to represent.
        due: Option<Instant>,
        /// The clipboard's change count after the write, where it has one.
        revision: Option<u64>,
        /// A keyed hash of the text, so the text needn't stay in memory until the clear.
        print: u64,
    },
}
impl Hold {
    pub(super) fn is_free(&self) -> bool { matches!(self, Self::Free) }
    /// Whether a clear of `which`, or of whichever secret is held, applies here.
    pub(super) fn answers(&self, which: Option<&Arc<Slot>>) -> bool {
        match self {
            Self::Free => false,
            Self::Holding { slot, .. } => which.is_none_or(|w| Arc::ptr_eq(slot, w)),
        }
    }
    pub(super) fn armed(&self, alarm: Attempt) -> bool {
        matches!(self, Self::Holding { attempt, .. } if *attempt == alarm)
    }
    pub(super) fn next_check(&self, watch: Instant) -> Option<Instant> {
        let Self::Holding { due, .. } = self else { return None };
        Some(due.map_or(watch, |d| d.min(watch)))
    }
}

pub(super) enum HoldEvent {
    /// A secret reached the clipboard and has been reported as holding it.
    Written { slot: Arc<Slot>, notify: Option<Notify>, due: Option<Instant>, revision: Option<u64>, print: u64 },
    /// A later write took its place, or the clipboard changed under it.
    Lost,
    /// A clear was asked for; `ours` says whether the clipboard still has what Rune wrote.
    Clear { ours: bool },
    /// An alarm came due.
    Due { attempt: Attempt, ours: bool },
}

pub(super) enum HoldEffect {
    /// Wipe the clipboard.
    Wipe,
    Release(Arc<Slot>, SecretState),
    Notify(Notify),
    /// Replace the alarm being waited on: a new hold never inherits the last one's.
    Arm(Option<Alarm>),
}

pub(super) fn step_hold(seq: &mut Seq, hold: &mut Hold, event: HoldEvent) -> Effects<HoldEffect> {
    match event {
        HoldEvent::Written { slot, notify, due, revision, print } => {
            let mut effects = release(hold, SecretState::Replaced);
            let attempt = seq.next();
            *hold = Hold::Holding { attempt, slot, notify: notify.clone(), due, revision, print };
            effects.push(HoldEffect::Arm(due.map(|due| Alarm { attempt, due })));
            if let Some(notify) = notify { effects.push(HoldEffect::Notify(notify)); }
            effects
        }
        HoldEvent::Lost => release(hold, SecretState::Replaced),
        HoldEvent::Clear { ours } => settle(hold, ours),
        HoldEvent::Due { attempt, ours } if hold.armed(attempt) => settle(hold, ours),
        HoldEvent::Due { .. } => Effects::none(),
    }
}

/// Ends a hold on a clear: wipes only what is still ours.
fn settle(hold: &mut Hold, ours: bool) -> Effects<HoldEffect> {
    if hold.is_free() || !ours { return release(hold, SecretState::Replaced); }
    let mut effects = Effects::one(HoldEffect::Wipe);
    effects.extend(release(hold, SecretState::Cleared));
    effects
}

fn release(hold: &mut Hold, state: SecretState) -> Effects<HoldEffect> {
    let Hold::Holding { slot, notify, .. } = std::mem::take(hold) else { return Effects::none() };
    let mut effects = Effects::one(HoldEffect::Release(slot, state));
    if let Some(notify) = notify { effects.push(HoldEffect::Notify(notify)); }
    effects
}
