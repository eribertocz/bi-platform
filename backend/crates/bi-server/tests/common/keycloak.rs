//! The real login form of the OIDC fixture (`docker/oidc`), shared by the
//! router tests (`src/oidc/tests.rs`) and the conformance target
//! (`tests/semantic_conformance.rs`): no provider endpoint is replaced.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header;

/// The login of `username` (password `oidc-test-only`) at the real form the
/// authorization `url` opens: the provider's redirect back to the
/// application, with its code and state.
pub async fn authorize_as(url: &str, username: &str) -> String {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = http.get(url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("; ");
    let html = response.text().await.unwrap();
    let form = html
        .split("id=\"kc-form-login\"")
        .nth(1)
        .expect("real Keycloak login form");
    let action = form
        .split("action=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .replace("&amp;", "&");
    let response = http
        .post(action)
        .header(header::COOKIE, cookies)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "username={username}&password=oidc-test-only&credentialId="
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned()
}
