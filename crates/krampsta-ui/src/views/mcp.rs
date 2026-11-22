//! The "MCP servers" part of Settings: a card per server (its state, its tools or why it
//! failed, a switch that starts or stops it, and Remove, which asks for a second click), and
//! the form that adds one. What they do lives in `servers.rs`.

use krampsta_agent::{ServerState, ServerStatus};
use rune::{El, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Spinner, Switch, TextArea, TextInput};

use crate::servers::ServerForm;
use crate::state::KrampstaApp;
use crate::{keys, theme};

/// The part, when there are servers to manage (the app handed them over).
pub fn settings(s: &KrampstaApp, k: &Kit) -> Option<El> {
    let source = s.servers.source.as_ref()?;
    let file = source.config_file().map(crate::projects::display_path);
    let mut part = col().w_full().gap(12.0).child(text("MCP servers").font(k.ty.small_medium).color(k.c.text_faint));
    let note = match &file {
        Some(file) => format!(
            "Saved in {file}, in the format Claude Desktop uses. A change here applies at once; one made \
             in the file, when Krampsta starts."
        ),
        None => "A change here applies at once.".to_owned(),
    };
    part = part.child(text(note).font(k.ty.small).color(k.c.text_muted).wrap().selectable(true));
    if s.servers.list.is_empty() && !s.servers.form.open {
        let empty = card(k)
            .key(keys::MCP_SERVERS_EMPTY)
            .child(text("No servers yet").font(k.ty.ui_medium).color(k.c.text_base))
            .child(
                text("Add one below, or paste a Claude Desktop file there and start Krampsta again.")
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .wrap(),
            );
        part = part.child(empty);
    }
    let confirm = s.servers.confirm_remove.as_deref();
    part = part.children(s.servers.list.iter().map(|server| server_card(k, server, confirm == Some(&server.name))));
    Some(part.child(add(s, k)))
}

fn card(k: &Kit) -> El {
    col().w_full().py(12.0).px(16.0).gap(4.0).radius(k.radius.md).fill(k.c.layer_01).border(1.0, k.c.border_weak)
}

/// One server: its name and how it is reached, its state and controls, and its tools or why it
/// failed. `confirming`: Remove was clicked once.
fn server_card(k: &Kit, server: &ServerStatus, confirming: bool) -> El {
    let name = server.name.clone();
    let enabled = server.state != ServerState::Disabled;
    let toggle = Switch::new(enabled)
        .on_toggle({
            let name = name.clone();
            move |s: &mut KrampstaApp| s.toggle_server(&name)
        })
        .build(k)
        .key(keys::mcp_server_toggle(&name))
        .label(format!("{name} enabled"));
    let remove = if confirming {
        row()
            .gap(6.0)
            .child(
                Button::new("Cancel")
                    .variant(ButtonVariant::Ghost)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut KrampstaApp| s.cancel_remove_server())
                    .build(k),
            )
            .child(
                Button::new("Remove for good")
                    .variant(ButtonVariant::Danger)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut KrampstaApp| s.confirm_remove_server())
                    .build(k)
                    .key(keys::mcp_server_remove_confirm(&name)),
            )
    } else {
        let asked = name.clone();
        Button::new("Remove")
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut KrampstaApp| s.ask_remove_server(&asked))
            .build(k)
            .key(keys::mcp_server_remove(&name))
    };
    let head = row()
        .w_full()
        .gap(16.0)
        .items_center()
        .justify_between()
        .child(
            col()
                .gap(2.0)
                .shrink()
                .min_w(0.0)
                .child(text(name.clone()).font(k.ty.ui_medium).color(k.c.text_base))
                .child(text(server.reached.clone()).font(k.ty.small).color(k.c.text_faint).truncate()),
        )
        .child(
            row()
                .gap(12.0)
                .items_center()
                .child(state(k, &server.state).key(keys::mcp_server_state(&name)))
                .child(toggle),
        );
    let detail = match &server.state {
        ServerState::Ready { tools } if !tools.is_empty() => {
            Some(text(tools.join(", ")).font(k.ty.small).color(k.c.text_muted).wrap().selectable(true))
        }
        ServerState::Failed(why) => {
            Some(text(why.clone()).font(k.ty.small).color(k.c.danger.icon).wrap().selectable(true))
        }
        _ => None,
    };
    let footer = if confirming {
        row()
            .w_full()
            .gap(12.0)
            .items_center()
            .justify_between()
            .child(
                text("Its settings, tokens included, leave the file.")
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .wrap()
                    .shrink()
                    .min_w(0.0),
            )
            .child(remove)
    } else {
        row().w_full().justify_end().child(remove)
    };
    card(k).key(keys::mcp_server(&name)).child(head).children(detail.map(|d| d.mt(6.0))).child(footer.mt(4.0))
}

/// Where a server is, as a dot (or a spinner) and a word.
fn state(k: &Kit, state: &ServerState) -> El {
    let dot = |color| el().size(8.0, 8.0).radius(4.0).fill(color);
    let (mark, label) = match state {
        ServerState::Connecting => {
            (Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k), "Connecting…".to_owned())
        }
        ServerState::Ready { tools } => {
            let label = match tools.len() {
                1 => "Connected · 1 tool".to_owned(),
                n => format!("Connected · {n} tools"),
            };
            (dot(k.c.success.icon), label)
        }
        ServerState::Failed(_) => (dot(k.c.danger.icon), "Failed".to_owned()),
        ServerState::Disabled => (dot(k.c.text_faint), "Off".to_owned()),
    };
    row().gap(6.0).items_center().child(mark).child(text(label).font(k.ty.small).color(k.c.text_muted))
}

/// "Add a server": a button, or the form once it is open.
fn add(s: &KrampstaApp, k: &Kit) -> El {
    let form = &s.servers.form;
    if !form.open {
        let open = Button::new("Add a server")
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Regular)
            .on_press(|s: &mut KrampstaApp| s.open_server_form())
            .build(k)
            .key(keys::MCP_ADD_OPEN);
        return row().w_full().child(open);
    }
    let kind = SegmentedControl::new(&["Command", "URL"], Some(usize::from(form.url)))
        .width(Some(180.0))
        .on_pick(|s: &mut KrampstaApp, i, _: &mut Tx| {
            s.servers.form.url = i == 1;
            s.servers.form.error = None;
        })
        .build(k)
        .key(keys::MCP_ADD_KIND);
    let name = TextInput::new(&form.name)
        .placeholder("github")
        .label("Server name")
        .width(None)
        .on_edit(|s: &mut KrampstaApp| &mut s.servers.form.name)
        .build(k)
        .key(keys::MCP_ADD_NAME);
    let (target_hint, extra_hint, extra_label) = target_hints(form);
    let target = TextInput::new(&form.target)
        .placeholder(target_hint)
        .label(if form.url { "Server URL" } else { "Command" })
        .width(None)
        .on_edit(|s: &mut KrampstaApp| &mut s.servers.form.target)
        .on_submit(|s: &mut KrampstaApp, _: &mut Tx| s.add_server())
        .build(k)
        .key(keys::MCP_ADD_TARGET);
    let extra = TextArea::new(&form.extra)
        .placeholder(extra_hint)
        .label(extra_label)
        .rows(2)
        .width(None)
        .on_edit(|s: &mut KrampstaApp| &mut s.servers.form.extra)
        .build(k)
        .key(keys::MCP_ADD_EXTRA);
    let caption = |t: &'static str| text(t).font(k.ty.small_medium).color(k.c.text_muted);
    let actions = row()
        .w_full()
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Regular)
                .on_press(|s: &mut KrampstaApp| s.close_server_form())
                .build(k),
        )
        .child(
            Button::new("Add")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Regular)
                .disabled(form.name.text.trim().is_empty() || form.target.text.trim().is_empty())
                .on_press(|s: &mut KrampstaApp| s.add_server())
                .build(k)
                .key(keys::MCP_ADD_SAVE),
        );
    let mut body = card(k)
        .key(keys::MCP_ADD_FORM)
        .gap(8.0)
        .child(row().w_full().items_center().justify_between().child(caption("Add a server")).child(kind))
        .child(name)
        .child(target)
        .child(caption(extra_label).mt(4.0))
        .child(extra);
    if let Some(error) = &form.error {
        body = body.child(text(error.clone()).font(k.ty.small).color(k.c.danger.icon).wrap().key(keys::MCP_ADD_ERROR));
    }
    body.child(actions.mt(4.0))
}

/// The placeholders and label the form shows for its kind of server.
fn target_hints(form: &ServerForm) -> (&'static str, &'static str, &'static str) {
    if form.url {
        ("https://example.com/mcp", "Authorization: Bearer …", "Headers, one per line (optional)")
    } else {
        ("npx -y @modelcontextprotocol/server-github", "GITHUB_TOKEN=…", "Environment, one per line (optional)")
    }
}
