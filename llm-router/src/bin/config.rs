//! Application-owned classifier env parsing, shared by binary and evaluation.
use nasiko_llm_router::routing::decision::{ClassifierRuntime, HostedClassifier, LocalClassifier};
use std::{env, path::PathBuf, sync::Arc, time::Duration};

pub struct ClassifierConfig {
    pub runtime: Arc<ClassifierRuntime>,
    pub load_timeout: Duration,
}
impl ClassifierConfig {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let backend = env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".into());
        let load_timeout = Duration::from_millis(number("CLASSIFIER_LOAD_TIMEOUT_MS", 180_000u64)?);
        if load_timeout.is_zero() {
            return Err("CLASSIFIER_LOAD_TIMEOUT_MS must be positive".into());
        }
        if backend == "regex" {
            return Ok(Self {
                runtime: Arc::new(ClassifierRuntime::default()),
                load_timeout,
            });
        }
        let timeout_ms = number("CLASSIFIER_TIMEOUT_MS", 2000u64)?;
        let confidence = number("CLASSIFIER_MIN_CONFIDENCE", 0.35f32)?;
        let temperature = number("CLASSIFIER_TEMPERATURE", 1.0f64)?;
        let seed = number("CLASSIFIER_SEED", 42u64)?;
        if timeout_ms == 0
            || !confidence.is_finite()
            || !(0.0..=1.0).contains(&confidence)
            || !temperature.is_finite()
            || temperature <= 0.0
        {
            return Err("invalid classifier timeout, confidence, or temperature".into());
        }
        let classifier: Arc<dyn nasiko_llm_router::routing::classifier::RequestClassifier> =
            match backend.as_str() {
                "local" => {
                    let threads = number("CLASSIFIER_THREADS", 2usize)?;
                    if threads == 0 {
                        return Err("CLASSIFIER_THREADS must be positive".into());
                    }
                    Arc::new(LocalClassifier::new(
                        env::var_os("CLASSIFIER_PYTHON")
                            .map(PathBuf::from)
                            .unwrap_or_else(|| "python3".into()),
                        env::var_os("CLASSIFIER_WORKER")
                            .map(PathBuf::from)
                            .unwrap_or_else(|| {
                                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                                    .join("classifier/model_worker.py")
                            }),
                        PathBuf::from(env::var("CLASSIFIER_MODEL_PATH")?),
                        temperature,
                        threads,
                    ))
                }
                "hosted" => {
                    let endpoint = env::var("CLASSIFIER_ENDPOINT")?;
                    let url = reqwest::Url::parse(&endpoint)?;
                    if !matches!(url.scheme(), "http" | "https")
                        || !url.username().is_empty()
                        || url.password().is_some()
                    {
                        return Err("CLASSIFIER_ENDPOINT must be an HTTP(S) URL without embedded credentials".into());
                    }
                    Arc::new(HostedClassifier {
                        client: reqwest::Client::builder()
                            .redirect(reqwest::redirect::Policy::none())
                            .build()?,
                        endpoint,
                        api_key: env::var("CLASSIFIER_API_KEY").unwrap_or_default(),
                        model: env::var("CLASSIFIER_MODEL").unwrap_or_default(),
                        temperature,
                    })
                }
                _ => return Err("CLASSIFIER_BACKEND must be regex, local, or hosted".into()),
            };
        Ok(Self {
            runtime: Arc::new(ClassifierRuntime::new(
                classifier,
                Duration::from_millis(timeout_ms),
                confidence,
                seed,
                true,
            )),
            load_timeout,
        })
    }
}
fn number<T: std::str::FromStr>(name: &str, default: T) -> Result<T, Box<dyn std::error::Error>> {
    match env::var(name) {
        Ok(value) => value.parse().map_err(|_| format!("invalid {name}").into()),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("invalid {name}").into()),
    }
}
