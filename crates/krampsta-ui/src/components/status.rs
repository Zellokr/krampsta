//! The agent status pill: a dot (or spinner while running) and the status name.

use krampsta_core::{AgentStatus, Usage};
use rune::{El, el, row, text};
use rune_kit::{Kit, Spinner};

use crate::costs::{Cost, format_cost};
use crate::keys;
use crate::theme;

pub fn pill(status: &AgentStatus, k: &Kit) -> El {
    let mark = if status.is_running() {
        Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k)
    } else {
        el().size(8.0, 8.0).radius(4.0).fill(theme::status_color(k, status))
    };
    let label = status.label();
    row()
        .key(keys::STATUS)
        .role("status")
        .label(format!("Agent {}", label.to_lowercase()))
        .h(26.0)
        .px(10.0)
        .gap(7.0)
        .items_center()
        .radius(13.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .child(row().size(12.0, 12.0).center().child(mark))
        .child(text(label).font(k.ty.small_medium).color(k.c.text_muted))
}

/// The tokens a session has used and, when its prices are known, what they cost ("1.2k in ·
/// 340 out · $0.02"), or nothing before the provider counts any.
pub fn usage(usage: Usage, cost: Cost, k: &Kit) -> Option<El> {
    if usage.is_empty() {
        return None;
    }
    let mut shown = format!("{} in · {} out", compact(usage.input), compact(usage.output));
    let mut spoken = format!("Tokens used: {} input, {} output", usage.input, usage.output);
    if let Cost::Known(usd) = cost {
        let cost = format_cost(usd);
        shown = format!("{shown} · {cost}");
        spoken = format!("{spoken}; cost {cost}");
    }
    Some(text(shown).key(keys::USAGE).role("status").label(spoken).font(k.ty.small).color(k.c.text_faint))
}

/// A count as a toolbar shows it: exact below a thousand, then `12.3k` and `1.2M`.
fn compact(n: u64) -> String {
    // Shown, not computed with: the precision a float loses past 2^53 tokens is irrelevant.
    #[expect(clippy::cast_precision_loss, reason = "a display approximation")]
    let x = n as f64;
    match n {
        0..1_000 => n.to_string(),
        1_000..999_950 => trim(format!("{:.1}", x / 1e3)) + "k",
        _ => trim(format!("{:.1}", x / 1e6)) + "M",
    }
}

/// `12.0` → `12`.
fn trim(mut s: String) -> String {
    if s.ends_with(".0") {
        s.truncate(s.len() - 2);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::compact;

    #[test]
    fn counts_shorten_past_a_thousand() {
        let shown: Vec<String> = [0, 999, 1_000, 1_234, 12_049, 999_949, 999_950, 2_500_000].map(compact).into();
        assert_eq!(shown, ["0", "999", "1k", "1.2k", "12k", "999.9k", "1M", "2.5M"]);
    }
}
