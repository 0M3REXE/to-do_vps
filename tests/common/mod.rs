//! # Shared test helpers
//!
//! Lives in a directory because every `.rs` directly in `tests/` becomes its
//! own test binary. Helpers drive the router via `oneshot` — real routing and
//! middleware, with no socket and no port.
//!
//! Most requests need a session cookie. [`Client`] is a very small cookie jar:
//! it remembers the `Set-Cookie` a response produced and sends it back on the
//! next request, which is what a browser does and what the session layer
//! expects to see.
//!
//! ## Databases
//!
//! `#[sqlx::test]` used to give every test its own throwaway Postgres. The
//! replacement is the same idea against libSQL: [`db()`] builds a fresh
//! in-memory database with migrations applied, so tests run in parallel with
//! no server and no files left behind.

#![allow(dead_code)]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
};
use todo::{
    AppConfig,
    auth::{MAX_PASSWORD_LEN, MIN_PASSWORD_LEN, create_user, password, user_id_by_email},
    db,
    todo::add,
};
use tower::ServiceExt;

/// The account the pre-existing todo helpers sign in as. One fixed address so a
/// test can mix `seed`, `get_index` and `post_todo` without colliding on the
/// unique email index.
pub const TEST_EMAIL: &str = "tests@example.com";

/// A password that satisfies the policy, for tests that need one.
pub const GOOD_PASSWORD: &str = "correct horse battery";

/// Re-exported so tests can assert the policy without hard-coding numbers.
pub const POLICY_MIN: usize = MIN_PASSWORD_LEN;
pub const POLICY_MAX: usize = MAX_PASSWORD_LEN;

/// A fresh in-memory database with the whole schema applied. Every call is its
/// own private world: two tests that both insert `tests@example.com` never see
/// each other.
pub async fn db() -> db::Db {
    db::open_in_memory()
        .await
        .expect("in-memory database should open")
}

/// Every test gets a router with auth wired in, so tests exercise real sessions
/// rather than a stand-in for them.
pub fn router(database: &db::Db, config: AppConfig) -> Router {
    todo::app::build_app(database.clone(), config)
}

/// The default wiring, for tests that use the default configuration.
pub fn router_default(database: &db::Db) -> Router {
    router(database, AppConfig::default())
}

/// Percent-encodes the characters that would otherwise be read as form syntax.
/// Tests need this because some of the payloads under test contain `&` and `=`.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn form_body(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A cookie jar plus the configuration of the router it drives. Every method
/// takes the test's database explicitly, so a client and the test helpers
/// always operate on the same world.
pub struct Client {
    /// Cookie jar keyed by cookie name.
    cookies: std::collections::BTreeMap<String, String>,
    config: AppConfig,
}

impl Client {
    /// A visitor who has never been here.
    pub fn anonymous() -> Self {
        Self::with_config(AppConfig::default())
    }

    /// A visitor with an explicit configuration.
    pub fn with_config(config: AppConfig) -> Self {
        Self {
            cookies: std::collections::BTreeMap::new(),
            config,
        }
    }

    /// The router this client drives, holding its own configuration.
    fn router(&self, database: &db::Db) -> Router {
        router(database, self.config.clone())
    }

    /// Logs in the shared test account on `database`, so a helper that only
    /// needs "a signed-in visitor" can get one without knowing about the
    /// account's fixtures.
    pub async fn sign_in_test_account(&mut self, database: &db::Db) {
        ensure_user(database, TEST_EMAIL).await;
        let (status, body) = self
            .post_form(
                database,
                "/login",
                &[
                    ("email", TEST_EMAIL),
                    ("password", GOOD_PASSWORD),
                    ("next", ""),
                ],
            )
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "login failed: {body}");
    }

    /// A visitor who has just signed up. Goes through the real signup handler
    /// rather than inserting a row, so the test also proves signup works.
    pub async fn signed_up(database: &db::Db, email: &str) -> Self {
        let mut client = Self::anonymous();
        let (status, body) = client.signed_up_with(database, email).await;
        assert_eq!(
            status,
            StatusCode::SEE_OTHER,
            "signup should have succeeded, got {status}: {body}"
        );
        client
    }

    /// Signs up, leaving the status to the caller so a test can assert on a
    /// signup that is expected to be refused.
    pub async fn signed_up_with(
        &mut self,
        database: &db::Db,
        email: &str,
    ) -> (StatusCode, String) {
        self.post_form(
            database,
            "/signup",
            &[
                ("email", email),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await
    }

    /// A client holding the cookie of an account that already exists. Used
    /// where the test cares about something other than how the session started.
    pub async fn for_email(database: &db::Db, email: &str) -> Self {
        Self::for_email_with(database, email, AppConfig::default()).await
    }

    /// [`Client::for_email`] with an explicit configuration.
    pub async fn for_email_with(
        database: &db::Db,
        email: &str,
        config: AppConfig,
    ) -> Self {
        let mut client = Self::with_config(config);
        ensure_user(database, email).await;
        let (status, body) = client
            .post_form(
                database,
                "/login",
                &[("email", email), ("password", GOOD_PASSWORD), ("next", "")],
            )
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "login failed: {body}");
        client
    }

    /// Sends one request, recording any cookie the response set.
    pub async fn request(
        &mut self,
        database: &db::Db,
        builder: Request<Body>,
    ) -> (StatusCode, String, HeaderMap) {
        let response = self
            .router(database)
            .oneshot(builder)
            .await
            .expect("oneshot should not error");

        let status = response.status();
        let headers = response.headers().clone();

        // Remember every cookie the server just set. A logout sends an expired
        // cookie under the same name, which replaces the stored one — the same
        // thing a browser does.
        Self::parse_set_cookie(&headers, &mut self.cookies);

        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body bytes")
                .to_vec(),
        )
        .expect("body utf8");

        (status, text, headers)
    }

    /// Starts a request, carrying all cookies and any extra headers.
    fn start(&self, method: &str, uri: &str) -> axum::http::request::Builder {
        let builder = Request::builder().method(method).uri(uri);
        match self.cookie_header() {
            Some(cookie) => builder.header(header::COOKIE, cookie),
            None => builder,
        }
    }

    /// A plain GET.
    pub async fn get(&mut self, database: &db::Db, uri: &str) -> (StatusCode, String) {
        let request = self.start("GET", uri).body(Body::empty()).expect("build");
        let (status, body, _) = self.request(database, request).await;
        (status, body)
    }

    /// A GET that claims to be htmx, which is what changes how an
    /// unauthenticated request is answered.
    pub async fn get_htmx(
        &mut self,
        database: &db::Db,
        uri: &str,
    ) -> (StatusCode, String, HeaderMap) {
        let request = self
            .start("GET", uri)
            .header("hx-request-type", "partial")
            .body(Body::empty())
            .expect("build");
        self.request(database, request).await
    }

    /// A GET with extra headers, for the origin checks.
    pub async fn get_with_headers(
        &mut self,
        database: &db::Db,
        uri: &str,
        extra: &[(&str, &str)],
    ) -> (StatusCode, String, HeaderMap) {
        let mut builder = self.start("GET", uri);
        for (name, value) in extra {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(Body::empty()).expect("build");
        self.request(database, request).await
    }

    /// POST an HTML form.
    pub async fn post_form(
        &mut self,
        database: &db::Db,
        uri: &str,
        fields: &[(&str, &str)],
    ) -> (StatusCode, String) {
        self.post_form_with(database, uri, fields, &[]).await
    }

    /// POST a form with extra headers, for the cross-origin test.
    pub async fn post_form_with(
        &mut self,
        database: &db::Db,
        uri: &str,
        fields: &[(&str, &str)],
        extra: &[(&str, &str)],
    ) -> (StatusCode, String) {
        let payload = form_body(fields);
        let mut builder = self
            .start("POST", uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");

        for (name, value) in extra {
            builder = builder.header(*name, *value);
        }

        let request = builder.body(Body::from(payload)).expect("build");
        let (status, body, _) = self.request(database, request).await;
        (status, body)
    }

    /// POST a todo from the panel — what htmx actually sends.
    pub async fn post_todo(
        &mut self,
        database: &db::Db,
        text: &str,
    ) -> (StatusCode, String) {
        self.post_form(database, "/app/todos", &[("text", text)]).await
    }

    /// POST a body exactly as given, with no encoding applied. Several tests
    /// care about the precise bytes on the wire — a malformed body, a payload
    /// with its escapes already in place — so they must not be re-encoded.
    pub async fn post_raw(
        &mut self,
        database: &db::Db,
        uri: &str,
        body: &str,
    ) -> (StatusCode, String) {
        let request = self
            .start("POST", uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body.to_owned()))
            .expect("build");
        let (status, body, _) = self.request(database, request).await;
        (status, body)
    }

    /// POST a form and report where the server told the browser to go.
    /// Redirect bodies are empty, so tests asserting a destination need the
    /// `Location` header, which [`Client::post_form`] throws away.
    pub async fn post_form_locating(
        &mut self,
        database: &db::Db,
        uri: &str,
        fields: &[(&str, &str)],
    ) -> (StatusCode, String) {
        let payload = form_body(fields);
        let request = self
            .start("POST", uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(payload))
            .expect("build");

        let (status, _, headers) = self.request(database, request).await;
        let location = headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        (status, location)
    }

    /// A GET reporting where the server sent the browser. Same reason as
    /// [`Client::post_form_locating`].
    pub async fn get_locating(&mut self, database: &db::Db, uri: &str) -> (StatusCode, String) {
        let request = self.start("GET", uri).body(Body::empty()).expect("build");
        let (status, _, headers) = self.request(database, request).await;
        let location = headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        (status, location)
    }

    /// The raw session cookie header value, or `None` if the jar is empty.
    pub fn cookie(&self) -> Option<String> {
        self.cookies
            .iter()
            .find(|(name, _)| *name == "session" || *name == "__Host-session")
            .map(|(name, value)| format!("{name}={value}"))
    }

    /// Every cookie in the jar, ready to send as one `Cookie:` header.
    pub fn cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            None
        } else {
            Some(
                self
                    .cookies
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }

    /// The device cookie pair, if present.
    pub fn device_cookie(&self) -> Option<String> {
        self.cookies
            .iter()
            .find(|(name, _)| *name == "device" || *name == "__Host-device")
            .map(|(name, value)| format!("{name}={value}"))
    }

    /// A visitor who presents a session cookie handed to them elsewhere.
    /// Covers the stale-cookie case: the server must greet a dead session with
    /// the login page, not a crash.
    pub fn with_replayed_cookie(raw: impl Into<String>) -> Self {
        Self::with_replayed_cookies(raw)
    }

    /// A visitor who presents a full cookie header handed to them elsewhere.
    pub fn with_replayed_cookies(raw: impl Into<String>) -> Self {
        let mut client = Self::with_config(AppConfig::default());
        for pair in raw.into().split(';') {
            let pair = pair.trim();
            if let Some((name, value)) = pair.split_once('=') {
                client.cookies.insert(name.trim().to_owned(), value.trim().to_owned());
            }
        }
        client
    }

    fn parse_set_cookie(headers: &HeaderMap, jar: &mut std::collections::BTreeMap<String, String>) {
        for value in headers.get_all(header::SET_COOKIE) {
            let Ok(text) = value.to_str() else { continue };
            let pair = text.split(';').next().unwrap_or_default().trim();
            let Some((name, value)) = pair.split_once('=') else { continue };
            let name = name.trim().to_owned();
            let value = value.trim().to_owned();
            let lower = text.to_ascii_lowercase();
            if value.is_empty() || lower.contains("max-age=0") {
                jar.remove(&name);
            } else {
                jar.insert(name, value);
            }
        }
    }
}

/// The id of the shared test account. Each test gets its own database, so this
/// always refers to the same fresh account within that test.
pub async fn user_id(database: &db::Db) -> i64 {
    ensure_user(database, TEST_EMAIL).await
}

/// Returns the id of `email`, creating the account the first time it is asked
/// for. Lets one test mix helpers freely without tripping the unique index.
pub async fn ensure_user(database: &db::Db, email: &str) -> i64 {
    if let Some(id) = user_id_by_email(database, email).await.expect("lookup") {
        return id;
    }

    let hash = password::hash(GOOD_PASSWORD.to_owned())
        .await
        .expect("hash");

    create_user(database, email, &hash)
        .await
        .expect("insert")
        .expect("account should not already exist")
        .id
}

/// Creates an account and returns its id, for tests that need a user but do not
/// care how they got one.
pub async fn make_user(database: &db::Db, email: &str) -> i64 {
    let hash = password::hash(GOOD_PASSWORD.to_owned())
        .await
        .expect("hash");

    create_user(database, email, &hash)
        .await
        .expect("insert")
        .expect("not a duplicate")
        .id
}

// --- the original todo helpers -----------------------------------------------
// These gained a user, because a todo now belongs to one. The signatures are
// unchanged so the existing tests read the same as before.

pub async fn seed(database: &db::Db, texts: &[&str]) {
    let user_id = ensure_user(database, TEST_EMAIL).await;
    for text in texts {
        add(database, user_id, text).await.expect("seed insert");
    }
}

pub async fn count(database: &db::Db) -> i64 {
    db::scalar_i64(database, "select count(*) from todos", vec![])
        .await
        .expect("count")
}

pub async fn ids(database: &db::Db) -> Vec<i64> {
    db::query_all(
        database,
        "select id from todos order by id",
        vec![],
        |row| db::ri(row, 0),
    )
    .await
    .expect("ids")
}

pub async fn text_of(database: &db::Db, id: i64) -> Option<String> {
    db::opt_query(
        database,
        "select text from todos where id = ?1",
        vec![db::pi(id)],
        |row| db::rs(row, 0),
    )
    .await
    .expect("text")
}

pub async fn done_of(database: &db::Db, id: i64) -> bool {
    db::scalar_i64(
        database,
        "select done from todos where id = ?1",
        vec![db::pi(id)],
    )
    .await
    .expect("done")
    != 0
}

pub async fn only_id(database: &db::Db) -> i64 {
    ids(database).await[0]
}

/// A GET as a signed-in visitor.
///
/// These two predate authentication and used to be anonymous. They now sign in
/// as the shared test account, because the routes they exercise all sit behind
/// the login guard — what each test is really asserting (400 on a bad id, 405
/// on the wrong method, the rendered fragment) is unchanged.
pub async fn get_to(database: &db::Db, uri: &str) -> (StatusCode, String) {
    let mut client = Client::anonymous();
    client.sign_in_test_account(database).await;
    client.get(database, uri).await
}

pub async fn post_to(database: &db::Db, uri: &str) -> (StatusCode, String) {
    let mut client = Client::anonymous();
    client.sign_in_test_account(database).await;
    client.post_form(database, uri, &[]).await
}

pub async fn post_todo(database: &db::Db, body: &'static str) -> (StatusCode, String) {
    let mut client = Client::anonymous();
    client.sign_in_test_account(database).await;
    client.post_raw(database, "/app/todos", body).await
}

pub async fn get_index(database: &db::Db) -> String {
    let mut client = Client::anonymous();
    client.sign_in_test_account(database).await;
    let (status, body) = client.get(database, "/app").await;
    assert_eq!(status, StatusCode::OK, "index should render: {body}");
    body
}
