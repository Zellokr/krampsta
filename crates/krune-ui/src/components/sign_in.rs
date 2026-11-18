//! The sign-in dialog: the device code to enter, a button that copies it and opens the page,
//! and what went wrong if it failed. Mounted from the root view on any screen.

use rune::{El, TextStyle, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, Kit};

use crate::accounts::SignIn;
use crate::keys;
use crate::state::KruneApp;

pub fn dialog(s: &KruneApp, k: &Kit, layer: &mut Vec<El>) {
    let Some(provider) = s.sign_in.provider() else { return };
    let label = s.provider_label(provider);
    let body = match &s.sign_in {
        SignIn::Closed => return,
        SignIn::Starting { .. } => {
            col().child(text(format!("Asking {label} for a sign-in code…")).font(k.ty.ui).color(k.c.text_muted))
        }
        SignIn::Code { code, .. } => {
            let big = TextStyle { size: 26.0, line_height: 34.0, ..k.ty.mono };
            col()
                .w_full()
                .gap(14.0)
                .child(
                    text(format!("Open {} and enter this code to let Krune use your {label} account:", code.url))
                        .font(k.ty.ui)
                        .color(k.c.text_base)
                        .wrap(),
                )
                .child(
                    text(code.code.clone())
                        .key(keys::SIGN_IN_CODE)
                        .font(big)
                        .color(k.c.text_base)
                        .selectable(true)
                        .selection_color(k.selection()),
                )
                .child(
                    row().child(
                        Button::new("Copy code and open the page")
                            .variant(ButtonVariant::Contrast)
                            .control_size(ControlSize::Regular)
                            .on_press_tx(|s: &mut KruneApp, tx: &mut Tx| s.open_sign_in_page(tx))
                            .build(k)
                            .key(keys::SIGN_IN_OPEN),
                    ),
                )
                .child(
                    text("Waiting for your approval. Closing this cancels the sign-in.")
                        .font(k.ty.small)
                        .color(k.c.text_faint)
                        .wrap(),
                )
        }
        SignIn::Failed { provider, error } => {
            let retry = provider.clone();
            col()
                .w_full()
                .gap(14.0)
                .child(text(error.clone()).font(k.ty.ui).color(k.c.danger.icon).wrap().selectable(true))
                .child(
                    row().child(
                        Button::new("Try again")
                            .variant(ButtonVariant::Neutral)
                            .control_size(ControlSize::Regular)
                            .on_press(move |s: &mut KruneApp| s.start_sign_in(&retry))
                            .build(k)
                            .key(keys::SIGN_IN_RETRY),
                    ),
                )
        }
    };
    Dialog::new(keys::SIGN_IN, format!("Sign in to {label}"))
        .open(true)
        .fit()
        .width(440.0)
        // The dialog pads its header, not its body: match the header's 16-point inset.
        .child(body.px(16.0).pb(18.0))
        .on_close(|s: &mut KruneApp| s.close_sign_in())
        .build(k, layer);
}
