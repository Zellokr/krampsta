//! A Copy button on each code block of the agent's replies (through rune-kit's
//! `Markdown::code_frame`): a strip above the block with its language and the button. The
//! button copies the block's code as written and says "Copied" until another block is copied
//! or a message is sent. A block still arriving gets no button: half a block isn't worth copying.

use krampsta_core::MessageId;
use rune::{El, Tx, col, row, text};
use rune_kit::markdown::CodeFrame;
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::components::icons;
use crate::keys;
use crate::state::KrampstaApp;

impl KrampstaApp {
    /// Copies block `index` of `message` (its code is `code`) and marks it copied.
    pub fn copy_code(&mut self, message: MessageId, index: usize, code: &str, tx: &mut Tx) {
        tx.copy(code.to_owned());
        self.copied_code = Some((message, index));
    }
}

/// `card` (a code block) with its strip. `copied`: the block of this message copied last, if any.
pub fn frame(card: El, c: &CodeFrame<'_>, message: MessageId, copied: Option<usize>, k: &Kit) -> El {
    let language = text(c.language.unwrap_or_default().to_owned()).font(k.ty.small).color(k.c.text_faint);
    let strip = row().w_full().h(28.0).items_center().justify_between().child(language);
    // A block still arriving has its strip already (nothing jumps when it ends), but no button.
    if c.open {
        return col().w_full().child(strip).child(card);
    }
    let index = c.index;
    let done = copied == Some(index);
    let code = c.source.to_owned();
    let button = Button::new(if done { "Copied" } else { "Copy" })
        .icon(if done { &icons::CHECK } else { &icons::COPY })
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press_tx(move |s: &mut KrampstaApp, tx: &mut Tx| s.copy_code(message, index, &code, tx))
        .build(k)
        .key(keys::code_copy(message, index))
        .label("Copy the code");
    col().w_full().child(strip.child(button)).child(card)
}
