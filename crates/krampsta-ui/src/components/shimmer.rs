//! A label with a band of light sweeping across it, for "the agent is working" lines.
//!
//! Rune's recipe (`runtime/tests/shimmer.rs`): a soft-clipped window slides over the dim label
//! while a bright copy inside it slides back by the same amount, so the copy stays on the label.
//! Both run on the render clock, so the view isn't rebuilt per frame; reduced motion holds the
//! sweep still.

use rune::{Color, Easing, El, LoopSpec, TextStyle, el, row, text};

/// The lit band's width, in points.
const BAND: f32 = 36.0;
/// How fast the band travels, in points per second.
const SPEED: f32 = 90.0;

/// `label` in `dim`, with a band of `lit` crossing it every pass. `key` must be unique among
/// the shimmers on screen: the sweep keeps its phase across rebuilds through it.
pub fn view(key: &str, label: &str, font: TextStyle, dim: Color, lit: Color) -> El {
    let (width, _) = rune::text::measure(label, &[], font, None);
    let period = f64::from((width + BAND) / SPEED);
    // Offsets are in band widths: from fully left of the label to fully past its end.
    let (start, end) = (-1.0, width / BAND);
    let sweep = |from: f32, to: f32| LoopSpec::slide(period, [from, 0.0], [to, 0.0]).easing(Easing::LINEAR);
    let copy = el()
        .key(format!("{key}-lit"))
        .absolute(0.0, 0.0)
        .w(BAND)
        .h_full()
        .looping(sweep(-start, -end))
        .child(text(label.to_owned()).font(font).color(lit).absolute(0.0, 0.0));
    let window = el()
        .key(format!("{key}-band"))
        .absolute(0.0, 0.0)
        .w(BAND)
        .h_full()
        .clip_fade(BAND / 2.0, 0.0, BAND / 2.0, 0.0)
        .looping(sweep(start, end))
        .child(copy)
        .a11y_hidden();
    row().key(key.to_owned()).child(text(label.to_owned()).font(font).color(dim)).child(window)
}
