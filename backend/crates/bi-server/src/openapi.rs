//! `OpenAPI` document generated from the handlers. `docs/api/openapi.json` must
//! match it exactly; regenerate with `cargo run -p bi-server -- openapi <path>`.

use utoipa::OpenApi;

use crate::{auth, health, models, oidc, query};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "bi-platform API",
        description = "Contrato HTTP vigente. Única fuente de tipos para servidor y cliente.",
        version = "0.1.0"
    ),
    paths(
        health::live,
        health::ready,
        auth::current,
        oidc::available,
        oidc::start,
        oidc::callback,
        auth::logout,
        auth::dev_login,
        auth::dev_users,
        query::run,
        models::list
    ),
    components(schemas(crate::problem::Problem)),
    security(),
    modifiers(&SessionSecurity)
)]
struct ApiDoc;

struct SessionSecurity;

impl utoipa::Modify for SessionSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
        openapi
            .components
            .as_mut()
            .expect("schemas exist")
            .add_security_scheme(
                "sessionCookie",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("bi_session"))),
            );
    }
}

/// The `OpenAPI` document served by this crate.
#[must_use]
pub fn document() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    // utoipa fills an empty license from Cargo metadata; the product license is
    // still an open decision, so the contract declares none.
    doc.info.license = None;
    doc
}

/// The `OpenAPI` document as pretty JSON with a trailing newline.
///
/// # Panics
/// Never in practice: the document only contains JSON-serializable values.
#[must_use]
pub fn json() -> String {
    let mut json = document()
        .to_pretty_json()
        .expect("OpenAPI document serializes to JSON");
    json.push('\n');
    json
}
