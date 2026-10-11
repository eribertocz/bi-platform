//! Lifecycle of the analytical runtime. It bootstraps in the background and
//! retries with backoff, so the API stays live while the lake is unreachable
//! and readiness reports it.

use std::time::Duration;

use bi_fixtures::sales::FixtureSpec;
use bi_runtime::{Bootstrap, Runtime, RuntimeConfig, RuntimeError, fixtures};
use tokio::sync::watch;

const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Analytics {
    enabled: bool,
    runtime: watch::Receiver<Option<Runtime>>,
}

impl Analytics {
    /// No analytical runtime (API-only); readiness does not check the lake.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            runtime: watch::channel(None).1,
        }
    }

    /// An already running runtime, e.g. in tests.
    #[must_use]
    pub fn ready(runtime: Runtime) -> Self {
        Self {
            enabled: true,
            runtime: watch::channel(Some(runtime)).1,
        }
    }

    /// Bootstraps `config` on a blocking thread, loading the synthetic
    /// `fixture` when given, until it succeeds.
    #[must_use]
    pub fn start(config: RuntimeConfig, fixture: Option<FixtureSpec>) -> Self {
        let (sender, runtime) = watch::channel(None);
        tokio::task::spawn_blocking(move || bootstrap_until_ready(&config, fixture, &sender));
        Self {
            enabled: true,
            runtime,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn runtime(&self) -> Option<Runtime> {
        self.runtime.borrow().clone()
    }

    /// Resolves once the runtime is ready; `false` if it never will be.
    pub async fn wait_until_ready(&self) -> bool {
        self.runtime.clone().wait_for(Option::is_some).await.is_ok()
    }
}

fn bootstrap_until_ready(
    config: &RuntimeConfig,
    fixture: Option<FixtureSpec>,
    sender: &watch::Sender<Option<Runtime>>,
) {
    let mut retry_in = RETRY_MIN;
    loop {
        match bootstrap(config, fixture) {
            Ok(runtime) => {
                tracing::info!(workers = config.workers, "analytical runtime ready");
                sender.send_replace(Some(runtime));
                return;
            }
            Err(error) => tracing::warn!(
                %error,
                retry_in_ms = retry_in.as_millis(),
                "analytical runtime not ready; retrying"
            ),
        }
        if sender.is_closed() {
            return;
        }
        std::thread::sleep(retry_in);
        retry_in = (retry_in * 2).min(RETRY_MAX);
    }
}

fn bootstrap(
    config: &RuntimeConfig,
    fixture: Option<FixtureSpec>,
) -> Result<Runtime, RuntimeError> {
    let bootstrap = Bootstrap::open(config.clone())?;
    if let Some(spec) = fixture {
        let outcome = fixtures::load_sales(&bootstrap, spec)?;
        tracing::info!(
            ?outcome,
            seed = spec.seed,
            rows = spec.rows,
            "development fixture"
        );
    }
    bootstrap.finish()
}
