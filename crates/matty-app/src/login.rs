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

use crate::style::{self, TypeStyleExt as _, scaled};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    field::Field,
    input::{InputEvent, InputState},
    spinner::Spinner,
    theme::UiTheme,
};

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
    /// The browser is open at `provider`'s sign-in; Matty waits for the
    /// `mattermost://` hand-off (#75). An error here (a timeout or a
    /// failed hand-off) offers Retry.
    Browser {
        server_name: SharedString,
        methods: SignInMethods,
        provider: SsoProvider,
    },
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
}

impl Default for LoginModel {
    fn default() -> Self {
        Self {
            step: LoginStep::Server,
            busy: false,
            error: None,
        }
    }
}

/// What the card asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginEvent {
    /// Check `url` with `/api/v4/system/ping` and fetch its client config.
    CheckServer(String),
    /// Sign in with a login id and password (also LDAP).
    SignIn { login_id: String, password: String },
    /// Start (or restart) the browser flow for `provider`.
    Sso(SsoProvider),
    /// Stop waiting for the browser and go back to the methods.
    CancelSso,
    /// Sign in with a personal access token.
    Token(String),
    /// Back to the server step.
    ChangeServer,
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

pub struct LoginView {
    model: LoginModel,
    server: Entity<InputState>,
    login_id: Entity<InputState>,
    password: Entity<InputState>,
    token: Entity<InputState>,
    show_token: bool,
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
        let mut subscriptions = Vec::new();
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
        // First launch lands with the cursor in the field it asks for.
        let focus = cx.focus_handle();
        let first = match &model.step {
            LoginStep::Server => server.read(cx).focus_handle(cx),
            LoginStep::Methods { .. } => login_id.read(cx).focus_handle(cx),
            LoginStep::Browser { .. } => focus.clone(),
        };
        window.on_next_frame(move |window, cx| window.focus(&first, cx));
        Self {
            model,
            server,
            login_id,
            password,
            token,
            show_token: false,
            focus,
            _subscriptions: subscriptions,
        }
    }

    pub fn model(&self) -> &LoginModel {
        &self.model
    }

    /// Replaces the model; moving to the methods step focuses its first
    /// field, moving back focuses the URL.
    pub fn set_model(&mut self, model: LoginModel, window: &mut Window, cx: &mut Context<Self>) {
        let step_changed = model.step != self.model.step;
        self.model = model;
        if step_changed {
            self.show_token = false;
            let focus = match &self.model.step {
                LoginStep::Server => self.server.read(cx).focus_handle(cx),
                LoginStep::Methods { .. } => self.login_id.read(cx).focus_handle(cx),
                LoginStep::Browser { .. } => self.focus.clone(),
            };
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        cx.notify();
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
                        cx.emit(LoginEvent::CheckServer(url));
                    }
                    Err(message) => self.model.error = Some(message.into()),
                }
            }
            Submit::Password => {
                let login_id = self.login_id.read(cx).value().trim().to_string();
                let password = self.password.read(cx).value().to_string();
                if login_id.is_empty() || password.is_empty() {
                    self.model.error = Some("Enter your email or username and password.".into());
                } else {
                    self.model.error = None;
                    cx.emit(LoginEvent::SignIn { login_id, password });
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
                .child(div().flex_1().min_w_0().child(error))
        })
    }

    fn server_step(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(Self::heading(
                "Connect to your Mattermost server",
                "Enter the address you open in the browser. Matty assumes https.",
                &theme,
            ))
            .child(
                Field::new("login.server", &self.server)
                    .label("Server address")
                    .required(true),
            )
            .children(self.error_line(cx))
            .child(self.primary("login.continue", "Continue", Submit::Server, cx))
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
        let login_label = match (methods.password, methods.ldap) {
            (true, true) => "Email, username or AD/LDAP username",
            (false, true) => "AD/LDAP username",
            _ => "Email or username",
        };
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
    use super::{LoginStep, SignInMethods, SsoProvider, normalize_server_url};

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
