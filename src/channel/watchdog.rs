//! The activity log's watchdog

use crate::channel::{ActivityLogState, ActivityStepState, CHANNEL};
use std::time::{Duration, Instant};
use tokio::sync::watch;


pub const ACTIVITY_BUDGET: Duration = Duration::from_secs(15);


pub const LINK_DOWN_CAP: Duration = Duration::from_secs(20);


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Silent,
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

      
        let budget = budget(CHANNEL.link_rx(watched.health_key));
        tokio::pin!(budget);
        let mut last = watched;
        loop {
            tokio::select! {
                verdict = &mut budget => {
                 
                    CHANNEL.activity_tx.send_modify(|current| {
                        if let Some(log) = current
                            && !log.is_terminal()
                            && continues(&last, log)
                        {
                            log.stall_active(message(verdict, log.health_key.is_some()).to_string());
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
        ActivityStepState::Ok { .. }
        | ActivityStepState::Error { .. }
        | ActivityStepState::Stalled { .. } => 2,
    }
}


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


async fn budget(mut rx: watch::Receiver<bool>) -> Verdict {
    let mut spent = Duration::ZERO;
    let mut down_since: Option<Instant> = None;
    loop {
       
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
            
                if changed.is_err() {
                    return verdict;
                }
            }
        }
    }
}
