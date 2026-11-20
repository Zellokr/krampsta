//! What a session cost: the tokens each model used times the prices the user set in settings
//! (US dollars per million tokens). Only providers billed by API key count; accounts (a Copilot
//! or ChatGPT plan), local servers and the simulated agent cost nothing here.
//!
//! No price ships with the app: prices change, and the user's own (their plan, a discount, a
//! compatible server's rates) are the ones that matter.

use krampsta_agent::ProviderKind;
use krampsta_core::{Session, Usage};
use rune::TextEdit;
use serde::{Deserialize, Serialize};

use crate::state::{KrampstaApp, Screen};

/// What one model costs, in US dollars per million tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

impl Price {
    /// What `usage` costs at this price, in US dollars.
    pub fn of(self, usage: Usage) -> f64 {
        // Token counts are far below 2^53, where a float stops being exact.
        #[expect(clippy::cast_precision_loss, reason = "token counts stay far below 2^53")]
        let (input, output) = (usage.input as f64, usage.output as f64);
        (input * self.input + output * self.output) / 1_000_000.0
    }
}

/// A session's cost, as far as the prices set tell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cost {
    /// Nothing it used is billed by key.
    Free,
    /// In US dollars.
    Known(f64),
    /// A billed model it used has no price yet.
    Unpriced,
}

/// The settings fields for one model's price.
#[derive(Debug)]
pub struct PriceField {
    pub provider: String,
    pub model: String,
    pub input: TextEdit,
    pub output: TextEdit,
}

/// A price field that doesn't read as dollars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a price is a number of dollars, 0 or more")]
pub struct NotDollars;

/// `text` as dollars: blank is `None`, a leading `$` and a decimal comma are fine, anything
/// else not a number ≥ 0 is an error.
pub fn parse_dollars(text: &str) -> Result<Option<f64>, NotDollars> {
    let text = text.trim().trim_start_matches('$').trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.replace(',', ".").parse::<f64>() {
        Ok(x) if x.is_finite() && x >= 0.0 => Ok(Some(x)),
        _ => Err(NotDollars),
    }
}

/// A cost as the toolbar shows it: `$0.42`, `$12.30`, and `<$0.01` for less than a cent.
pub fn format_cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 { "<$0.01".to_owned() } else { format!("${usd:.2}") }
}

/// A price as its field shows it: `2.5`, not `2.500000`.
fn shown(x: f64) -> String {
    let s = format!("{x:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl KrampstaApp {
    /// Whether `provider`'s tokens cost money (it's reached with an API key).
    pub fn is_billed(&self, provider: &str) -> bool {
        self.engine.providers.iter().any(|p| p.id == provider && p.kind == ProviderKind::Cloud)
    }

    pub fn price(&self, provider: &str, model: &str) -> Option<Price> {
        self.prices.get(&(provider.to_owned(), model.to_owned())).copied()
    }

    /// What `session` cost so far at the prices set.
    pub fn cost(&self, session: &Session) -> Cost {
        let mut total = 0.0;
        let mut billed = false;
        for used in session.by_model.iter().filter(|m| self.is_billed(&m.provider)) {
            billed = true;
            match self.price(&used.provider, &used.model) {
                Some(price) => total += price.of(used.usage),
                None => return Cost::Unpriced,
            }
        }
        if billed { Cost::Known(total) } else { Cost::Free }
    }

    /// Shows the settings, with a price field for every billed model in use: the ones sessions
    /// used, the one selected, and any that has a price already.
    pub fn open_settings(&mut self) {
        let mut models: Vec<(String, String)> = self.prices.keys().cloned().collect();
        let used = self.sessions.iter().flat_map(|s| &s.by_model).map(|m| (m.provider.clone(), m.model.clone()));
        let selected = self.engine.selection.model.clone().map(|m| (self.engine.selection.provider.clone(), m));
        models.extend(used.chain(selected).filter(|(provider, _)| self.is_billed(provider)));
        models.sort();
        models.dedup();
        self.price_fields = models
            .into_iter()
            .map(|(provider, model)| {
                let price = self.price(&provider, &model);
                PriceField {
                    input: TextEdit::new(price.map(|p| shown(p.input)).unwrap_or_default()),
                    output: TextEdit::new(price.map(|p| shown(p.output)).unwrap_or_default()),
                    provider,
                    model,
                }
            })
            .collect();
        self.screen = Screen::Settings;
    }

    /// A price field's text, for its input to edit (`output`: the output price's). A field
    /// that went away while a press was in flight edits a scratch one instead.
    pub fn price_text(&mut self, index: usize, output: bool) -> &mut TextEdit {
        match self.price_fields.get_mut(index) {
            Some(f) if output => &mut f.output,
            Some(f) => &mut f.input,
            None => &mut self.price_scratch,
        }
    }

    /// Takes a price field's text as the model's price once both read as dollars; blank in both
    /// removes the price. A field that isn't a number leaves the price as it was.
    pub fn edit_price(&mut self, index: usize) {
        let Some(field) = self.price_fields.get(index) else { return };
        let key = (field.provider.clone(), field.model.clone());
        match (parse_dollars(&field.input.text), parse_dollars(&field.output.text)) {
            (Ok(None), Ok(None)) => {
                self.prices.remove(&key);
            }
            (Ok(input), Ok(output)) => {
                let price = Price { input: input.unwrap_or(0.0), output: output.unwrap_or(0.0) };
                self.prices.insert(key, price);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig, Provider, Selection};
    use krampsta_core::{AgentEvent, ModelUsage, RunId, SessionId};

    use super::*;

    fn provider(id: &str, kind: ProviderKind) -> Provider {
        Provider {
            id: id.into(),
            label: id.into(),
            default_model: Some("m".into()),
            missing: None,
            kind,
            reasoning_effort: false,
            api_key: None,
        }
    }

    /// An app that knows a billed provider ("openai") and a local one ("ollama").
    fn app() -> KrampstaApp {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
        app.engine.providers = vec![provider("openai", ProviderKind::Cloud), provider("ollama", ProviderKind::Local)];
        app
    }

    fn used(provider: &str, model: &str, input: u64, output: u64) -> ModelUsage {
        ModelUsage { provider: provider.into(), model: model.into(), usage: Usage { input, output } }
    }

    #[test]
    fn a_session_costs_its_billed_tokens_at_the_prices_set() {
        let mut app = app();
        let mut s = Session::new(SessionId(1));
        assert_eq!(app.cost(&s), Cost::Free, "nothing used");
        s.by_model = vec![used("ollama", "llama", 5_000_000, 5_000_000)];
        assert_eq!(app.cost(&s), Cost::Free, "local models cost nothing");
        s.by_model.push(used("openai", "gpt-a", 2_000_000, 500_000));
        assert_eq!(app.cost(&s), Cost::Unpriced);
        app.prices.insert(("openai".into(), "gpt-a".into()), Price { input: 2.0, output: 8.0 });
        assert_eq!(app.cost(&s), Cost::Known(8.0), "2M in at $2 + 0.5M out at $8");
    }

    #[test]
    fn price_fields_cover_billed_models_in_use_and_edits_become_prices() {
        let mut app = app();
        app.engine.selection = Selection { provider: "openai".into(), model: Some("gpt-b".into()) };
        let mut s = Session::new(SessionId(1));
        s.by_model = vec![used("openai", "gpt-a", 1, 1), used("ollama", "llama", 1, 1)];
        app.sessions.push(s);
        app.open_settings();
        let models: Vec<&str> = app.price_fields.iter().map(|f| f.model.as_str()).collect();
        assert_eq!(models, ["gpt-a", "gpt-b"], "billed ones only, used and selected");
        assert_eq!(app.screen, Screen::Settings);

        app.price_fields[0].input = TextEdit::new("$2.50");
        app.edit_price(0);
        assert_eq!(app.price("openai", "gpt-a"), Some(Price { input: 2.5, output: 0.0 }));
        app.price_fields[0].output = TextEdit::new("ten");
        app.edit_price(0);
        assert_eq!(app.price("openai", "gpt-a"), Some(Price { input: 2.5, output: 0.0 }), "kept");
        app.price_fields[0].input = TextEdit::new("");
        app.price_fields[0].output = TextEdit::new(" ");
        app.edit_price(0);
        assert_eq!(app.price("openai", "gpt-a"), None, "blank removes it");
    }

    #[test]
    fn sending_bills_the_run_to_the_selected_model() {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
        let id = app.new_session();
        app.draft = TextEdit::new("hi");
        let run: RunId = app.send().unwrap();
        let session = app.sessions.iter_mut().find(|s| s.id == id).unwrap();
        session.apply(&AgentEvent::Usage { run, usage: Usage { input: 3, output: 4 } });
        let selection = &app.engine.selection;
        let by = &app.session(id).unwrap().by_model;
        assert_eq!(by.len(), 1);
        assert_eq!((&by[0].provider, Some(&by[0].model)), (&selection.provider, selection.model.as_ref()));
    }

    #[test]
    fn dollars_parse_leniently_and_show_briefly() {
        assert_eq!(parse_dollars(" $1,25 "), Ok(Some(1.25)));
        assert_eq!(parse_dollars(""), Ok(None));
        assert_eq!(parse_dollars("-1"), Err(NotDollars));
        assert_eq!(format_cost(0.004), "<$0.01");
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(12.3), "$12.30");
        assert_eq!(shown(2.5), "2.5");
        assert_eq!(shown(3.0), "3");
    }
}
