//! The press an overlay's panel holds: from the local monitor seeing a Down until the runtime
//! has its Up. While held, the panel keeps taking mouse events, even off its hit area, so the
//! release reaches it.
//!
//! The monitor sees a click before winit dispatches it, so the seen clicks wait in a queue, and
//! another Down can already be queued when the runtime hears the Up before it. A release can also
//! be lost: the button reads up and winit never dispatches an Up. `Releasing` waits out a grace
//! for one, then gives up and asks the runtime to end the press ([`Effect::ReleaseLost`]).

use std::{collections::VecDeque, time::{Duration, Instant}};
use crate::machine::Effects;

pub(super) type Point = (f32, f32);

/// How long an Up the monitor saw may wait: winit dispatches one within the turn, so one still
/// queued this much later was never delivered (the panel lost its view).
const QUEUED_UP_GRACE: Duration = Duration::from_secs(2);
/// How long the button may read up with no Up seen at all.
const UNSEEN_UP_GRACE: Duration = Duration::from_millis(100);
/// How often to read the button of a press the monitor never saw: nothing else may wake the
/// loop when its Up is lost.
const UNSEEN_PRESS_CHECK: Duration = Duration::from_millis(100);

fn grace(queued_up: bool) -> Duration { if queued_up { QUEUED_UP_GRACE } else { UNSEEN_UP_GRACE } }

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Click {
    pub(super) down: bool,
    pub(super) at: Point,
    /// On a Down: the point where the press before it lost its release, to end that press first.
    pub(super) ends: Option<Point>,
}

impl Click {
    fn up(at: Point) -> Self { Self { down: false, at, ends: None } }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Hold {
    /// The panel holds no press. `ups` are releases still to dispatch, of presses the monitor
    /// never saw, and `unseen` is a press winit dispatched that the monitor never saw.
    Free { ups: VecDeque<Point>, unseen: Unseen },
    /// A press is held, the button down or its Up still on the way. `at` is the newest press's
    /// Down, then where the button was last read down.
    Pressed { clicks: VecDeque<Click>, at: Point },
    /// The button read up at `at` and `since`, and the runtime hasn't had the Up.
    Releasing { clicks: VecDeque<Click>, since: Instant, at: Point },
    /// The button read up at `at`, then down again before any click was seen: the next Down's
    /// event may be on its way.
    Rebound { clicks: VecDeque<Click>, at: Point },
}

impl Default for Hold {
    fn default() -> Self { Self::Free { ups: VecDeque::new(), unseen: Unseen::Idle } }
}

/// A press the runtime holds that the monitor never saw (no monitor was running). The panel
/// doesn't capture for it, so its normal click is untouched; this only notices a lost Up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Unseen {
    Idle,
    /// The button reads down at `at`; read it again at `check`.
    Down { at: Point, check: Instant },
    /// The button read up at `at` and `since`.
    Rose { at: Point, since: Instant },
    /// The button read up at `at`, then down again; read it again at `check`.
    Rebound { at: Point, check: Instant },
}

impl Unseen {
    fn point(self) -> Option<Point> {
        match self {
            Self::Idle => None,
            Self::Down { at, .. } | Self::Rose { at, .. } | Self::Rebound { at, .. } => Some(at),
        }
    }

    fn deadline(self, queued_up: bool) -> Option<Instant> {
        match self {
            Self::Idle => None,
            Self::Down { check, .. } | Self::Rebound { check, .. } => Some(check),
            Self::Rose { since, .. } => Some(since + grace(queued_up)),
        }
    }
}

impl Hold {
    pub(super) fn captured(&self) -> bool { !matches!(self, Self::Free { .. }) }

    /// Whether a press is being held, captured or not.
    pub(super) fn holds_press(&self) -> bool { !matches!(self, Self::Free { unseen: Unseen::Idle, .. }) }

    /// When a lost release gives up, for the event loop to sample the pointer then even if
    /// nothing else moves.
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Free { ups, unseen } => unseen.deadline(!ups.is_empty()),
            Self::Releasing { clicks, since, .. } => Some(*since + grace(clicks.iter().any(|click| !click.down))),
            Self::Pressed { .. } | Self::Rebound { .. } => None,
        }
    }

    pub(super) fn into_clicks(self) -> VecDeque<Click> {
        match self {
            Self::Free { ups, .. } => ups.into_iter().map(Click::up).collect(),
            Self::Pressed { clicks, .. } | Self::Releasing { clicks, .. } | Self::Rebound { clicks, .. } => clicks,
        }
    }

    fn queued_down(&self) -> bool {
        match self {
            Self::Free { .. } => false,
            Self::Pressed { clicks, .. } | Self::Releasing { clicks, .. } | Self::Rebound { clicks, .. } => clicks.iter().any(|click| click.down),
        }
    }

    /// Whether a sampled position is newer than the press the runtime holds: the button has
    /// risen since, or a click is still on its way to it.
    pub(super) fn ahead_of_clicks(&self) -> bool {
        match self {
            Self::Free { ups, unseen } => match unseen {
                Unseen::Idle => false,
                Unseen::Down { .. } => !ups.is_empty(),
                Unseen::Rose { .. } | Unseen::Rebound { .. } => true,
            },
            Self::Releasing { .. } | Self::Rebound { .. } => true,
            Self::Pressed { clicks, .. } => !clicks.is_empty(),
        }
    }

    /// Where the press the runtime holds let go, if that's known: its own queued Up, the point a
    /// queued press carries for it, or where the button rose.
    fn lost_point(&self) -> Option<Point> {
        let (clicks, rose) = match self {
            Self::Free { ups, unseen } => return ups.front().copied().or(unseen.point()),
            Self::Pressed { clicks, at } | Self::Releasing { clicks, at, .. } | Self::Rebound { clicks, at } => (clicks, *at),
        };
        clicks.front().and_then(|click| if click.down { click.ends } else { Some(click.at) }).or(Some(rose))
    }

    /// Where a press is last known to have been down.
    fn known_point(&self) -> Option<Point> {
        match self {
            Self::Free { unseen, .. } => unseen.point(),
            Self::Pressed { at, .. } | Self::Releasing { at, .. } | Self::Rebound { at, .. } => Some(*at),
        }
    }

    /// Where the press before a new Down lost its release, if no Up is queued between them.
    fn lost_before(&self) -> Option<Point> {
        let (clicks, rose) = match self {
            Self::Free { ups, unseen } => return unseen.point().filter(|_| ups.is_empty()),
            Self::Pressed { clicks, at } | Self::Releasing { clicks, at, .. } | Self::Rebound { clicks, at } => (clicks, *at),
        };
        clicks.back().is_none_or(|click| click.down).then_some(rose)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Event {
    /// The local monitor saw a click; `over` is whether it fell on the hit area.
    Seen { down: bool, at: Point, over: bool },
    /// winit dispatched a press (`down`) or a release.
    Dispatched { down: bool },
    /// winit dispatched a press at `at` that the monitor never saw, and the runtime holds it.
    UnseenPress { at: Point, now: Instant },
    /// winit is about to report a position, which belongs to the click at the front of the queue.
    Positioning,
    /// AppKit sent `mouseMoved:`, which it only does with no button down: as good as reading the
    /// button up, at wherever the press was last known.
    Hovered { now: Instant },
    /// The runtime has had the release, or a system drag took it.
    Released,
    /// A sample of the pointer and the primary button.
    Sampled { button_down: bool, now: Instant, at: Point },
    /// winit's view was out of the panel, so the clicks seen meanwhile never reached it.
    ViewLost { last: Option<Point> },
    /// The panel hid.
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Effect {
    /// Where the click winit just dispatched happened.
    Deliver(Point),
    /// The press ended without an Up: the runtime should give it up at this point.
    ReleaseLost(Point),
}

pub(super) fn step(hold: &mut Hold, event: Event) -> Effects<Effect> {
    match event {
        Event::Seen { down, at, over } => {
            match hold {
                Hold::Free { .. } if !over => {}
                Hold::Free { ups, .. } if !down => ups.push_back(at),
                Hold::Pressed { clicks, .. } | Hold::Releasing { clicks, .. } | Hold::Rebound { clicks, .. } if !down => clicks.push_back(Click::up(at)),
                _ => {
                    let ends = hold.lost_before();
                    let mut clicks = std::mem::take(hold).into_clicks();
                    clicks.push_back(Click { down: true, at, ends });
                    *hold = Hold::Pressed { clicks, at };
                }
            }
            Effects::none()
        }
        Event::Dispatched { down } => {
            // A press nothing saw is still held, so its Up never came.
            if down && let Hold::Free { unseen, .. } = hold && let Some(at) = unseen.point() {
                *unseen = Unseen::Idle;
                return Effects::one(Effect::ReleaseLost(at));
            }
            let click = match hold {
                Hold::Free { ups, .. } if !down => ups.pop_front().map(Click::up),
                Hold::Pressed { clicks, .. } | Hold::Releasing { clicks, .. } | Hold::Rebound { clicks, .. } if clicks.front().is_some_and(|click| click.down == down) =>
                    clicks.pop_front(),
                _ => None,
            };
            let Some(click) = click else { return Effects::none() };
            // The press before this one ends first, so this one can't replace it.
            let mut effects = click.ends.map_or(Effects::none(), |at| Effects::one(Effect::ReleaseLost(at)));
            effects.push(Effect::Deliver(click.at));
            effects
        }
        Event::UnseenPress { at, now } => {
            if let Hold::Free { unseen, .. } = hold { *unseen = Unseen::Down { at, check: now + UNSEEN_PRESS_CHECK }; }
            Effects::none()
        }
        Event::Hovered { now } => match hold.known_point() {
            Some(at) => sampled(hold, false, now, at),
            None => Effects::none(),
        },
        Event::Positioning => {
            let ends = match hold {
                Hold::Free { .. } => None,
                Hold::Pressed { clicks, .. } | Hold::Releasing { clicks, .. } | Hold::Rebound { clicks, .. } =>
                    clicks.front_mut().and_then(|click| click.ends.take()),
            };
            ends.map_or(Effects::none(), |at| Effects::one(Effect::ReleaseLost(at)))
        }
        Event::Released => {
            // Native callbacks can queue another Down before winit dispatches this Up, and
            // what the button did since is that press's.
            if !hold.queued_down() {
                let clicks = std::mem::take(hold).into_clicks();
                *hold = Hold::Free { ups: clicks.into_iter().map(|click| click.at).collect(), unseen: Unseen::Idle };
            }
            Effects::none()
        }
        Event::Sampled { button_down, now, at } => sampled(hold, button_down, now, at),
        Event::ViewLost { last } => {
            let lost = hold.lost_point().or(last);
            *hold = Hold::default();
            lost.map_or(Effects::none(), |at| Effects::one(Effect::ReleaseLost(at)))
        }
        Event::Stopped => {
            *hold = Hold::default();
            Effects::none()
        }
    }
}

fn sampled(hold: &mut Hold, button_down: bool, now: Instant, point: Point) -> Effects<Effect> {
    let due = hold.deadline();
    match hold {
        Hold::Free { unseen, .. } => match *unseen {
            Unseen::Idle => Effects::none(),
            Unseen::Down { .. } if button_down => {
                *unseen = Unseen::Down { at: point, check: now + UNSEEN_PRESS_CHECK };
                Effects::none()
            }
            Unseen::Rose { at, .. } | Unseen::Rebound { at, .. } if button_down => {
                *unseen = Unseen::Rebound { at, check: now + UNSEEN_PRESS_CHECK };
                Effects::none()
            }
            Unseen::Down { .. } => {
                *unseen = Unseen::Rose { at: point, since: now };
                Effects::none()
            }
            Unseen::Rebound { at, .. } => {
                *unseen = Unseen::Rose { at, since: now };
                Effects::none()
            }
            Unseen::Rose { .. } => give_up_when_due(hold, due, now),
        },
        Hold::Pressed { at, .. } if button_down => {
            *at = point;
            Effects::none()
        }
        Hold::Rebound { .. } if button_down => Effects::none(),
        Hold::Pressed { clicks, .. } => {
            *hold = Hold::Releasing { clicks: std::mem::take(clicks), since: now, at: point };
            Effects::none()
        }
        Hold::Rebound { clicks, at } => {
            *hold = Hold::Releasing { clicks: std::mem::take(clicks), since: now, at: *at };
            Effects::none()
        }
        Hold::Releasing { clicks, at, .. } if button_down => {
            *hold = Hold::Rebound { clicks: std::mem::take(clicks), at: *at };
            Effects::none()
        }
        Hold::Releasing { .. } => give_up_when_due(hold, due, now),
    }
}

fn give_up_when_due(hold: &mut Hold, due: Option<Instant>, now: Instant) -> Effects<Effect> {
    if due.is_some_and(|due| now < due) { return Effects::none(); }
    let lost = hold.lost_point();
    *hold = Hold::default();
    lost.map_or(Effects::none(), |at| Effects::one(Effect::ReleaseLost(at)))
}

#[cfg(test)]
#[path = "hold_tests.rs"]
mod tests;
