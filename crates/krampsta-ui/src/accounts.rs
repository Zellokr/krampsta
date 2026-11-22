//! Signing in to account providers (GitHub Copilot, a ChatGPT plan) and saving API keys from the
//! UI: the dialog's and the key field's state, and what their buttons do. The catalog does the
//! work on the host's runtime; codes and outcomes land through [`KrampstaApp::pump`].

use krampsta_agent::{Account, AccountEvent, AgentError, DeviceCode, Provider};
use rune::{TextEdit, Tx};

use crate::state::KrampstaApp;

/// The sign-in dialog. Open in every state but `Closed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SignIn {
    #[default]
    Closed,
    /// Asking the provider for a code.
    Starting {
        provider: String,
    },
    /// Waiting for the user to enter `code` at its URL.
    Code {
        provider: String,
        code: DeviceCode,
    },
    Failed {
        provider: String,
        error: String,
    },
}

impl SignIn {
    pub fn provider(&self) -> Option<&str> {
        match self {
            SignIn::Closed => None,
            SignIn::Starting { provider } | SignIn::Code { provider, .. } | SignIn::Failed { provider, .. } => {
                Some(provider)
            }
        }
    }
}

/// The API key field in settings: what is typed (masked) and how the last save went.
#[derive(Clone, Debug)]
pub struct KeyField {
    pub draft: TextEdit,
    pub state: KeyState,
}

impl Default for KeyField {
    fn default() -> Self {
        KeyField { draft: TextEdit::new("").secure(), state: KeyState::Idle }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeyState {
    #[default]
    Idle,
    /// Saving or removing `provider`'s key in the keychain.
    Saving {
        provider: String,
    },
    Failed {
        provider: String,
        error: String,
    },
}

impl KrampstaApp {
    /// The providers whose API key can be saved here, for the settings' API keys section.
    pub fn key_providers(&self) -> impl Iterator<Item = &Provider> {
        self.engine.providers.iter().filter(|p| p.api_key.is_some())
    }

    /// Saves the typed key for `provider` in the system keychain.
    pub fn save_api_key(&mut self, provider: &str) {
        let key = self.api_key.draft.text.trim().to_owned();
        if key.is_empty() {
            return;
        }
        self.change_api_key(provider, Some(key));
    }

    /// Forgets `provider`'s saved key (the environment's, if any, takes over).
    pub fn remove_api_key(&mut self, provider: &str) {
        self.change_api_key(provider, None);
    }

    fn change_api_key(&mut self, provider: &str, key: Option<String>) {
        let work = self.catalog.set_api_key(provider, key);
        self.api_key.state = match self.agent.save_api_key(provider, work) {
            Ok(()) => KeyState::Saving { provider: provider.to_owned() },
            Err(e) => KeyState::Failed { provider: provider.to_owned(), error: reason(&e) },
        };
    }

    /// The providers that work through an account, for the settings' Accounts section.
    pub fn account_providers(&self) -> impl Iterator<Item = &Provider> {
        self.engine.providers.iter().filter(|p| p.account().is_some())
    }

    pub fn provider_label<'a>(&'a self, id: &'a str) -> &'a str {
        self.engine.providers.iter().find(|p| p.id == id).map_or(id, |p| p.label.as_str())
    }

    /// Starts signing in to `provider`; the dialog opens and shows the code once it arrives.
    pub fn start_sign_in(&mut self, provider: &str) {
        let catalog = self.catalog.clone();
        let id = provider.to_owned();
        self.sign_in = match self.agent.sign_in(provider, move |prompt| catalog.login(&id, prompt)) {
            Ok(()) => SignIn::Starting { provider: provider.to_owned() },
            Err(e) => SignIn::Failed { provider: provider.to_owned(), error: e.to_string() },
        };
    }

    /// Closes the dialog, giving up on a sign-in still waiting.
    pub fn close_sign_in(&mut self) {
        self.agent.cancel_sign_in();
        self.sign_in = SignIn::Closed;
    }

    /// Copies the code and opens the page where it goes, in the default browser.
    pub fn open_sign_in_page(&mut self, tx: &mut Tx) {
        if let SignIn::Code { code, .. } = &self.sign_in {
            tx.copy(code.code.clone());
            if let Err(e) = tx.open_url(&code.url) {
                self.notice = Some(format!("Could not open the browser ({e}); go to {} yourself.", code.url));
            }
        }
    }

    pub fn sign_out(&mut self, provider: &str) {
        let logout = self.catalog.logout(provider);
        if let Err(e) = self.agent.sign_out(provider, logout) {
            self.notice = Some(e.to_string());
        }
    }

    /// Applies sign-in codes and outcomes. True when anything visible changed.
    pub(crate) fn land_accounts(&mut self, events: Vec<AccountEvent>) -> bool {
        let changed = !events.is_empty();
        for event in events {
            match event {
                AccountEvent::Code { provider, code } => {
                    if self.sign_in.provider() == Some(provider.as_str()) {
                        self.sign_in = SignIn::Code { provider, code };
                    }
                }
                AccountEvent::SignedIn { provider, result } => match result {
                    Ok(_) => {
                        self.sign_in = SignIn::Closed;
                        self.accounts_changed(&provider);
                    }
                    // The user closed the dialog; nothing to report.
                    Err(AgentError::Cancelled) => {}
                    Err(e) => {
                        if self.sign_in.provider() == Some(provider.as_str()) {
                            self.sign_in = SignIn::Failed { provider, error: reason(&e) };
                        }
                    }
                },
                AccountEvent::SignedOut { provider, result } => match result {
                    Ok(()) => self.accounts_changed(&provider),
                    Err(e) => self.notice = Some(reason(&e)),
                },
                AccountEvent::KeySaved { provider, result } => match result {
                    Ok(()) => {
                        // A fresh secure field: the old one's buffer is wiped as it drops.
                        self.api_key = KeyField::default();
                        self.accounts_changed(&provider);
                    }
                    Err(e) => self.api_key.state = KeyState::Failed { provider, error: reason(&e) },
                },
            }
        }
        changed
    }

    /// After signing in or out: reread the providers, forget the old model list and rebuild the
    /// engine if this provider is the selected one. Signing out of the selected provider moves
    /// the selection to one that can still run.
    fn accounts_changed(&mut self, provider: &str) {
        self.engine.providers = self.catalog.providers();
        self.engine.models.remove(provider);
        if self.keep_selection_usable() || self.engine.selection.provider == provider {
            self.rebuild_engine();
            self.ensure_models();
        }
    }
}

/// Whether `p` is signed in, and as whom.
pub fn signed_in_user(p: &Provider) -> Option<&str> {
    match p.account() {
        Some(Account::SignedIn { user }) => Some(user),
        _ => None,
    }
}

fn reason(e: &AgentError) -> String {
    match e {
        AgentError::Provider(message) => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use krampsta_agent::{
        AgentEngine, BoxFuture, EngineCatalog, KeySource, LoginPrompt, MOCK_PROVIDER, MockAgent, MockCatalog,
        MockConfig, ProviderKind, Selection,
    };

    use super::*;

    /// One account provider, "acct", whose sign-in shows a code and succeeds (or fails when
    /// told to), and the simulated agent.
    #[derive(Default)]
    struct Fake {
        signed_in: Arc<AtomicBool>,
        fail: bool,
        logins: Mutex<usize>,
    }

    impl EngineCatalog for Fake {
        fn providers(&self) -> Vec<Provider> {
            let on = self.signed_in.load(Ordering::SeqCst);
            vec![
                Provider {
                    id: "acct".into(),
                    label: "Acct".into(),
                    default_model: Some("m".into()),
                    missing: (!on).then(|| "needs signing in".into()),
                    kind: ProviderKind::Account(if on {
                        Account::SignedIn { user: "me".into() }
                    } else {
                        Account::SignedOut
                    }),
                    reasoning_effort: false,
                    images: false,
                    api_key: None,
                },
                MockCatalog::provider(),
            ]
        }
        fn initial(&self) -> Selection {
            Selection { provider: "acct".into(), model: Some("m".into()) }
        }
        fn engine(&self, provider: &str, _: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
            if provider == "acct" && !self.signed_in.load(Ordering::SeqCst) {
                return Err(AgentError::Provider("Acct needs signing in".into()));
            }
            Ok(Arc::new(MockAgent::new(MockConfig::instant())))
        }
        fn models(&self, _: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
            Box::pin(async { Ok(vec!["m".to_owned()]) })
        }
        fn login(&self, _: &str, prompt: LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>> {
            *self.logins.lock().unwrap() += 1;
            let (flag, fail) = (self.signed_in.clone(), self.fail);
            Box::pin(async move {
                prompt(DeviceCode { url: "https://example.com/device".into(), code: "WXYZ-0000".into() });
                tokio::time::sleep(Duration::from_millis(30)).await;
                if fail {
                    return Err(AgentError::Provider("the code expired".into()));
                }
                flag.store(true, Ordering::SeqCst);
                Ok("me".into())
            })
        }
        fn logout(&self, _: &str) -> BoxFuture<'static, Result<(), AgentError>> {
            let flag = self.signed_in.clone();
            Box::pin(async move {
                flag.store(false, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    fn pump_until(app: &mut KrampstaApp, done: impl Fn(&KrampstaApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(app) {
            assert!(Instant::now() < deadline, "timed out; sign-in is {:?}", app.sign_in);
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn acct(app: &KrampstaApp) -> &Provider {
        app.account_providers().next().unwrap()
    }

    fn listed(app: &KrampstaApp) -> Vec<&str> {
        app.engine.usable().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn signing_in_lists_the_account_and_signing_out_moves_off_it() {
        let mut app = KrampstaApp::new(Arc::new(Fake::default()));
        assert_eq!(listed(&app), [MOCK_PROVIDER], "signed out: not in the pickers");
        assert_eq!(app.engine.selection.provider, MOCK_PROVIDER, "it starts on what can run");
        app.choose_provider("acct");
        assert_eq!(app.engine.selection.provider, MOCK_PROVIDER);
        assert_eq!(app.sign_in, SignIn::Closed, "choosing doesn't sign in: the settings card does");

        app.start_sign_in("acct");
        assert_eq!(app.sign_in, SignIn::Starting { provider: "acct".into() });
        pump_until(&mut app, |a| matches!(a.sign_in, SignIn::Code { .. }));
        let SignIn::Code { code, .. } = &app.sign_in else { unreachable!() };
        assert_eq!(code.code, "WXYZ-0000");

        pump_until(&mut app, |a| a.sign_in == SignIn::Closed);
        assert_eq!(signed_in_user(acct(&app)), Some("me"));
        assert_eq!(listed(&app), ["acct", MOCK_PROVIDER]);
        app.choose_provider("acct");
        assert_eq!(app.engine.selection.provider, "acct");
        assert_eq!(app.engine.problem, None);

        app.sign_out("acct");
        pump_until(&mut app, |a| signed_in_user(acct(a)).is_none());
        assert_eq!(app.engine.selection.provider, MOCK_PROVIDER);
        assert_eq!(app.engine.problem, None, "the simulated agent can run");
    }

    #[test]
    fn a_failed_sign_in_says_why_and_closing_cancels() {
        let mut app = KrampstaApp::new(Arc::new(Fake { fail: true, ..Fake::default() }));
        app.start_sign_in("acct");
        pump_until(&mut app, |a| matches!(a.sign_in, SignIn::Failed { .. }));
        assert_eq!(app.sign_in, SignIn::Failed { provider: "acct".into(), error: "the code expired".into() });
        app.close_sign_in();
        assert_eq!(app.sign_in, SignIn::Closed);

        // Closing while waiting: the late "cancelled" outcome leaves the dialog closed.
        app.start_sign_in("acct");
        pump_until(&mut app, |a| matches!(a.sign_in, SignIn::Code { .. }));
        app.close_sign_in();
        std::thread::sleep(Duration::from_millis(60));
        app.pump();
        assert_eq!(app.sign_in, SignIn::Closed);
    }

    /// One provider, "keyed", that needs an API key saved here (refused when it is "bad"),
    /// and the simulated agent.
    #[derive(Default)]
    struct Keyed {
        key: Arc<Mutex<Option<String>>>,
    }

    impl EngineCatalog for Keyed {
        fn providers(&self) -> Vec<Provider> {
            let saved = self.key.lock().unwrap().is_some();
            vec![
                Provider {
                    id: "keyed".into(),
                    label: "Keyed".into(),
                    default_model: Some("m".into()),
                    missing: (!saved).then(|| "needs an API key".into()),
                    kind: ProviderKind::Cloud,
                    reasoning_effort: false,
                    images: false,
                    api_key: Some(if saved { KeySource::Saved } else { KeySource::Missing }),
                },
                MockCatalog::provider(),
            ]
        }
        fn initial(&self) -> Selection {
            MockCatalog::default().initial()
        }
        fn engine(&self, _: &str, _: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
            Ok(Arc::new(MockAgent::new(MockConfig::instant())))
        }
        fn models(&self, _: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
            Box::pin(async { Ok(vec!["m".to_owned()]) })
        }
        fn set_api_key(&self, _: &str, key: Option<String>) -> BoxFuture<'static, Result<(), AgentError>> {
            let slot = self.key.clone();
            Box::pin(async move {
                if key.as_deref() == Some("bad") {
                    return Err(AgentError::Provider("the keychain is locked".into()));
                }
                *slot.lock().unwrap() = key;
                Ok(())
            })
        }
    }

    fn keyed(app: &KrampstaApp) -> Option<KeySource> {
        app.key_providers().next().and_then(|p| p.api_key.clone())
    }

    #[test]
    fn a_saved_key_lists_the_provider_and_removing_it_takes_it_away() {
        let catalog = Arc::new(Keyed::default());
        let mut app = KrampstaApp::new(catalog.clone());
        assert_eq!(keyed(&app), Some(KeySource::Missing));
        assert_eq!(listed(&app), [MOCK_PROVIDER]);

        app.save_api_key("keyed");
        assert_eq!(app.api_key.state, KeyState::Idle, "nothing typed, nothing saved");

        app.api_key.draft = TextEdit::new("  sk-typed  ").secure();
        app.save_api_key("keyed");
        assert_eq!(app.api_key.state, KeyState::Saving { provider: "keyed".into() });
        pump_until(&mut app, |a| keyed(a) == Some(KeySource::Saved));
        assert_eq!(catalog.key.lock().unwrap().as_deref(), Some("sk-typed"), "trimmed");
        assert_eq!(app.api_key.state, KeyState::Idle);
        assert!(app.api_key.draft.text.is_empty() && app.api_key.draft.is_secure(), "a fresh masked field");
        assert_eq!(listed(&app), ["keyed", MOCK_PROVIDER]);

        app.remove_api_key("keyed");
        pump_until(&mut app, |a| keyed(a) == Some(KeySource::Missing));
        assert_eq!(listed(&app), [MOCK_PROVIDER]);

        app.api_key.draft = TextEdit::new("bad").secure();
        app.save_api_key("keyed");
        pump_until(&mut app, |a| matches!(a.api_key.state, KeyState::Failed { .. }));
        let error = "the keychain is locked".to_owned();
        assert_eq!(app.api_key.state, KeyState::Failed { provider: "keyed".into(), error });
        assert_eq!(app.api_key.draft.text, "bad", "what was typed stays to try again");
    }
}
