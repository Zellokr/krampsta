//! Signing in to account providers (GitHub Copilot, a ChatGPT plan) from the UI: the dialog's
//! state and what its buttons do. The catalog does the work on the host's runtime; its code and
//! outcome land through [`KrampstaApp::pump`].

use krampsta_agent::{Account, AccountEvent, AgentError, DeviceCode, Provider};
use rune::Tx;

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

impl KrampstaApp {
    /// The providers that work through an account, for the settings' Accounts section.
    pub fn account_providers(&self) -> impl Iterator<Item = &Provider> {
        self.engine.providers.iter().filter(|p| p.account.is_some())
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
            }
        }
        changed
    }

    /// After signing in or out: reread the providers, forget the old model list and rebuild the
    /// engine if this provider is the selected one.
    fn accounts_changed(&mut self, provider: &str) {
        self.engine.providers = self.catalog.providers();
        self.engine.models.remove(provider);
        if self.engine.selection.provider == provider {
            self.rebuild_engine();
            self.ensure_models();
        }
    }
}

/// Whether `p` is signed in, and as whom.
pub fn signed_in_user(p: &Provider) -> Option<&str> {
    match &p.account {
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
        AgentEngine, BoxFuture, EngineCatalog, LoginPrompt, MockAgent, MockCatalog, MockConfig, Selection,
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
                    account: Some(if on { Account::SignedIn { user: "me".into() } } else { Account::SignedOut }),
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

    #[test]
    fn signing_in_shows_the_code_then_unlocks_the_selected_provider() {
        let mut app = KrampstaApp::new(Arc::new(Fake::default()));
        assert!(app.engine.problem.is_some(), "the account provider can't run signed out");
        assert_eq!(signed_in_user(acct(&app)), None);

        app.start_sign_in("acct");
        assert_eq!(app.sign_in, SignIn::Starting { provider: "acct".into() });
        pump_until(&mut app, |a| matches!(a.sign_in, SignIn::Code { .. }));
        let SignIn::Code { code, .. } = &app.sign_in else { unreachable!() };
        assert_eq!(code.code, "WXYZ-0000");

        pump_until(&mut app, |a| a.sign_in == SignIn::Closed);
        assert_eq!(signed_in_user(acct(&app)), Some("me"));
        assert_eq!(app.engine.problem, None, "the engine was rebuilt for the signed-in account");

        app.sign_out("acct");
        pump_until(&mut app, |a| signed_in_user(acct(a)).is_none());
        assert!(app.engine.problem.is_some());
    }

    #[test]
    fn choosing_a_signed_out_account_provider_opens_the_sign_in() {
        let mut app = KrampstaApp::new(Arc::new(Fake::default()));
        app.choose_provider(krampsta_agent::MOCK_PROVIDER);
        assert_eq!(app.sign_in, SignIn::Closed);
        app.choose_provider("acct");
        assert_eq!(app.sign_in, SignIn::Starting { provider: "acct".into() });
        assert_eq!(crate::picker::provider_text(acct(&app)), "Acct", "no mark: choosing it signs in");
        pump_until(&mut app, |a| a.sign_in == SignIn::Closed);
        assert_eq!(app.engine.selection.provider, "acct");
        assert_eq!(app.engine.problem, None);
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
}
