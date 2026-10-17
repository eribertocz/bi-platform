use super::*;
use proptest::prelude::*;

fn config(issuer: &str, client: &str) -> OidcConfig {
    OidcConfig::from_lookup(
        &|key| match key {
            "BI_OIDC_ISSUER" => Some(issuer.to_owned()),
            "BI_OIDC_CLIENT_ID" => Some(client.to_owned()),
            "BI_OIDC_ATTRIBUTE_CLAIMS" => Some("department".to_owned()),
            _ => None,
        },
        "http://127.0.0.1:8080",
    )
    .unwrap()
    .unwrap()
}

#[test]
fn configuration_is_all_or_nothing_and_redacted() {
    for (key, value) in [
        ("BI_OIDC_CLIENT_SECRET", "secret"),
        ("BI_OIDC_ISSUER", "https://id.example.test"),
    ] {
        assert!(
            OidcConfig::from_lookup(
                &|k| (k == key).then(|| value.to_owned()),
                "http://127.0.0.1:8080"
            )
            .is_err()
        );
    }
    for issuer in [
        "http://id.example.test",
        "https://u:p@id.example.test",
        "https://id.example.test/#bad",
        "https://id.example.test/?bad",
    ] {
        assert!(
            OidcConfig::from_lookup(
                &|key| match key {
                    "BI_OIDC_ISSUER" => Some(issuer.to_owned()),
                    "BI_OIDC_CLIENT_ID" => Some("client".to_owned()),
                    _ => None,
                },
                "http://127.0.0.1:8080"
            )
            .is_err()
        );
    }
    let configured = config("https://id.example.test", "client");
    assert_eq!(format!("{configured:?}"), "OidcConfig { [redacted] }");
    assert_eq!(
        configured.redirect.as_str(),
        "http://127.0.0.1:8080/api/v1/oidc/callback"
    );
}

proptest! {
    #[test]
    fn insecure_issuer_configuration_is_always_rejected(host in "[a-z]{1,30}") {
        let parsed = OidcConfig::from_lookup(&|key| match key {
            "BI_OIDC_ISSUER" => Some(format!("http://{host}.example.test")),
            "BI_OIDC_CLIENT_ID" => Some("client".to_owned()),
            _ => None,
        }, "http://127.0.0.1:8080");
        prop_assert!(parsed.is_err());
    }

    #[test]
    fn arbitrary_cookies_never_authenticate(raw in "[a-zA-Z0-9]{0,200}") {
        let mut headers = HeaderMap::new();
        if let Ok(value) = raw.parse() {
            headers.append(header::COOKIE, value);
            prop_assert!(browser_binding(&headers).is_none());
        }
    }

    #[test]
    fn duplicate_binding_is_always_rejected(value in "[a-f0-9]{64}") {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, format!("bi_oidc={value}; bi_oidc={value}").parse().unwrap());
        prop_assert!(browser_binding(&headers).is_none());
        headers.clear();
        headers.append(header::COOKIE, format!("bi_oidc={value}").parse().unwrap());
        headers.append(header::COOKIE, format!("bi_oidc={value}").parse().unwrap());
        prop_assert!(browser_binding(&headers).is_none());
    }

    #[test]
    fn identity_key_preserves_issuer_subject_boundary(a in "[a-z]{1,30}", b in "[a-z]{1,30}", c in "[a-z]{1,30}") {
        let first = serde_json::to_vec(&(format!("{a}{b}"), &c)).unwrap();
        let second = serde_json::to_vec(&(&a, format!("{b}{c}"))).unwrap();
        prop_assert_ne!(Sha256::digest(first), Sha256::digest(second));
    }
}

#[tokio::test]
async fn audit_events_never_include_callback_input_or_credentials() {
    for raw_id in ["request-1", "bad request?code=secret", ""] {
        let id = RequestId::new(raw_id.parse().unwrap());
        let event = login_event(&id, None).await.unwrap();
        event.validate().unwrap();
        assert!(!event.canonical_json().unwrap().contains("secret"));
        assert_eq!(event.result, bi_audit::ResultCode::Denied);
    }
}

#[test]
fn admission_limits_clients_globally_and_refills_without_sleep() {
    let now = Instant::now();
    let ip = IpAddr::from([127, 0, 0, 1]);
    let mut limit = Admission::new(now);
    for _ in 0..20 {
        assert!(limit.allow(ip, now));
    }
    assert!(!limit.allow(ip, now));
    assert!(limit.allow(ip, now + Duration::from_secs(3)));
    assert!(!limit.allow(ip, now + Duration::from_secs(3)));
    let mut limit = Admission::new(now);
    for n in 0..120 {
        assert!(limit.allow(IpAddr::from([10, 0, 0, n]), now));
    }
    assert!(!limit.allow(ip, now));
    assert!(limit.allow(ip, now + Duration::from_millis(500)));
}

#[test]
fn client_map_is_bounded_and_active_buckets_are_not_evicted() {
    let now = Instant::now();
    let mut limit = Admission::new(now);
    for n in 0..MAX_CLIENTS {
        limit.clients.insert(
            IpAddr::from(u32::try_from(n).unwrap().to_be_bytes()),
            Bucket {
                tokens: 0.0,
                updated: now,
            },
        );
    }
    let ip = IpAddr::from([127, 0, 0, 1]);
    assert!(!limit.allow(ip, now));
    assert_eq!(limit.clients.len(), MAX_CLIENTS);
    assert!(limit.allow(ip, now + Duration::from_secs(60)));
    assert_eq!(limit.clients.len(), 1);
}

proptest! {
    #[test]
    fn admission_never_exceeds_either_burst(clients in prop::collection::vec(0u8..30, 0..300)) {
        let now = Instant::now();
        let mut limit = Admission::new(now);
        let mut counts = BTreeMap::new();
        for client in clients {
            if limit.allow(IpAddr::from([10, 0, 0, client]), now) { *counts.entry(client).or_insert(0usize) += 1; }
        }
        prop_assert!(counts.values().all(|count| *count <= 20));
        prop_assert!(counts.values().sum::<usize>() <= 120);
    }
}

#[cfg(feature = "postgres-tests")]
mod real {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine as _;
    use http_body_util::BodyExt;
    use sqlx::PgPool;
    use tower::ServiceExt;

    fn issuer() -> String {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        std::env::var("BI_TEST_OIDC_ISSUER")
            .expect("postgres-tests require the real Keycloak fixture (docker/oidc)")
    }

    // Actual Keycloak login form and token exchange, without replacing any provider endpoint.
    async fn authorize(url: &str) -> String {
        let http = http_client().unwrap();
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
            .body("username=oidc-user&password=oidc-test-only&credentialId=")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned()
    }

    async fn tokens(
        config: &OidcConfig,
    ) -> (
        DiscoveredClient,
        openidconnect::core::CoreTokenResponse,
        String,
    ) {
        let http = http_client().unwrap();
        let client = config.client(&http).await.expect("real provider discovery");
        let nonce = auth::random_token().await.unwrap();
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let url_nonce = nonce.clone();
        let (url, _, _) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                move || Nonce::new(url_nonce),
            )
            .add_scope(Scope::new("profile".to_owned()))
            .set_pkce_challenge(challenge)
            .url();
        let callback = reqwest::Url::parse(&authorize(url.as_str()).await).unwrap();
        let code = callback
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        let response = client
            .exchange_code(AuthorizationCode::new(code))
            .unwrap()
            .set_pkce_verifier(verifier)
            .request_async(&http)
            .await
            .unwrap();
        (client, response, nonce)
    }

    fn replace_token(
        tokens: &openidconnect::core::CoreTokenResponse,
        jwt: &str,
    ) -> openidconnect::core::CoreTokenResponse {
        let mut value = serde_json::to_value(tokens).unwrap();
        value["id_token"] = serde_json::Value::String(jwt.to_owned());
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Adversarial checks share one real signed token and JWKS.
    async fn real_signed_tokens_reject_signature_issuer_audience_expiry_nonce_and_none() {
        let config = config(&issuer(), "bi-platform");
        let (client, valid, nonce) = tokens(&config).await;
        let (user, identity) = verified_identity(&client, &config, &valid, &nonce).unwrap();
        assert!(user.id.starts_with("oidc:"));
        assert_eq!(identity.issuer, issuer());
        assert_eq!(identity.groups, ["analysts"]);
        assert_eq!(identity.attributes["department"], "finance");
        assert_eq!(identity.attributes.len(), 1);
        assert!(verified_identity(&client, &config, &valid, "other-nonce").is_err());
        assert!(verified_identity(&client, &config, &valid, "").is_err());
        assert!(
            verify_identity_at(
                &client,
                &config,
                &valid,
                &nonce,
                now_seconds().unwrap() - 360
            )
            .is_err()
        );
        assert!(
            verify_identity_at(
                &client,
                &config,
                &valid,
                &nonce,
                now_seconds().unwrap() + 360
            )
            .is_err()
        );
        let jwt = valid.id_token().unwrap().to_string();
        // Property tests mutate the signature of a real token, never a fake issuer.
        let mut runner = proptest::test_runner::TestRunner::default();
        runner
            .run(&(0_usize..256, 1_u8..=255), |(position, xor)| {
                let mut parts: Vec<String> = jwt.split('.').map(str::to_owned).collect();
                let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
                let mut signature = base64.decode(&parts[2]).unwrap();
                let index = position % signature.len();
                signature[index] ^= xor;
                parts[2] = base64.encode(signature);
                prop_assert!(
                    verified_identity(
                        &client,
                        &config,
                        &replace_token(&valid, &parts.join(".")),
                        &nonce
                    )
                    .is_err()
                );
                Ok(())
            })
            .unwrap();
        let mut parts: Vec<String> = jwt.split('.').map(str::to_owned).collect();
        let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut signature = base64.decode(&parts[2]).unwrap();
        signature[0] ^= 1;
        parts[2] = base64.encode(signature);
        assert!(
            verified_identity(
                &client,
                &config,
                &replace_token(&valid, &parts.join(".")),
                &nonce
            )
            .is_err()
        );
        parts[0] = base64.encode(br#"{"alg":"none","typ":"JWT"}"#);
        parts[2] = String::new();
        assert!(
            verified_identity(
                &client,
                &config,
                &replace_token(&valid, &parts.join(".")),
                &nonce
            )
            .is_err()
        );

        let (_, wrong_aud, aud_nonce) = tokens(&super::config(&issuer(), "other-client")).await;
        assert!(verified_identity(&client, &config, &wrong_aud, &aud_nonce).is_err());
        let foreign_config =
            super::config(&issuer().replace("bi-test", "foreign-test"), "bi-platform");
        let (_, foreign, foreign_nonce) = tokens(&foreign_config).await;
        assert!(verified_identity(&client, &config, &foreign, &foreign_nonce).is_err());
        // Independently exercise issuer validation with the foreign provider's real public key.
        let http = http_client().unwrap();
        let metadata = CoreProviderMetadata::discover_async(foreign_config.issuer.clone(), &http)
            .await
            .unwrap()
            .set_issuer(config.issuer.clone());
        let foreign_key_client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            None,
        );
        assert!(verified_identity(&foreign_key_client, &config, &foreign, &foreign_nonce).is_err());

        let expired_config = super::config(&issuer(), "expired-client");
        let (expired_client, expired, expired_nonce) = tokens(&expired_config).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            verified_identity(&expired_client, &expired_config, &expired, &expired_nonce).is_err()
        );
        // A fresh authorization has a new nonce; an old token cannot complete that attempt.
        let (_, _, new_nonce) = tokens(&config).await;
        assert_ne!(nonce, new_nonce);
        assert!(verified_identity(&client, &config, &valid, &new_nonce).is_err());
    }

    async fn send(
        app: &Router,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
    ) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn fresh_jwks_accepts_a_real_provider_key_rotation() {
        // Dedicated realm: changing signing keys must not race the other tests.
        let issuer = issuer().replace("bi-test", "rotation-test");
        let root = issuer.split("/realms/").next().unwrap();
        let config = config(&issuer, "bi-platform");
        let (old_client, _, _) = tokens(&config).await;
        let http = http_client().unwrap();
        let login = http.post(format!("{root}/realms/master/protocol/openid-connect/token"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body("grant_type=password&client_id=admin-cli&username=test-admin&password=oidc-admin-test-only")
            .send().await.unwrap();
        assert!(login.status().is_success());
        let admin: serde_json::Value = serde_json::from_str(&login.text().await.unwrap()).unwrap();
        let authorization = format!("Bearer {}", admin["access_token"].as_str().unwrap());
        let realm = http
            .get(format!("{root}/admin/realms/rotation-test"))
            .header(header::AUTHORIZATION, &authorization)
            .send()
            .await
            .unwrap();
        let realm: serde_json::Value = serde_json::from_str(&realm.text().await.unwrap()).unwrap();
        let key = serde_json::json!({
            "name": "test-rotation", "providerId": "rsa-generated",
            "providerType": "org.keycloak.keys.KeyProvider", "parentId": realm["id"],
            "config": {"priority": ["200"], "enabled": ["true"], "active": ["true"], "algorithm": ["RS256"], "keySize": ["2048"]}
        });
        let response = http
            .post(format!("{root}/admin/realms/rotation-test/components"))
            .header(header::AUTHORIZATION, &authorization)
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&key).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let remove = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let (new_client, rotated, nonce) = tokens(&config).await;
        let old_accepts = verified_identity(&old_client, &config, &rotated, &nonce).is_ok();
        let new_accepts = verified_identity(&new_client, &config, &rotated, &nonce).is_ok();
        assert_eq!(
            http.delete(remove)
                .header(header::AUTHORIZATION, authorization)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(!old_accepts, "the old JWKS must lack the new signing key");
        assert!(
            new_accepts,
            "rediscovery must accept the provider's new key"
        );
    }

    #[sqlx::test(migrations = false)]
    async fn http_limits_reject_both_routes_without_provider_or_writes(pool: PgPool) {
        use opentelemetry::metrics::MeterProvider as _;
        let db = crate::db::Database::start(
            &pool.connect_options(),
            crate::metadata::MetadataMode::Development,
        );
        assert!(db.wait_until_ready().await);
        let mut state = AppState::new(db.clone());
        state.auth.oidc = Some(config(
            "http://127.0.0.1:1/realms/unreachable",
            "bi-platform",
        ));
        let exporter = opentelemetry_sdk::metrics::InMemoryMetricExporter::default();
        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        state.metrics =
            crate::metrics::QueryMetrics::new(&provider.meter("test"), &state.analytics);
        let limit = Arc::new(Mutex::new(Admission::new(Instant::now())));
        let app = routes_with_limit(limit.clone())
            .layer(Extension(RequestId::new("admission-test".parse().unwrap())))
            .with_state(state);
        let ip = IpAddr::from([192, 0, 2, 1]);
        // Every combination must fail before discovery, attempts and callback audit.
        for global in [false, true] {
            for (method, path) in [
                ("POST", "/api/v1/oidc/start"),
                ("GET", "/api/v1/oidc/callback"),
            ] {
                {
                    let now = Instant::now();
                    let mut admission = limit.lock().unwrap();
                    *admission = Admission::new(now);
                    admission.clock = Some(now);
                    if global {
                        admission.global.tokens = 0.0;
                    } else {
                        admission.clients.insert(
                            ip,
                            Bucket {
                                tokens: 0.0,
                                updated: now,
                            },
                        );
                    }
                }
                let request = Request::builder()
                    .method(method)
                    .uri(path)
                    .extension(ConnectInfo(SocketAddr::new(ip, 1234)))
                    .header("x-forwarded-for", "192.0.2.99")
                    .header("origin", "http://127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
                assert!(!response.headers().contains_key(header::SET_COOKIE));
                let body = response.into_body().collect().await.unwrap().to_bytes();
                assert!(std::str::from_utf8(&body).unwrap().contains("rate_limited"));
            }
        }
        let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.oidc_attempts")
            .fetch_one(&pool)
            .await
            .unwrap();
        let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.audit_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(attempts, 0);
        assert_eq!(audited, 0);
        provider.force_flush().unwrap();
        let metrics = exporter.get_finished_metrics().unwrap();
        let point = metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .find(|m| m.name() == "bi_oidc_login")
            .unwrap();
        let opentelemetry_sdk::metrics::data::AggregatedMetrics::U64(
            opentelemetry_sdk::metrics::data::MetricData::Sum(sum),
        ) = point.data()
        else {
            panic!("expected counter");
        };
        let point = sum.data_points().next().unwrap();
        assert_eq!(point.value(), 4);
        assert!(
            point
                .attributes()
                .any(|a| a.key.as_str() == "code" && a.value.as_str() == "rate_limited")
        );
        // Admitted rejections still append to the audit chain.
        *limit.lock().unwrap() = Admission::new(Instant::now());
        {
            let mut admission = limit.lock().unwrap();
            // Audit I/O may take more than 500 ms, enough to refill a token.
            // Freeze admission time so this tests sharing between routes.
            admission.clock = Some(admission.global.updated);
            admission.global.tokens = 1.0;
        }
        assert_eq!(
            send(&app, "GET", "/api/v1/oidc/callback", None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &app,
                "POST",
                "/api/v1/oidc/start",
                None,
                Some("http://127.0.0.1:8080")
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.audit_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(audited, 1);
        db.close().await;
    }

    #[sqlx::test(migrations = false)]
    async fn live_attempt_capacity_is_atomic_and_http_returns_503(pool: PgPool) {
        let db = crate::db::Database::start(
            &pool.connect_options(),
            crate::metadata::MetadataMode::Development,
        );
        assert!(db.wait_until_ready().await);
        sqlx::query("INSERT INTO bi_meta.oidc_attempts (state_hash, browser_hash, nonce, verifier) SELECT sha256(n::text::bytea), sha256(n::text::bytea), 'nonce', 'verifier' FROM generate_series(1, $1) n")
            .bind(MAX_LIVE_ATTEMPTS - 1).execute(&pool).await.unwrap();
        let (a, b) = tokio::join!(
            save_attempt(
                pool.begin().await.unwrap(),
                vec![1; 32],
                vec![1; 32],
                "nonce".into(),
                "verifier".into(),
                None
            ),
            save_attempt(
                pool.begin().await.unwrap(),
                vec![2; 32],
                vec![2; 32],
                "nonce".into(),
                "verifier".into(),
                None
            )
        );
        assert_ne!(a.unwrap(), b.unwrap(), "only one concurrent insert fits");
        let mut state = AppState::new(db.clone());
        state.auth.oidc = Some(config(&issuer(), "bi-platform"));
        let app = crate::router(state);
        let response = send(
            &app,
            "POST",
            "/api/v1/oidc/start",
            None,
            Some("http://127.0.0.1:8080"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.oidc_attempts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, MAX_LIVE_ATTEMPTS);
        sqlx::query(
            "UPDATE bi_meta.oidc_attempts SET expires_at = clock_timestamp() - interval '1 second'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            save_attempt(
                pool.begin().await.unwrap(),
                vec![3; 32],
                vec![3; 32],
                "nonce".into(),
                "verifier".into(),
                None
            )
            .await
            .unwrap()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.oidc_attempts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        db.close().await;
    }

    async fn begin(app: &Router) -> (String, String) {
        let response = send(
            app,
            "POST",
            "/api/v1/oidc/start",
            None,
            Some("http://127.0.0.1:8080"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Lax; Max-Age=300"));
        let cookie = cookie.split(';').next().unwrap().to_owned();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let login: LoginStart = serde_json::from_slice(&body).unwrap();
        (login.authorization_url, cookie)
    }

    /// The restricted mode (#115) against the real provider: the documented
    /// grants (`admin/runtime-grants.sql`) are enough for a whole login —
    /// attempt lock, cleanup, count and insert; atomic consumption; session
    /// and audit event — and a session never exists without its event.
    #[sqlx::test(migrations = false)]
    #[allow(clippy::too_many_lines)] // One administrative setup and two logins.
    async fn limited_runtime_role_completes_a_real_login_atomically(pool: PgPool) {
        use sqlx::AssertSqlSafe;
        const PASSWORD: &str = "decoy-password-7f3a";
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let tag: String = database
            .chars()
            .rev()
            .filter(char::is_ascii_alphanumeric)
            .take(16)
            .collect::<String>()
            .to_ascii_lowercase();
        let (owner, runtime) = (format!("o{tag}_owner"), format!("o{tag}_runtime"));
        let sql = |statement: String| {
            let pool = pool.clone();
            async move {
                sqlx::raw_sql(AssertSqlSafe(statement))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        sql(format!(
            "DROP ROLE IF EXISTS {runtime}; DROP ROLE IF EXISTS {owner}; \
             CREATE ROLE {owner} LOGIN PASSWORD '{PASSWORD}'; \
             CREATE ROLE {runtime} LOGIN PASSWORD '{PASSWORD}'; \
             ALTER DATABASE \"{database}\" OWNER TO {owner};"
        ))
        .await;
        let options = |role: &str| {
            pool.connect_options()
                .as_ref()
                .clone()
                .username(role)
                .password(PASSWORD)
        };
        crate::db::migrate(&options(&owner), crate::db::MIGRATION_TIMEOUT)
            .await
            .unwrap();
        let administrative = PgPool::connect_lazy_with(options(&owner));
        sqlx::raw_sql(AssertSqlSafe(
            include_str!("../../admin/runtime-grants.sql").replace("bi_runtime", &runtime),
        ))
        .execute(&administrative)
        .await
        .unwrap();
        administrative.close().await;

        let db = crate::db::Database::start(
            &options(&runtime),
            crate::metadata::MetadataMode::Restricted,
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(30), db.wait_until_ready())
                .await
                .unwrap()
        );
        let mut state = AppState::new(db.clone());
        state.auth.oidc = Some(config(&issuer(), "bi-platform"));
        let app = crate::router(state);
        let login = |app: Router| async move {
            let (url, cookie) = begin(&app).await;
            let callback = reqwest::Url::parse(&authorize(&url).await).unwrap();
            let path = format!("{}?{}", callback.path(), callback.query().unwrap());
            send(&app, "GET", &path, Some(&cookie), None).await
        };
        let response = login(app.clone()).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let session = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .find(|v| v.starts_with("bi_session="))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&session), None)
                .await
                .status(),
            StatusCode::OK
        );
        let count = |table: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(AssertSqlSafe(format!(
                    "SELECT count(*) FROM bi_meta.{table}"
                )))
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(count("sessions").await, 1);
        assert_eq!(count("audit_events").await, 1);

        // A corporate session cannot use the G1 query route (#118): refused
        // before its body is read, and the refusal is audited.
        let legacy = app
            .clone()
            .oneshot(
                Request::post("/api/v1/queries")
                    .header(header::COOKIE, &session)
                    .header(header::ORIGIN, "http://127.0.0.1:8080")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"dataset":"sales","measures":["row_count"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::FORBIDDEN);
        let problem: serde_json::Value =
            serde_json::from_slice(&legacy.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(problem["code"], "legacy_query_disabled");
        assert_eq!(count("audit_events").await, 2);

        // Without its audit path the role cannot create a session at all.
        sql(format!(
            "REVOKE EXECUTE ON FUNCTION bi_meta.audit_append(text) FROM {runtime}"
        ))
        .await;
        let refused = login(app.clone()).await;
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(count("sessions").await, 1, "no session without its event");
        assert_eq!(count("audit_events").await, 2);
        crate::audit::verify(&pool, None).await.unwrap();

        db.close().await;
        sql(format!(
            "ALTER DATABASE \"{database}\" OWNER TO CURRENT_USER; \
             REASSIGN OWNED BY {owner}, {runtime} TO CURRENT_USER; \
             DROP OWNED BY {owner}, {runtime}; DROP ROLE {runtime}; DROP ROLE {owner};"
        ))
        .await;
    }

    #[sqlx::test(migrations = false)]
    #[allow(clippy::too_many_lines)] // Keep the login, reauthentication and logout workflow together.
    async fn real_callback_creates_persistent_session_and_consumes_bound_state(pool: PgPool) {
        let db = crate::db::Database::start(
            &pool.connect_options(),
            crate::metadata::MetadataMode::Development,
        );
        assert!(db.wait_until_ready().await);
        let mut state = AppState::new(db.clone());
        state.auth.oidc = Some(config(&issuer(), "bi-platform"));
        let app = crate::router(state.clone());
        for origin in [None, Some("http://evil.test"), Some("null")] {
            assert_eq!(
                send(&app, "POST", "/api/v1/oidc/start", None, origin)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        for param in [
            "redirect_uri=https://evil.test",
            "return_to=//evil.test",
            "next=https://evil.test",
        ] {
            assert_eq!(
                send(
                    &app,
                    "POST",
                    &format!("/api/v1/oidc/start?{param}"),
                    None,
                    Some("http://127.0.0.1:8080")
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let (url, cookie) = begin(&app).await;
        let callback = reqwest::Url::parse(&authorize(&url).await).unwrap();
        let path = format!("{}?{}", callback.path(), callback.query().unwrap());
        // The code is real; absent, duplicate or another browser's binding cannot consume it.
        for binding in [
            None,
            Some("bi_oidc=bad"),
            Some(&format!("bi_oidc={}", "a".repeat(64))),
            Some(&format!("{cookie}; {cookie}")),
        ] {
            assert_eq!(
                send(&app, "GET", &path, binding, None).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            send(
                &app,
                "GET",
                "/api/v1/oidc/callback?code=unused",
                Some(&cookie),
                None
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let foreign_state = format!("/api/v1/oidc/callback?state={}&code=unused", "b".repeat(64));
        assert_eq!(
            send(&app, "GET", &foreign_state, Some(&cookie), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        for extra in [
            format!("state={}", "c".repeat(64)),
            "next=https://evil.test".to_owned(),
        ] {
            assert_eq!(
                send(&app, "GET", &format!("{path}&{extra}"), Some(&cookie), None)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let (first, second) = tokio::join!(
            // A single database DELETE arbitrates concurrent callbacks.
            send(&app, "GET", &path, Some(&cookie), None),
            send(&app, "GET", &path, Some(&cookie), None)
        );
        let (response, rejected) = if first.status() == StatusCode::SEE_OTHER {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/");
        let session = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .find(|v| v.starts_with("bi_session="))
            .unwrap()
            .to_owned();
        assert!(session.contains("HttpOnly; SameSite=Strict"));
        let session = session.split(';').next().unwrap();
        assert_eq!(
            send(&app, "GET", &path, Some(&cookie), None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let restarted = crate::router(state);
        assert_eq!(
            send(&restarted, "GET", "/api/v1/session", Some(session), None)
                .await
                .status(),
            StatusCode::OK
        );
        let identity: String =
            sqlx::query_scalar("SELECT federated_identity::text FROM bi_meta.sessions")
                .fetch_one(&pool)
                .await
                .unwrap();
        let identity: FederatedIdentity = serde_json::from_str(&identity).unwrap();
        assert_eq!(identity.groups, ["analysts"]);
        let events: Vec<String> =
            sqlx::query_scalar("SELECT payload FROM bi_meta.audit_events ORDER BY sequence")
                .fetch_all(&pool)
                .await
                .unwrap();
        let events: Vec<bi_audit::Event> = events
            .iter()
            .map(|json| serde_json::from_str(json).unwrap())
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.result == bi_audit::ResultCode::Success)
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|e| e.result == bi_audit::ResultCode::Denied)
        );
        crate::audit::verify(&pool, None).await.unwrap();
        // Continue on the restarted server already checked above; admission is
        // process-local and the first phase deliberately exhausted many tokens.
        let app = restarted.clone();
        // Reauthentication rotates the session and revokes the previous one.
        let start = send(
            &app,
            "POST",
            "/api/v1/oidc/start",
            Some(session),
            Some("http://127.0.0.1:8080"),
        )
        .await;
        assert_eq!(start.status(), StatusCode::OK);
        let binding = start.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let body = start.into_body().collect().await.unwrap().to_bytes();
        let login: LoginStart = serde_json::from_slice(&body).unwrap();
        let rotated = reqwest::Url::parse(&authorize(&login.authorization_url).await).unwrap();
        let rotated_path = format!("{}?{}", rotated.path(), rotated.query().unwrap());
        let response = send(&app, "GET", &rotated_path, Some(&binding), None).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let replacement = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .find(|v| v.starts_with("bi_session="))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_ne!(replacement, session);
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(session), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let session = replacement.as_str();
        assert_eq!(
            send(
                &restarted,
                "POST",
                "/api/v1/logout",
                Some(session),
                Some("http://evil.test")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(
                &restarted,
                "POST",
                "/api/v1/logout",
                Some(session),
                Some("http://127.0.0.1:8080")
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&restarted, "GET", "/api/v1/session", Some(session), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );

        // A bad nonce consumes state; even correcting the record cannot resurrect the attempt.
        let (url, cookie) = begin(&app).await;
        let callback = reqwest::Url::parse(&authorize(&url).await).unwrap();
        let path = format!("{}?{}", callback.path(), callback.query().unwrap());
        sqlx::query("UPDATE bi_meta.oidc_attempts SET nonce = 'wrong-nonce'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            send(&app, "GET", &path, Some(&cookie), None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(&app, "GET", &path, Some(&cookie), None).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let (url, cookie) = begin(&app).await;
        let callback = reqwest::Url::parse(&authorize(&url).await).unwrap();
        let path = format!("{}?{}", callback.path(), callback.query().unwrap());
        sqlx::query(
            "UPDATE bi_meta.oidc_attempts SET expires_at = clock_timestamp() - interval '1 second'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            send(&app, "GET", &path, Some(&cookie), None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        db.close().await;
    }

    #[sqlx::test(migrations = false)]
    async fn pkce_failure_and_audit_failure_cannot_create_sessions(pool: PgPool) {
        let db = crate::db::Database::start(
            &pool.connect_options(),
            crate::metadata::MetadataMode::Development,
        );
        assert!(db.wait_until_ready().await);
        let mut state = AppState::new(db.clone());
        state.auth.oidc = Some(config(&issuer(), "bi-platform"));
        let app = crate::router(state);
        let (url, cookie) = begin(&app).await;
        let callback = reqwest::Url::parse(&authorize(&url).await).unwrap();
        let path = format!("{}?{}", callback.path(), callback.query().unwrap());
        sqlx::query("UPDATE bi_meta.oidc_attempts SET verifier = repeat('a', 64)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            send(&app, "GET", &path, Some(&cookie), None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let (url, cookie) = begin(&app).await;
        let callback = reqwest::Url::parse(&authorize(&url).await).unwrap();
        let path = format!("{}?{}", callback.path(), callback.query().unwrap());
        // Break the append adapter after a genuine authorization and token exchange.
        sqlx::query("DROP FUNCTION bi_meta.audit_append(TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let response = send(&app, "GET", &path, Some(&cookie), None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            !response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|v| v.to_str().unwrap().starts_with("bi_session="))
        );
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0);
        db.close().await;
    }
}
