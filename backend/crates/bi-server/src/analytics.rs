//! Lifecycle of the analytical runtime. It bootstraps in the background and
//! retries with backoff, so the API stays live while the lake is unreachable
//! and readiness reports it.

use std::time::Duration;

use bi_fixtures::sales::FixtureSpec;
use bi_runtime::{Bootstrap, LoadOutcome, Runtime, RuntimeConfig, RuntimeError, fixtures};
use tokio::sync::watch;

const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// What a runtime's own bootstrap established about its data. Only the
/// bootstrap sets it; a request, a header or the name of a table never can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// The synthetic `sales` fixture, written or compared row by row with the
    /// generated fixture before the runtime started serving.
    SyntheticSales(FixtureSpec),
    /// Anything else: no fixture requested, a lake whose fixture table was
    /// altered, or a runtime built outside this bootstrap.
    Unknown,
}

/// A runtime together with its provenance, read in one step so a decision
/// about one never applies to another.
#[derive(Clone)]
pub struct Instance {
    pub runtime: Runtime,
    pub provenance: Provenance,
}

#[derive(Clone)]
pub struct Analytics {
    enabled: bool,
    instance: watch::Receiver<Option<Instance>>,
}

impl Analytics {
    /// No analytical runtime (API-only); readiness does not check the lake.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            instance: watch::channel(None).1,
        }
    }

    /// An already running runtime, e.g. in tests. Its data was not loaded
    /// here, so its provenance is [`Provenance::Unknown`].
    #[must_use]
    pub fn ready(runtime: Runtime) -> Self {
        Self {
            enabled: true,
            instance: watch::channel(Some(Instance {
                runtime,
                provenance: Provenance::Unknown,
            }))
            .1,
        }
    }

    /// Bootstraps `config` on a blocking thread, loading the synthetic
    /// `fixture` when given, until it succeeds.
    #[must_use]
    pub fn start(config: RuntimeConfig, fixture: Option<FixtureSpec>) -> Self {
        let (sender, instance) = watch::channel(None);
        tokio::task::spawn_blocking(move || bootstrap_until_ready(&config, fixture, &sender));
        Self {
            enabled: true,
            instance,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn runtime(&self) -> Option<Runtime> {
        self.instance
            .borrow()
            .as_ref()
            .map(|instance| instance.runtime.clone())
    }

    /// The runtime and its provenance, as one snapshot.
    #[must_use]
    pub fn instance(&self) -> Option<Instance> {
        self.instance.borrow().clone()
    }

    /// Resolves once the runtime is ready; `false` if it never will be.
    pub async fn wait_until_ready(&self) -> bool {
        self.instance
            .clone()
            .wait_for(Option::is_some)
            .await
            .is_ok()
    }
}

fn bootstrap_until_ready(
    config: &RuntimeConfig,
    fixture: Option<FixtureSpec>,
    sender: &watch::Sender<Option<Instance>>,
) {
    let mut retry_in = RETRY_MIN;
    loop {
        match bootstrap(config, fixture) {
            Ok(instance) => {
                tracing::info!(
                    workers = config.workers,
                    synthetic = matches!(instance.provenance, Provenance::SyntheticSales(_)),
                    "analytical runtime ready"
                );
                sender.send_replace(Some(instance));
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
) -> Result<Instance, RuntimeError> {
    let bootstrap = Bootstrap::open(config.clone())?;
    let mut provenance = Provenance::Unknown;
    if let Some(spec) = fixture {
        let outcome = fixtures::load_sales(&bootstrap, spec)?;
        tracing::info!(
            ?outcome,
            seed = spec.seed,
            rows = spec.rows,
            "development fixture"
        );
        if matches!(outcome, LoadOutcome::Loaded | LoadOutcome::AlreadyLoaded) {
            provenance = Provenance::SyntheticSales(spec);
        }
    }
    Ok(Instance {
        runtime: bootstrap.finish()?,
        provenance,
    })
}
