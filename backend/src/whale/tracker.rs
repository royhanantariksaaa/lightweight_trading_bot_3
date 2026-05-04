use super::PROGRESS_PRINT_STEP;
use super::model::FlowTracker;

pub fn progress(tracker: &FlowTracker) -> f64 {
    if tracker.required_notional <= 0.0 {
        0.0
    } else {
        desired_flow(tracker) / tracker.required_notional
    }
}

pub fn desired_flow(tracker: &FlowTracker) -> f64 {
    if tracker.whale_side == "SELL" {
        tracker.buy_notional
    } else {
        tracker.sell_notional
    }
}

pub fn opposite_flow(tracker: &FlowTracker) -> f64 {
    if tracker.whale_side == "SELL" {
        tracker.sell_notional
    } else {
        tracker.buy_notional
    }
}

pub fn net_pressure(tracker: &FlowTracker) -> f64 {
    desired_flow(tracker) - opposite_flow(tracker)
}

pub fn tracker_signal(tracker: &FlowTracker) -> String {
    let p = progress(tracker);
    let net = net_pressure(tracker);

    if tracker.required_notional <= 0.0 {
        return "NO_LIQUIDITY_ESTIMATE".to_string();
    }

    match (tracker.whale_side.as_str(), p, net > 0.0) {
        ("SELL", p, true) if p >= 1.0 => "RECOVERY_CONFIRMED",
        ("BUY", p, true) if p >= 1.0 => "PULLBACK_CONFIRMED",
        ("SELL", p, true) if p >= 0.70 => "RECOVERY_LIKELY",
        ("BUY", p, true) if p >= 0.70 => "PULLBACK_LIKELY",
        ("SELL", p, _) if p >= 0.30 => "RECOVERY_BUILDING",
        ("BUY", p, _) if p >= 0.30 => "PULLBACK_BUILDING",
        ("SELL", _, _) => "RECOVERY_WEAK",
        _ => "PULLBACK_WEAK",
    }
    .to_string()
}

pub fn should_print_tracker_update(tracker: &mut FlowTracker) -> bool {
    let bucket = (progress(tracker) / PROGRESS_PRINT_STEP).floor() as i32;
    let signal = tracker_signal(tracker);
    let should_print = bucket > tracker.last_printed_bucket || signal != tracker.last_signal;

    if should_print {
        tracker.last_printed_bucket = bucket;
        tracker.last_signal = signal;
    }

    should_print
}
