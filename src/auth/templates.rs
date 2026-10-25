//! # Template structs — 2, one per auth template
//!
//! Same rule as the todo templates: every field becomes a variable in the
//! matching `.html`, and a field that does not exist in the template is a
//! build error rather than a runtime blank.

use askama::Template;

/// The public landing page — marketing copy with login and signup CTAs.
#[derive(Template, Default)]
#[template(path = "landing.html")]
pub(crate) struct LandingTemplate;

#[derive(Template, Default)]
#[template(path = "signup.html")]
pub(crate) struct SignupTemplate {
    /// Kept so a rejected form does not make the visitor retype their address.
    /// The password is never echoed back.
    pub(crate) email: String,
    /// `None` on the first visit; a message once something was rejected.
    pub(crate) error: Option<String>,
}

#[derive(Template)]
#[template(path = "login.html")]
pub(crate) struct LoginTemplate {
    pub(crate) error: Option<String>,
    /// Where to go once the password checks out. Already been through
    /// `safe_next`, so it is a path on this site or nothing at all.
    pub(crate) next: String,
}

/// The account page: change password, and log out everywhere.
#[derive(Template)]
#[template(path = "account_password.html")]
pub(crate) struct AccountTemplate {
    pub(crate) email: String,
    /// How many live sessions this account has, so "log out everywhere" has a
    /// visible effect rather than being a button that silently does something.
    pub(crate) sessions: i64,
    pub(crate) error: Option<String>,
    /// Set after a successful action, to say so. Separate from `error` because
    /// the page is re-rendered on failure and on success, but only one of the
    /// two is ever showing.
    pub(crate) notice: Option<String>,
}
