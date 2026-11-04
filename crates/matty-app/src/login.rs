//! The sign-in screen (PLAN §4.5, DESIGN.md §4): one centered card. First
//! the server URL with inline validation, then the sign-in methods the
//! server advertises. The view owns the fields; the window owns what
//! happens on submit, because reaching the server is sync's job.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _,
};

use crate::settings::{RecentServer, SettingsStore};
use crate::style::{self, TypeStyleExt as _, scaled};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    field::{Field, field_title},
    input::{InputEvent, InputState},
    otp_field::{OtpEvent, OtpField, OtpState},
    spinner::Spinner,
    theme::UiTheme,
    toggle_group::{ToggleGroup, ToggleGroupItem},
};

/// How many digits an authenticator code has.
pub const MFA_CODE_LEN: usize = 6;

/// A single sign-on provider the server has enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsoProvider {
    GitLab,
    Google,
    Office365,
    OpenId,
    Saml,
}

impl SsoProvider {
    pub fn label(self) -> &'static str {
        match self {
            SsoProvider::GitLab => "GitLab",
            SsoProvider::Google => "Google",
            SsoProvider::Office365 => "Office 365",
            SsoProvider::OpenId => "OpenID Connect",
            SsoProvider::Saml => "SAML",
        }
    }
}

/// What the server lets people sign in with, from `/api/v4/config/client`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignInMethods {
    /// Email or username and password.
    pub password: bool,
    /// LDAP/AD, which shares the password form and changes its wording.
    pub ldap: bool,
    pub sso: Vec<SsoProvider>,
}

impl SignInMethods {
    /// Whether the form offers a choice between the server's own accounts
    /// and AD/LDAP: only when both are on.
    pub fn ldap_choice(&self) -> bool {
        self.password && self.ldap
    }

    /// Whether a password sign-in goes to AD/LDAP only (`ldap_only`):
    /// always when LDAP is the only password method, otherwise when the
    /// person picked AD/LDAP.
    pub fn ldap_only(&self, picked_ldap: bool) -> bool {
        self.ldap && (!self.password || picked_ldap)
    }

    /// The login field's label for the method in use.
    pub fn login_label(&self, picked_ldap: bool) -> &'static str {
        if self.ldap_only(picked_ldap) {
            "AD/LDAP username"
        } else {
            "Email or username"
        }
    }
}

/// Which step the card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginStep {
    /// Asking for the server URL.
    Server,
    /// The server answered; asking how to sign in.
    Methods {
        server_name: SharedString,
        methods: SignInMethods,
    },
    /// The password was right and the account uses multi-factor
    /// authentication: asking for the code. Back returns to `Methods`.
    Mfa {
        server_name: SharedString,
        methods: SignInMethods,
    },
    /// The browser is open at `provider`'s sign-in; Matty waits for the
    /// `mattermost://` hand-off (#75). An error here (a timeout or a
    /// failed hand-off) offers Retry.
    Browser {
        server_name: SharedString,
        methods: SignInMethods,
        provider: SsoProvider,
    },
}

/// Why a sign-in did not go through, as the card needs to know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInFailure {
    /// The server wants an MFA code, or the code sent was wrong.
    MfaRequired,
    /// One sentence to show under the form.
    Message(SharedString),
}

impl LoginStep {
    /// The methods step this one came from, for Cancel.
    pub fn back_to_methods(&self) -> LoginStep {
        match self {
            LoginStep::Browser {
                server_name,
                methods,
                ..
            } => LoginStep::Methods {
                server_name: server_name.clone(),
                methods: methods.clone(),
            },
            other => other.clone(),
        }
    }

    /// Waiting for `provider` in the browser, from the methods step (or
    /// from waiting already, for Retry). The server step stays put.
    pub fn to_browser(&self, provider: SsoProvider) -> LoginStep {
        match self.back_to_methods() {
            LoginStep::Methods {
                server_name,
                methods,
            } => LoginStep::Browser {
                server_name,
                methods,
                provider,
            },
            other => other,
        }
    }
}

/// Everything the card draws besides what the person typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginModel {
    pub step: LoginStep,
    /// A request is in flight; the primary button shows a spinner.
    pub busy: bool,
    /// One specific sentence under the form, never a dialog.
    pub error: Option<SharedString>,
    /// Signed in already and adding another server (#206): the card
    /// offers Cancel, back to the server shown before.
    pub adding: bool,
}

impl LoginModel {
    /// Applies a failed sign-in: a first MFA answer moves to the code step,
    /// one on the code step means the code was wrong, and anything else is
    /// shown where the person is.
    pub fn sign_in_failed(&mut self, failure: SignInFailure) {
        self.busy = false;
        self.error = match (failure, &self.step) {
            (
                SignInFailure::MfaRequired,
                LoginStep::Methods {
                    server_name,
                    methods,
                },
            ) => {
                self.step = LoginStep::Mfa {
                    server_name: server_name.clone(),
                    methods: methods.clone(),
                };
                None
            }
            (SignInFailure::MfaRequired, LoginStep::Mfa { .. }) => Some(
                "That code is not right. Enter the current code from your authenticator app."
                    .into(),
            ),
            (SignInFailure::MfaRequired, LoginStep::Server | LoginStep::Browser { .. }) => {
                Some("This account needs a multi-factor code. Sign in again.".into())
            }
            (SignInFailure::Message(message), _) => Some(message),
        };
    }

    /// Leaves the code step for the password form, keeping what was typed.
    pub fn back_to_methods(&mut self) {
        if let LoginStep::Mfa {
            server_name,
            methods,
        } = &self.step
        {
            self.step = LoginStep::Methods {
                server_name: server_name.clone(),
                methods: methods.clone(),
            };
            self.error = None;
        }
    }
}

impl Default for LoginModel {
    fn default() -> Self {
        Self {
            step: LoginStep::Server,
            busy: false,
            error: None,
            adding: false,
        }
    }
}

/// A password sign-in, as `POST /users/login` takes it.
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordSignIn {
    pub login_id: String,
    pub password: String,
    /// Check the password against AD/LDAP only.
    pub ldap_only: bool,
    /// The authenticator code, once the server has asked for one.
    pub mfa_token: Option<String>,
}

impl std::fmt::Debug for PasswordSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordSignIn")
            .field("login_id", &self.login_id)
            .field("password", &"[redacted]")
            .field("ldap_only", &self.ldap_only)
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// What the card asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginEvent {
    /// Check `url` with `/api/v4/system/ping` and fetch its client config.
    CheckServer(String),
    /// Sign in with a login id and password (also LDAP and MFA).
    SignIn(PasswordSignIn),
    /// Start (or restart) the browser flow for `provider`.
    Sso(SsoProvider),
    /// Stop waiting for the browser and go back to the methods.
    CancelSso,
    /// Sign in with a personal access token.
    Token(String),
    /// Back to the server step.
    ChangeServer,
    /// Stop adding a server and go back to the one shown before (#206).
    Cancel,
}

/// Turns what someone typed into the server URL to check, assuming
/// `https://` and dropping a trailing slash. `Err` is the sentence to show.
pub fn normalize_server_url(input: &str) -> Result<String, &'static str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter your server's address, like chat.example.com.");
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let Some((scheme, rest)) = with_scheme.split_once("://") else {
        return Err("That does not look like a server address.");
    };
    if scheme != "https" && scheme != "http" {
        return Err("The address has to start with https://.");
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_only = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if host_only.is_empty()
        || host_only.contains(char::is_whitespace)
        || (!host_only.contains('.') && host_only != "localhost")
    {
        return Err("That does not look like a server address.");
    }
    Ok(with_scheme.trim_end_matches('/').to_string())
}

/// `chat.example.com` from `https://chat.example.com/sub`.
fn server_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

pub struct LoginView {
    model: LoginModel,
    server: Entity<InputState>,
    login_id: Entity<InputState>,
    password: Entity<InputState>,
    token: Entity<InputState>,
    code: Entity<OtpState>,
    show_token: bool,
    /// The person picked AD/LDAP where the server offers both.
    picked_ldap: bool,
    /// Servers whose check went through before, newest first.
    recent: Vec<RecentServer>,
    /// The URL last sent to be checked, remembered once it goes through.
    checking: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<LoginEvent> for LoginView {}

impl Focusable for LoginView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LoginView {
    pub fn new(model: LoginModel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let server = cx.new(|cx| InputState::new(window, cx).placeholder("chat.example.com"));
        let login_id = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let code = cx.new(|cx| OtpState::new(MFA_CODE_LEN, window, cx));
        let mut subscriptions = Vec::new();
        // The last digit submits, as the code step has nothing else to fill.
        subscriptions.push(cx.subscribe_in(
            &code,
            window,
            |this, _, event: &OtpEvent, window, cx| {
                if let OtpEvent::Complete = event {
                    this.submit(Submit::Code, window, cx);
                }
            },
        ));
        for (state, submit) in [
            (&server, Submit::Server),
            (&login_id, Submit::Password),
            (&password, Submit::Password),
            (&token, Submit::Token),
        ] {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit(submit, window, cx);
                    }
                },
            ));
        }
        // Typing clears a complaint about the address; leaving the field
        // says what is wrong with it before anything is sent.
        subscriptions.push(cx.subscribe_in(
            &server,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.server_edited(cx),
                InputEvent::Blur => this.server_left(cx),
                _ => {}
            },
        ));
        let recent = SettingsStore::try_global(cx)
            .map(|store| store.read(cx).get().recent_servers.clone())
            .unwrap_or_default();
        // First launch lands with the cursor in the field it asks for.
        let focus = cx.focus_handle();
        let first = match &model.step {
            LoginStep::Server => server.read(cx).focus_handle(cx),
            LoginStep::Methods { .. } => login_id.read(cx).focus_handle(cx),
            LoginStep::Mfa { .. } => code.read(cx).focus_handle(cx),
            LoginStep::Browser { .. } => focus.clone(),
        };
        window.on_next_frame(move |window, cx| window.focus(&first, cx));
        Self {
            model,
            server,
            login_id,
            password,
            token,
            code,
            show_token: false,
            picked_ldap: false,
            recent,
            checking: None,
            focus,
            _subscriptions: subscriptions,
        }
    }

    pub fn model(&self) -> &LoginModel {
        &self.model
    }

    /// Replaces the model; moving to the methods step focuses its first
    /// field, moving back focuses the URL. The code step starts empty, and
    /// a wrong code is cleared for the next one.
    pub fn set_model(&mut self, model: LoginModel, window: &mut Window, cx: &mut Context<Self>) {
        let step_changed = model.step != self.model.step;
        if let (LoginStep::Server, LoginStep::Methods { server_name, .. }) =
            (&self.model.step, &model.step)
            && let Some(url) = self.checking.take()
        {
            self.remember_server(&url, server_name, cx);
        }
        self.model = model;
        let on_code = matches!(self.model.step, LoginStep::Mfa { .. });
        if on_code && (step_changed || (self.model.error.is_some() && !self.model.busy)) {
            self.code
                .update(cx, |code, cx| code.set_value("", window, cx));
        }
        if step_changed {
            self.show_token = false;
            self.focus_step(window, cx);
        }
        cx.notify();
    }

    fn focus_step(&self, window: &mut Window, cx: &App) {
        let focus = self.step_focus(cx);
        window.on_next_frame(move |window, cx| window.focus(&focus, cx));
    }

    /// What the current step focuses when it opens.
    fn step_focus(&self, cx: &App) -> FocusHandle {
        match &self.model.step {
            LoginStep::Server => self.server.read(cx).focus_handle(cx),
            LoginStep::Methods { .. } => self.login_id.read(cx).focus_handle(cx),
            LoginStep::Mfa { .. } => self.code.read(cx).focus_handle(cx),
            LoginStep::Browser { .. } => self.focus.clone(),
        }
    }

    /// The password sign-in for what is typed, with `mfa_token` if given.
    fn password_sign_in(&self, mfa_token: Option<String>, cx: &App) -> PasswordSignIn {
        let ldap_only = match &self.model.step {
            LoginStep::Methods { methods, .. } | LoginStep::Mfa { methods, .. } => {
                methods.ldap_only(self.picked_ldap)
            }
            LoginStep::Server | LoginStep::Browser { .. } => false,
        };
        PasswordSignIn {
            login_id: self.login_id.read(cx).value().trim().to_string(),
            password: self.password.read(cx).value().to_string(),
            ldap_only,
            mfa_token,
        }
    }

    /// Puts `url` first among the recent servers, here and in settings.
    fn remember_server(&mut self, url: &str, name: &str, cx: &mut Context<Self>) {
        let Some(store) = SettingsStore::try_global(cx) else {
            return;
        };
        store.update(cx, |store, cx| {
            store.update(cx, |settings| settings.remember_server(url, name))
        });
        self.recent = store.read(cx).get().recent_servers.clone();
    }

    /// The server step's address changed: a complaint about the old one
    /// no longer applies.
    fn server_edited(&mut self, cx: &mut Context<Self>) {
        if self.model.step == LoginStep::Server && !self.model.busy && self.model.error.is_some() {
            self.model.error = None;
            cx.notify();
        }
    }

    /// Focus left the address: say now if it cannot be a server address.
    /// An empty field waits for Continue.
    fn server_left(&mut self, cx: &mut Context<Self>) {
        if self.model.step != LoginStep::Server || self.model.busy {
            return;
        }
        let typed = self.server.read(cx).value().to_string();
        if typed.trim().is_empty() {
            return;
        }
        if let Err(message) = normalize_server_url(&typed) {
            self.model.error = Some(message.into());
            cx.notify();
        }
    }

    /// Fills the address field with `address`, from a link to a server not
    /// saved yet (#206), for the person to check and continue.
    pub fn prefill_server(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.step == LoginStep::Server {
            self.server.update(cx, |field, cx| {
                field.set_value(address.to_owned(), window, cx)
            });
        }
    }

    /// The URL as typed, for the window to keep across steps.
    pub fn server_url(&self, cx: &App) -> String {
        self.server.read(cx).value().to_string()
    }

    fn submit(&mut self, submit: Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.busy {
            return;
        }
        match submit {
            Submit::Server => {
                let typed = self.server.read(cx).value().to_string();
                match normalize_server_url(&typed) {
                    Ok(url) => {
                        self.model.error = None;
                        self.checking = Some(url.clone());
                        cx.emit(LoginEvent::CheckServer(url));
                    }
                    Err(message) => self.model.error = Some(message.into()),
                }
            }
            Submit::Password => {
                let sign_in = self.password_sign_in(None, cx);
                if sign_in.login_id.is_empty() || sign_in.password.is_empty() {
                    self.model.error = Some("Enter your email or username and password.".into());
                } else {
                    self.model.error = None;
                    cx.emit(LoginEvent::SignIn(sign_in));
                }
            }
            Submit::Code => {
                let code = self.code.read(cx).value().to_string();
                if code.len() < MFA_CODE_LEN {
                    self.model.error = Some(
                        format!("Enter the {MFA_CODE_LEN}-digit code from your authenticator app.")
                            .into(),
                    );
                } else {
                    self.model.error = None;
                    cx.emit(LoginEvent::SignIn(self.password_sign_in(Some(code), cx)));
                }
            }
            Submit::Token => {
                let token = self.token.read(cx).value().trim().to_string();
                if token.is_empty() {
                    self.model.error = Some("Paste a personal access token.".into());
                } else {
                    self.model.error = None;
                    cx.emit(LoginEvent::Token(token));
                }
            }
        }
        let _ = window;
        cx.notify();
    }

    fn primary(
        &self,
        id: &'static str,
        label: &'static str,
        submit: Submit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let busy = self.model.busy;
        Button::new(id)
            .variant(ButtonVariant::Default)
            .size(ButtonSize::Lg)
            .w_full()
            .disabled(busy)
            .aria_label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.submit(submit, window, cx)))
            .when(busy, |button| {
                button.child(
                    Spinner::new(gpui_kit::ElementId::Name(format!("{id}.spinner").into()))
                        .color(theme.colors.primary_foreground),
                )
            })
            .child(label)
    }

    fn error_line(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let theme = UiTheme::read(cx).clone();
        self.model.error.clone().map(|error| {
            div()
                .flex()
                .items_start()
                .gap(theme.space(2.))
                .type_style(style::META, &theme)
                .text_color(theme.colors.destructive)
                .child(
                    lucide(LucideIcon::CircleAlert)
                        .size(scaled(&theme, 14.))
                        .text_color(theme.colors.destructive)
                        .flex_shrink_0()
                        .mt(theme.space(0.5)),
                )
                // Wraps inside the card instead of running past it.
                .child(div().flex_1().min_w_0().child(error))
        })
    }

    fn server_step(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(if self.model.adding {
                Self::heading(
                    "Add a Mattermost server",
                    "Enter the address you open in the browser. Matty assumes https.",
                    &theme,
                )
            } else {
                Self::heading(
                    "Connect to your Mattermost server",
                    "Enter the address you open in the browser. Matty assumes https.",
                    &theme,
                )
            })
            .child(
                Field::new("login.server", &self.server)
                    .label("Server address")
                    .required(true),
            )
            .children(self.error_line(cx))
            .child(self.primary("login.continue", "Continue", Submit::Server, cx))
            .when(!self.recent.is_empty(), |step| {
                step.child(self.recent_servers(cx))
            })
    }

    /// The servers checked before, after Continue in tab order: one press
    /// checks that server again.
    fn recent_servers(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let rows = self.recent.iter().enumerate().map(|(index, server)| {
            let host = server_host(&server.url).to_string();
            let url = server.url.clone();
            Button::new(gpui_kit::ElementId::NamedInteger(
                "login.recent".into(),
                index as u64,
            ))
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Lg)
            .w_full()
            .justify_start()
            .disabled(self.model.busy)
            .aria_label(format!("Connect to {}", server.name))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.server
                    .update(cx, |field, cx| field.set_value(url.clone(), window, cx));
                this.submit(Submit::Server, window, cx);
            }))
            .child(
                lucide(LucideIcon::Server)
                    .size(scaled(&theme, 14.))
                    .text_color(muted)
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.colors.foreground)
                    .child(server.name.clone()),
            )
            .when(host != server.name, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .type_style(style::META, &theme)
                        .text_color(muted)
                        .child(host),
                )
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .child(
                div()
                    .type_style(style::LABEL, &theme)
                    .text_color(muted)
                    .child("RECENT SERVERS"),
            )
            .children(rows)
    }

    fn methods_step(
        &self,
        server_name: &SharedString,
        methods: &SignInMethods,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let server_row = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.space(2.))
            .px(theme.space(3.))
            .h(theme.space(9.))
            .rounded(theme.radius.md)
            .bg(theme.colors.muted)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(2.))
                    .min_w_0()
                    .child(
                        lucide(LucideIcon::Server)
                            .size(scaled(&theme, 14.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .truncate()
                            .type_style(style::UI_STRONG, &theme)
                            .text_color(theme.colors.foreground)
                            .child(server_name.clone()),
                    ),
            )
            .child(
                Button::new("login.change-server")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Xs)
                    .aria_label("Change server")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.error = None;
                        cx.emit(LoginEvent::ChangeServer);
                    }))
                    .child("Change"),
            );
        let password_form = methods.password || methods.ldap;
        let login_label = methods.login_label(self.picked_ldap);
        let mut card = div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(Self::heading(
                "Sign in",
                "Use the account you have on this server.",
                &theme,
            ))
            .child(server_row);
        if password_form {
            card = card
                .when(methods.ldap_choice(), |card| {
                    card.child(self.method_choice(cx))
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.space(3.))
                        .child(Field::new("login.login-id", &self.login_id).label(login_label))
                        .child(Field::new("login.password", &self.password).label("Password")),
                )
                .children(self.error_line(cx))
                .child(self.primary("login.sign-in", "Sign in", Submit::Password, cx));
        } else {
            card = card.children(self.error_line(cx));
        }
        if !methods.sso.is_empty() {
            if password_form {
                card = card.child(Self::or_rule(&theme));
            }
            let buttons = methods.sso.iter().map(|provider| {
                let provider = *provider;
                Button::new(gpui_kit::ElementId::Name(
                    format!("login.sso.{}", provider.label()).into(),
                ))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Lg)
                .w_full()
                .disabled(self.model.busy)
                .aria_label(format!("Continue with {}", provider.label()))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(LoginEvent::Sso(provider))))
                .child(
                    lucide(LucideIcon::ExternalLink)
                        .size(scaled(&theme, 14.))
                        .text_color(muted),
                )
                .child(format!("Continue with {}", provider.label()))
            });
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(2.))
                    .children(buttons),
            );
        }
        let token_toggle = Button::new("login.token-toggle")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::Sm)
            .aria_label("Use a personal access token")
            .text_color(muted)
            .on_click(cx.listener(|this, _, window, cx| {
                this.show_token = !this.show_token;
                if this.show_token {
                    let focus = this.token.read(cx).focus_handle(cx);
                    window.on_next_frame(move |window, cx| window.focus(&focus, cx));
                }
                cx.notify();
            }))
            .child(if self.show_token {
                "Hide personal access token"
            } else {
                "Use a personal access token instead"
            });
        card = card.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(theme.space(3.))
                .pt(theme.space(1.))
                .child(token_toggle)
                .when(self.show_token, |col| {
                    col.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(theme.space(3.))
                            .child(
                                Field::new("login.token", &self.token)
                                    .label("Personal access token")
                                    .description(
                                        "From Profile › Security › Personal access tokens.",
                                    ),
                            )
                            .child(self.primary(
                                "login.token-sign-in",
                                "Sign in with token",
                                Submit::Token,
                                cx,
                            )),
                    )
                }),
        );
        card
    }

    /// Email or username against AD/LDAP, where the server offers both.
    fn method_choice(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        let item = |id: &'static str, value: &'static str, text: &'static str| {
            ToggleGroupItem::new(id, value)
                .aria_label(text)
                .flex_1()
                .child(text)
        };
        ToggleGroup::new("login.method")
            .aria_label("Sign in with")
            .joined(true)
            .w_full()
            .disabled(self.model.busy)
            .value([if self.picked_ldap { "ldap" } else { "password" }])
            .item(item(
                "login.method.password",
                "password",
                "Email or username",
            ))
            .item(item("login.method.ldap", "ldap", "AD/LDAP"))
            .on_change(move |next, _, cx| {
                let ldap = next.first().is_some_and(|value| value == "ldap");
                view.update(cx, |this, cx| {
                    this.picked_ldap = ldap;
                    cx.notify();
                })
                .ok();
            })
    }

    /// The code step: six digit cells that submit when full, the error,
    /// Verify, and Back to the password form.
    fn mfa_step(
        &self,
        server_name: &SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(Self::heading(
                "Enter your code",
                "This account uses multi-factor authentication. Open your authenticator app and enter the code it shows.",
                &theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(2.))
                    .child(field_title(cx).child(format!("{MFA_CODE_LEN}-digit code for {server_name}")))
                    .child(
                        OtpField::new(&self.code)
                            .aria_label("Authentication code")
                            .disabled(self.model.busy),
                    ),
            )
            .children(self.error_line(cx))
            .child(self.primary("login.verify", "Verify", Submit::Code, cx))
            .child(
                div().flex().justify_center().child(
                    Button::new("login.mfa-back")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .aria_label("Back to sign in")
                        .text_color(theme.colors.muted_foreground)
                        .disabled(self.model.busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.model.back_to_methods();
                            this.focus_step(window, cx);
                            cx.notify();
                        }))
                        .child("Back"),
                ),
            )
    }

    /// Waiting for the browser: what is happening, a spinner while it
    /// does, and Cancel; after a timeout or a failed hand-off, the error
    /// and Retry.
    fn browser_step(
        &self,
        server_name: &SharedString,
        provider: SsoProvider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let failed = self.model.error.is_some();
        let status = if self.model.busy {
            "Signing in…".to_string()
        } else {
            format!("Waiting for {}…", provider.label())
        };
        let status_row = div()
            .flex()
            .items_center()
            .gap(theme.space(2.))
            .px(theme.space(3.))
            .h(theme.space(9.))
            .rounded(theme.radius.md)
            .bg(theme.colors.muted)
            .child(if failed {
                lucide(LucideIcon::Server)
                    .size(scaled(&theme, 14.))
                    .text_color(muted)
                    .into_any_element()
            } else {
                Spinner::new("login.browser.spinner")
                    .color(muted)
                    .into_any_element()
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .type_style(style::UI_STRONG, &theme)
                    .text_color(theme.colors.foreground)
                    .child(server_name.clone()),
            )
            .when(!failed, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .type_style(style::META, &theme)
                        .text_color(muted)
                        .child(status),
                )
            });
        let cancel = Button::new("login.browser.cancel")
            .variant(if failed {
                ButtonVariant::Ghost
            } else {
                ButtonVariant::Outline
            })
            .size(ButtonSize::Lg)
            .w_full()
            .disabled(self.model.busy)
            .aria_label("Cancel browser sign-in")
            .on_click(cx.listener(|this, _, _, cx| {
                this.model.error = None;
                cx.emit(LoginEvent::CancelSso);
            }))
            .child("Cancel");
        let retry = Button::new("login.browser.retry")
            .variant(ButtonVariant::Default)
            .size(ButtonSize::Lg)
            .w_full()
            .aria_label(format!("Try {} sign-in again", provider.label()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.error = None;
                cx.emit(LoginEvent::Sso(provider));
            }))
            .child(
                lucide(LucideIcon::ExternalLink)
                    .size(scaled(&theme, 14.))
                    .text_color(theme.colors.primary_foreground),
            )
            .child("Retry");
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(if failed {
                Self::heading(
                    "Browser sign-in did not finish",
                    "Try again to open a new sign-in page, or cancel to pick another way in.",
                    &theme,
                )
            } else {
                Self::heading(
                    "Waiting for your browser…",
                    "Finish signing in in the browser window Matty opened. Matty continues on its own when you are done.",
                    &theme,
                )
            })
            .child(status_row)
            .children(self.error_line(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(2.))
                    .when(failed, |col| col.child(retry))
                    .child(cancel),
            )
    }

    fn heading(
        title: &'static str,
        text: &'static str,
        theme: &UiTheme,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap(theme.space(1.))
            .child(
                div()
                    .text_size(theme.text(20.))
                    .line_height(theme.text(26.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme.colors.foreground)
                    .child(title),
            )
            .child(
                div()
                    .type_style(style::UI, theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(text),
            )
    }

    fn or_rule(theme: &UiTheme) -> impl IntoElement + use<> {
        let rule = || div().flex_1().h(style::hairline()).bg(theme.colors.border);
        div()
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .child(rule())
            .child(
                div()
                    .type_style(style::LABEL, theme)
                    .text_color(theme.colors.muted_foreground)
                    .child("OR"),
            )
            .child(rule())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Submit {
    Server,
    Password,
    Code,
    Token,
}

impl Render for LoginView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let body = match self.model.step.clone() {
            LoginStep::Server => self.server_step(cx).into_any_element(),
            LoginStep::Methods {
                server_name,
                methods,
            } => self
                .methods_step(&server_name, &methods, cx)
                .into_any_element(),
            LoginStep::Mfa { server_name, .. } => {
                self.mfa_step(&server_name, cx).into_any_element()
            }
            LoginStep::Browser {
                server_name,
                provider,
                ..
            } => self
                .browser_step(&server_name, provider, cx)
                .into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme.space(6.))
            .p(theme.space(6.))
            .bg(theme.colors.sidebar)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(2.))
                    .child(
                        div()
                            .size(theme.space(8.))
                            .rounded(theme.radius.lg)
                            .bg(theme.colors.primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.colors.primary_foreground)
                            .type_style(style::UI_STRONG, &theme)
                            .child("M"),
                    )
                    .child(
                        div()
                            .text_size(theme.text(18.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(theme.colors.foreground)
                            .child("Matty"),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(scaled(&theme, 400.))
                    .p(theme.space(6.))
                    .rounded(theme.radius.xl)
                    .bg(theme.colors.card)
                    .border(style::hairline())
                    .border_color(theme.colors.border)
                    .shadow_md()
                    .child(body),
            )
            .when(self.model.adding, |page| {
                page.child(
                    Button::new("login.cancel")
                        .variant(ButtonVariant::Ghost)
                        .label("Cancel")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(LoginEvent::Cancel))),
                )
            })
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child("Matty is a native Mattermost client. Your password goes only to your server."),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn methods(password: bool, ldap: bool) -> SignInMethods {
        SignInMethods {
            password,
            ldap,
            sso: Vec::new(),
        }
    }

    fn at_methods() -> LoginModel {
        LoginModel {
            step: LoginStep::Methods {
                server_name: "chat.example.com".into(),
                methods: methods(true, false),
            },
            busy: true,
            error: None,
            adding: false,
        }
    }

    #[test]
    fn the_ldap_choice_shows_only_when_the_server_offers_both() {
        assert!(methods(true, true).ldap_choice());
        assert!(!methods(true, false).ldap_choice());
        assert!(!methods(false, true).ldap_choice());
    }

    #[test]
    fn ldap_only_is_sent_for_an_ldap_only_server_or_when_picked() {
        assert!(methods(false, true).ldap_only(false));
        assert!(methods(true, true).ldap_only(true));
        assert!(!methods(true, true).ldap_only(false));
        // Without LDAP on the server, a stale pick never sends it.
        assert!(!methods(true, false).ldap_only(true));
        assert_eq!(methods(false, true).login_label(false), "AD/LDAP username");
        assert_eq!(methods(true, true).login_label(false), "Email or username");
        assert_eq!(methods(true, true).login_label(true), "AD/LDAP username");
    }

    #[test]
    fn an_mfa_answer_moves_to_the_code_step_and_back_keeps_the_server() {
        let mut model = at_methods();
        model.sign_in_failed(SignInFailure::MfaRequired);
        assert_eq!(
            model.step,
            LoginStep::Mfa {
                server_name: "chat.example.com".into(),
                methods: methods(true, false),
            }
        );
        assert!(!model.busy);
        assert_eq!(model.error, None);

        model.back_to_methods();
        assert_eq!(
            model,
            LoginModel {
                busy: false,
                ..at_methods()
            }
        );
    }

    #[test]
    fn a_wrong_code_stays_on_the_code_step_with_an_error() {
        let mut model = at_methods();
        model.sign_in_failed(SignInFailure::MfaRequired);
        model.busy = true;
        model.sign_in_failed(SignInFailure::MfaRequired);
        assert!(matches!(model.step, LoginStep::Mfa { .. }));
        assert!(!model.busy);
        assert!(
            model
                .error
                .as_ref()
                .is_some_and(|e| e.contains("code is not right"))
        );

        model.sign_in_failed(SignInFailure::Message("Locked.".into()));
        assert!(matches!(model.step, LoginStep::Mfa { .. }));
        assert_eq!(model.error, Some("Locked.".into()));
    }

    #[test]
    fn other_failures_stay_on_the_password_form() {
        let mut model = at_methods();
        model.sign_in_failed(SignInFailure::Message("Wrong password.".into()));
        assert!(matches!(model.step, LoginStep::Methods { .. }));
        assert_eq!(model.error, Some("Wrong password.".into()));
    }

    #[test]
    fn a_sign_in_never_prints_its_password_or_code() {
        let sign_in = PasswordSignIn {
            login_id: "arya".into(),
            password: "hunter2".into(),
            ldap_only: false,
            mfa_token: Some("123456".into()),
        };
        let printed = format!("{:?}", LoginEvent::SignIn(sign_in));
        assert!(printed.contains("arya"));
        assert!(!printed.contains("hunter2"));
        assert!(!printed.contains("123456"));
    }

    #[test]
    fn browser_step_goes_back_to_the_same_methods() {
        let methods = LoginStep::Methods {
            server_name: "chat.example.com".into(),
            methods: SignInMethods {
                password: true,
                ldap: false,
                sso: vec![SsoProvider::GitLab, SsoProvider::Saml],
            },
        };
        let waiting = methods.to_browser(SsoProvider::Saml);
        assert!(matches!(
            waiting,
            LoginStep::Browser {
                provider: SsoProvider::Saml,
                ..
            }
        ));
        assert_eq!(waiting.back_to_methods(), methods);
        // Retry keeps waiting on the provider asked for.
        assert!(matches!(
            waiting.to_browser(SsoProvider::GitLab),
            LoginStep::Browser {
                provider: SsoProvider::GitLab,
                ..
            }
        ));
        assert_eq!(
            LoginStep::Server.to_browser(SsoProvider::GitLab),
            LoginStep::Server
        );
    }

    /// The sign-in card alone in a window, with the events it sends.
    struct Card {
        view: Entity<LoginView>,
        cx: gpui_kit::VisualTestContext,
        events: std::rc::Rc<std::cell::RefCell<Vec<LoginEvent>>>,
    }

    impl Card {
        fn open(cx: &mut gpui_kit::TestAppContext, model: LoginModel) -> Self {
            cx.update(|cx| {
                crate::ui::theme::init(cx);
                crate::settings::SettingsStore::init(None, cx);
            });
            let window = cx.add_window(|window, cx| {
                window.activate_window();
                LoginView::new(model, window, cx)
            });
            let view = window.root(cx).unwrap();
            let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let sent = events.clone();
            cx.update(|cx| {
                cx.subscribe(&view, move |_, event: &LoginEvent, _| {
                    sent.borrow_mut().push(event.clone())
                })
                .detach()
            });
            let cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
            let mut card = Self { view, cx, events };
            card.focus_step();
            card
        }

        /// What the view's next frame does: focus the step's first control.
        fn focus_step(&mut self) {
            let view = self.view.clone();
            self.cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                view.read(cx).step_focus(cx).focus(window, cx);
                window.draw(cx).clear(cx);
            });
        }

        fn press(&mut self, keys: &'static str) {
            for key in keys.split(' ') {
                let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
                self.cx.simulate_event(gpui_kit::KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                });
                self.cx.simulate_event(gpui_kit::KeyUpEvent { keystroke });
                self.redraw();
            }
        }

        fn type_text(&mut self, text: &str) {
            self.cx.simulate_input(text);
            self.redraw();
        }

        fn redraw(&mut self) {
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn sent(&self) -> Vec<LoginEvent> {
            self.events.borrow().clone()
        }

        fn error(&mut self) -> Option<SharedString> {
            let view = self.view.clone();
            self.cx.update(|_, cx| view.read(cx).model().error.clone())
        }

        fn typed(&mut self) -> String {
            let view = self.view.clone();
            self.cx.update(|_, cx| view.read(cx).server_url(cx))
        }

        fn set_model(&mut self, model: LoginModel) {
            let view = self.view.clone();
            self.cx
                .update(|window, cx| view.update(cx, |view, cx| view.set_model(model, window, cx)));
            self.focus_step();
        }
    }

    #[gpui_kit::test]
    fn enter_in_the_address_checks_it_with_https(cx: &mut gpui_kit::TestAppContext) {
        let mut card = Card::open(cx, LoginModel::default());
        card.type_text("chat.example.com");
        card.press("enter");
        assert_eq!(
            card.sent(),
            [LoginEvent::CheckServer("https://chat.example.com".into())]
        );
    }

    #[gpui_kit::test]
    fn tab_goes_from_the_address_to_continue(cx: &mut gpui_kit::TestAppContext) {
        let mut card = Card::open(cx, LoginModel::default());
        card.type_text("chat.example.com");
        // Enter on the focused Continue button submits too.
        card.press("tab enter");
        assert_eq!(
            card.sent(),
            [LoginEvent::CheckServer("https://chat.example.com".into())]
        );
    }

    #[gpui_kit::test]
    fn leaving_a_bad_address_says_why_and_typing_clears_it(cx: &mut gpui_kit::TestAppContext) {
        let mut card = Card::open(cx, LoginModel::default());
        // Leaving the empty field says nothing yet.
        card.press("tab");
        assert_eq!(card.error(), None);
        card.press("shift-tab");
        card.type_text("justaword");
        card.press("tab");
        assert_eq!(
            card.error(),
            Some("That does not look like a server address.".into())
        );
        assert!(card.sent().is_empty());
        card.press("shift-tab");
        card.type_text(".com");
        assert_eq!(card.error(), None);
    }

    #[gpui_kit::test]
    fn the_address_stays_editable_while_a_check_runs(cx: &mut gpui_kit::TestAppContext) {
        let mut card = Card::open(cx, LoginModel::default());
        card.type_text("chat.example.com");
        card.press("enter");
        card.set_model(LoginModel {
            busy: true,
            ..LoginModel::default()
        });
        card.type_text("/x");
        assert_eq!(card.typed(), "chat.example.com/x");
        // A second Enter waits for the first check.
        card.press("enter");
        assert_eq!(card.sent().len(), 1);
    }

    #[gpui_kit::test]
    fn a_server_that_answers_is_remembered_and_offered_again(cx: &mut gpui_kit::TestAppContext) {
        let mut card = Card::open(cx, LoginModel::default());
        card.type_text("chat.example.com");
        card.press("enter");
        card.set_model(LoginModel {
            step: LoginStep::Methods {
                server_name: "Example Chat".into(),
                methods: methods(true, false),
            },
            ..LoginModel::default()
        });
        let recent = card.cx.update(|_, cx| {
            crate::settings::SettingsStore::global(cx)
                .read(cx)
                .get()
                .recent_servers
                .clone()
        });
        assert_eq!(
            recent,
            [RecentServer {
                url: "https://chat.example.com".into(),
                name: "Example Chat".into(),
            }]
        );

        // Back on the server step, Tab passes Continue to the recent
        // server, and Enter checks it again.
        card.set_model(LoginModel::default());
        card.press("tab tab enter");
        assert_eq!(
            card.sent().last(),
            Some(&LoginEvent::CheckServer("https://chat.example.com".into()))
        );
    }

    #[test]
    fn a_host_drops_the_scheme_and_path() {
        assert_eq!(
            server_host("https://chat.example.com/sub"),
            "chat.example.com"
        );
        assert_eq!(server_host("http://localhost:8065"), "localhost:8065");
    }

    #[test]
    fn assumes_https_and_trims() {
        assert_eq!(
            normalize_server_url("  chat.example.com/ "),
            Ok("https://chat.example.com".to_string())
        );
        assert_eq!(
            normalize_server_url("http://localhost:8065"),
            Ok("http://localhost:8065".to_string())
        );
        assert_eq!(
            normalize_server_url("https://mm.example.com/subpath/"),
            Ok("https://mm.example.com/subpath".to_string())
        );
    }

    #[test]
    fn rejects_what_is_not_an_address() {
        assert!(normalize_server_url("").is_err());
        assert!(normalize_server_url("ftp://files.example.com").is_err());
        assert!(normalize_server_url("not an address").is_err());
        assert!(normalize_server_url("justaword").is_err());
    }
}
