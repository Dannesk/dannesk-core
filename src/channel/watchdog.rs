//! The activity log's watchdog: one task for the life of the process, which
//! fails the step a flow is stuck on once the network has had its chance to
//! answer and did not. Desktop ran it in its controller until 2026-10-09; it
//! lives here so that Android inherits it instead of copying it, and so that
//! the failure lands in the channel every interface reads.
//!
//! Armed once per operation, not per step, and it does not measure wall-clock
//! time: link-up silence counts against [`ACTIVITY_BUDGET`], link-down time
//! against [`LINK_DOWN_CAP`], separately. A step landing keeps the budget
//! running — one watchdog measures a whole operation — and the operation
//! ending, being dismissed, or being replaced retires it.

use crate::channel::{ActivityLogState, ActivityStepState, CHANNEL};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Link-up silence an operation may accumulate before the watchdog gives up.
/// No step here takes long — a Bitcoin import lands in 2–3 s, an XRPL
/// transaction validates in 5–10 s — so connected silence past this is a relay
/// or node that is not answering. Rarer than it was: the relay forwards an
/// immediate rejection instead of leaving the client to wait for a validation
/// that cannot come.
pub const ACTIVITY_BUDGET: Duration = Duration::from_secs(15);

/// How long the link may stay down under a live log before the user is let
/// go. Down time does not count against the budget — the log shows it (the
/// amber line), and an answer to something submitted still arrives on a quick
/// reconnect, because the relay keys replies by wallet. But Done is disabled
/// while a log is live, so an outage must not hold the user hostage.
pub const LINK_DOWN_CAP: Duration = Duration::from_secs(20);

/// Why the watchdog stopped waiting. Two different facts, so two different
/// sentences on the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The link was up for `ACTIVITY_BUDGET` in total and nothing landed.
    Silent,
    /// The link has been down for `LINK_DOWN_CAP` without coming back.
    LinkDown,
}

/// Runs for the life of the process: spawn it once on the app's runtime,
/// beside the socket task. It returns only if the channel's sender is gone,
/// which the static `CHANNEL` never lets happen.
pub async fn activity_watchdog() {
    let mut activity = CHANNEL.activity_rx.clone();
    loop {
        // Wait for a live operation.
        let watched = loop {
            let log = activity.borrow_and_update().clone();
            if let Some(log) = log
                && !log.is_terminal()
            {
                break log;
            }
            if activity.changed().await.is_err() {
                return;
            }
        };

        // Its budget runs on the link its answer must arrive over; a flow that
        // named none needs the socket itself (`Channel::link_rx`).
        let budget = budget(CHANNEL.link_rx(watched.health_key));
        tokio::pin!(budget);
        let mut last = watched;
        loop {
            tokio::select! {
                verdict = &mut budget => {
                    // Fail the step still running — in the channel, atomically
                    // with any step landing at this instant. A late answer may
                    // still finish that step: `finish` overwrites the error,
                    // which is the right order for the two true statements.
                    CHANNEL.activity_tx.send_modify(|current| {
                        if let Some(log) = current
                            && !log.is_terminal()
                            && continues(&last, log)
                        {
                            log.fail_active(message(verdict, log.health_key.is_some()).to_string());
                        }
                    });
                    break;
                }
                changed = activity.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let now = activity.borrow_and_update().clone();
                    match now {
                        Some(next) if !next.is_terminal() && continues(&last, &next) => last = next,
                        // Ended, dismissed, or another operation: the outer loop decides.
                        _ => break,
                    }
                }
            }
        }
    }
}

/// Whether `next` is the operation `prev` was: the same title and steps, none
/// of which went backwards. A log that restarted — dismissed and begun again
/// under the same title between two reads of the channel — is a new operation,
/// and its budget starts afresh instead of inheriting the old one's spent time.
fn continues(prev: &ActivityLogState, next: &ActivityLogState) -> bool {
    prev.title == next.title
        && prev.steps.len() == next.steps.len()
        && prev
            .steps
            .iter()
            .zip(&next.steps)
            .all(|(a, b)| a.id == b.id && rank(&b.state) >= rank(&a.state))
}

fn rank(state: &ActivityStepState) -> u8 {
    match state {
        ActivityStepState::Pending => 0,
        ActivityStepState::Active { .. } => 1,
        ActivityStepState::Ok { .. } | ActivityStepState::Error { .. } => 2,
    }
}

/// Deliberately not "Failed". We stopped waiting; that is not the same as the
/// work not happening. A submitted transaction can still land after this
/// fires, and telling someone their send failed when it may be in a ledger is
/// the worse of the two wrong answers. What CAN be said is why we stopped and
/// what to do about it: the ledger is public, the history pane reads it, and
/// nothing here ever re-sends a blob — a signature is the user's to give
/// again. A flow that named a link is a signing flow (`ActivityLogState::begin`);
/// one that did not is an import or the like, where "sign again" would be the
/// wrong instruction.
fn message(verdict: Verdict, signing: bool) -> &'static str {
    match (verdict, signing) {
        (Verdict::Silent, true) => {
            "No answer from the network — it may still complete. Check the history before signing again."
        }
        (Verdict::Silent, false) => "No answer from the network — try again.",
        (Verdict::LinkDown, true) => {
            "Connection lost before the answer arrived — check the history before signing again."
        }
        (Verdict::LinkDown, false) => "Connection lost — try again.",
    }
}

/// Wait out an operation's budget, counting only the time its link is up.
///
/// `rx` is the transport bool for the link the flow's answer must arrive over.
/// Up: the budget runs. Down: the budget pauses and the cap runs instead, reset
/// by every rise (a reconnect re-syncs, and with it an answer gets its chance).
/// Whichever expires first is the verdict.
async fn budget(mut rx: watch::Receiver<bool>) -> Verdict {
    let mut spent = Duration::ZERO;
    let mut down_since: Option<Instant> = None;
    loop {
        // `borrow_and_update` marks the current value seen, so `changed` below
        // waits for the NEXT notification rather than returning at once on this one.
        //
        // A notification is not a transition: a watch `send` wakes receivers
        // even when the value is unchanged, and the socket task re-publishes
        // the transport bools on every reconnect attempt and every proxy link
        // frame. So neither clock may restart on a wake-up — the budget
        // accumulates across them, and the cap runs from the moment the link
        // was first seen down, clearing only when it is seen up again.
        let up = *rx.borrow_and_update();
        let (limit, verdict) = if up {
            down_since = None;
            (ACTIVITY_BUDGET.saturating_sub(spent), Verdict::Silent)
        } else {
            let since = *down_since.get_or_insert_with(Instant::now);
            (LINK_DOWN_CAP.saturating_sub(since.elapsed()), Verdict::LinkDown)
        };
        let started = Instant::now();
        tokio::select! {
            _ = tokio::time::sleep(limit) => return verdict,
            changed = rx.changed() => {
                if up {
                    spent += started.elapsed();
                }
                // The sender lives in the global CHANNEL and never drops; this
                // is unreachable, and the verdict is the harmless way out.
                if changed.is_err() {
                    return verdict;
                }
            }
        }
    }
}
