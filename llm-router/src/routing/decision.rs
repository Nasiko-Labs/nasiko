//! Opt-in semantic decisions. Pure configuration values enter here; no env reads.
//! Both router and evaluation use `ClassifierRuntime::run`, including fallbacks.

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RegexClassifier, RequestClassifier, RequestType,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

pub const LABELS: [&str; 7] = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
];
const MAX_RESPONSE: usize = 64 * 1024;

/// Shared versioned label definitions for local and hosted decision models.
pub fn questions() -> Value {
    serde_json::from_str(include_str!("../../assets/classifier_questions.json"))
        .expect("embedded classifier rubric")
}

pub fn request(input: &ClassifyInput<'_>) -> Value {
    json!({"state": {"query": input.query, "context": input.context.unwrap_or("")}, "questions": questions()})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    Startup,
    Timeout,
    Error,
    InvalidOutput,
    LowConfidence,
}
impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Timeout => "timeout",
            Self::Error => "error",
            Self::InvalidOutput => "invalid_output",
            Self::LowConfidence => "low_confidence",
        }
    }
}

pub struct Outcome {
    pub classification: Classification,
    pub fallback: Option<FallbackReason>,
}

pub struct ClassifierRuntime {
    pub classifier: Arc<dyn RequestClassifier>,
    timeout: Duration,
    min_confidence: f32,
    pub seed: u64,
    pub experimental: bool,
    startup_failed: AtomicBool,
    pub fallbacks: AtomicU64,
}

impl Default for ClassifierRuntime {
    fn default() -> Self {
        Self::new(
            Arc::new(RegexClassifier),
            Duration::from_secs(1),
            0.0,
            0,
            false,
        )
    }
}

impl ClassifierRuntime {
    pub fn new(
        classifier: Arc<dyn RequestClassifier>,
        timeout: Duration,
        min_confidence: f32,
        seed: u64,
        experimental: bool,
    ) -> Self {
        Self {
            classifier,
            timeout,
            min_confidence,
            seed,
            experimental,
            startup_failed: AtomicBool::new(false),
            fallbacks: AtomicU64::new(0),
        }
    }

    pub async fn prepare(&self, load_timeout: Duration) {
        let failed = !matches!(
            tokio::time::timeout(load_timeout, self.classifier.prepare()).await,
            Ok(Ok(()))
        );
        self.startup_failed.store(failed, Ordering::Relaxed);
        if failed {
            tracing::warn!(
                backend = self.classifier.name(),
                "classifier startup failed; regex fallback enabled"
            );
        }
    }

    pub async fn run(&self, input: &ClassifyInput<'_>) -> Outcome {
        let result = if self.startup_failed.load(Ordering::Relaxed) {
            Err(FallbackReason::Startup)
        } else {
            match tokio::time::timeout(self.timeout, self.classifier.classify(input)).await {
                Err(_) | Ok(Err(ClassifyError::Timeout)) => Err(FallbackReason::Timeout),
                Ok(Err(ClassifyError::InvalidOutput)) => Err(FallbackReason::InvalidOutput),
                Ok(Err(_)) => Err(FallbackReason::Error),
                Ok(Ok(value)) => match value.validate() {
                    Err(_) => Err(FallbackReason::InvalidOutput),
                    Ok(value) if self.experimental && value.confidence < self.min_confidence => {
                        Err(FallbackReason::LowConfidence)
                    }
                    Ok(value) => Ok(value),
                },
            }
        };
        match result {
            Ok(classification) => Outcome {
                classification,
                fallback: None,
            },
            Err(reason) => {
                self.fallbacks.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    backend = self.classifier.name(),
                    reason = reason.as_str(),
                    "classifier used regex fallback"
                );
                Outcome {
                    classification: RegexClassifier::predict(input.query),
                    fallback: Some(reason),
                }
            }
        }
    }
}

/// Map typed decision distributions into Nasiko's contract. A hosted `confidence`
/// field may mean concentration, not accuracy; use the selected class probability.
/// Temperature is fitted on validation only. T=1 means uncalibrated model output.
pub fn decode(value: &Value, temperature: f64) -> Result<Classification, ClassifyError> {
    let probs = value["answers"]["request_type"]["probabilities"]
        .as_object()
        .ok_or(ClassifyError::InvalidOutput)?;
    if probs.len() != LABELS.len() {
        return Err(ClassifyError::InvalidOutput);
    }
    let raw: Vec<f64> = LABELS
        .iter()
        .map(|label| {
            probs
                .get(*label)
                .and_then(Value::as_f64)
                .ok_or(ClassifyError::InvalidOutput)
        })
        .collect::<Result<_, _>>()?;
    let p = calibrated(&raw, temperature)?;
    let winner = argmax(&p);
    let raw_levels = &value["answers"]["complexity"]["probabilities"];
    let levels: Vec<f64> = if let Some(a) = raw_levels.as_array() {
        a.iter()
            .map(|v| v.as_f64().ok_or(ClassifyError::InvalidOutput))
            .collect::<Result<_, _>>()?
    } else if let Some(m) = raw_levels.as_object() {
        // Score levels are indexed 0..4, corresponding to our rubric's 1..5.
        if m.len() != 5 {
            return Err(ClassifyError::InvalidOutput);
        }
        (0..5)
            .map(|i| {
                m.get(&i.to_string())
                    .and_then(Value::as_f64)
                    .ok_or(ClassifyError::InvalidOutput)
            })
            .collect::<Result<_, _>>()?
    } else {
        return Err(ClassifyError::InvalidOutput);
    };
    if levels.len() != 5 {
        return Err(ClassifyError::InvalidOutput);
    }
    let levels = calibrated(&levels, 1.0)?;
    Classification {
        request_type: RequestType::from_wire(LABELS[winner]).ok_or(ClassifyError::InvalidOutput)?,
        complexity: (argmax(&levels) + 1) as u8,
        confidence: p[winner] as f32,
    }
    .validate()
}

fn argmax(p: &[f64]) -> usize {
    p.iter()
        .enumerate()
        .fold(0, |best, (i, x)| if *x > p[best] { i } else { best })
}

fn calibrated(p: &[f64], temperature: f64) -> Result<Vec<f64>, ClassifyError> {
    if !temperature.is_finite()
        || temperature <= 0.0
        || p.iter().any(|x| !x.is_finite() || !(0.0..=1.0).contains(x))
        || (p.iter().sum::<f64>() - 1.0).abs() > 0.01
    {
        return Err(ClassifyError::InvalidOutput);
    }
    let logits: Vec<_> = p.iter().map(|x| x.max(1e-12).ln() / temperature).collect();
    let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exp: Vec<_> = logits.iter().map(|x| (x - max).exp()).collect();
    let sum: f64 = exp.iter().sum();
    Ok(exp.iter().map(|x| x / sum).collect())
}

/// Explicit endpoint is the full System One POST URL, never derived from routed
/// provider credentials. Redirects are disabled by the application-owned client.
pub struct HostedClassifier {
    pub client: reqwest::Client,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f64,
}

#[async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut body = request(input);
        if !self.model.is_empty() {
            body["model"] = json!(self.model);
        }
        let mut req = self.client.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let mut response = req
            .send()
            .await
            .map_err(|_| ClassifyError::Unavailable)?
            .error_for_status()
            .map_err(|_| ClassifyError::Unavailable)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ClassifyError::Unavailable)?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(ClassifyError::InvalidOutput);
            }
            bytes.extend_from_slice(&chunk);
        }
        decode(
            &serde_json::from_slice::<Value>(&bytes).map_err(|_| ClassifyError::InvalidOutput)?,
            self.temperature,
        )
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Persistent offline subprocess. A cancelled exchange drops and kills its worker,
/// so a late response can never be mistaken for the next request's response.
pub struct LocalClassifier {
    pub python: PathBuf,
    pub script: PathBuf,
    pub model_path: PathBuf,
    pub temperature: f64,
    pub threads: usize,
    worker: Mutex<Option<Worker>>,
}

impl LocalClassifier {
    pub fn new(
        python: PathBuf,
        script: PathBuf,
        model_path: PathBuf,
        temperature: f64,
        threads: usize,
    ) -> Self {
        Self {
            python,
            script,
            model_path,
            temperature,
            threads,
            worker: Mutex::new(None),
        }
    }
    async fn start(&self) -> Result<Worker, ClassifyError> {
        if !self.model_path.is_dir() {
            return Err(ClassifyError::Unavailable);
        }
        let mut cmd = Command::new(&self.python);
        cmd.arg(&self.script)
            .arg("--model-path")
            .arg(&self.model_path)
            .arg("--threads")
            .arg(self.threads.to_string())
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("USE_TF", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|_| ClassifyError::Unavailable)?;
        let stdin = child.stdin.take().ok_or(ClassifyError::Unavailable)?;
        let stdout = BufReader::new(child.stdout.take().ok_or(ClassifyError::Unavailable)?);
        let mut worker = Worker {
            child,
            stdin,
            stdout,
        };
        let ready = read_message(&mut worker.stdout).await?;
        if ready["ready"] != true {
            return Err(ClassifyError::Unavailable);
        }
        Ok(worker)
    }
}

async fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Value, ClassifyError> {
    use tokio::io::AsyncReadExt;
    let mut limited = reader.take((MAX_RESPONSE + 1) as u64);
    let mut line = Vec::new();
    limited
        .read_until(b'\n', &mut line)
        .await
        .map_err(|_| ClassifyError::Unavailable)?;
    if line.is_empty() || line.len() > MAX_RESPONSE {
        return Err(ClassifyError::InvalidOutput);
    }
    serde_json::from_slice(&line).map_err(|_| ClassifyError::InvalidOutput)
}

#[async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }
    async fn prepare(&self) -> Result<(), ClassifyError> {
        let mut slot = self.worker.lock().await;
        if slot.is_none() {
            *slot = Some(self.start().await?);
        }
        Ok(())
    }
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut slot = self.worker.lock().await;
        let mut worker = match slot.take() {
            Some(w) => w,
            None => self.start().await?,
        };
        let mut body =
            serde_json::to_vec(&request(input)).map_err(|_| ClassifyError::InvalidOutput)?;
        body.push(b'\n');
        worker
            .stdin
            .write_all(&body)
            .await
            .map_err(|_| ClassifyError::Unavailable)?;
        worker
            .stdin
            .flush()
            .await
            .map_err(|_| ClassifyError::Unavailable)?;
        let value = read_message(&mut worker.stdout).await?;
        let result = decode(&value, self.temperature);
        if worker
            .child
            .try_wait()
            .map_err(|_| ClassifyError::Unavailable)?
            .is_none()
        {
            *slot = Some(worker);
        }
        result
    }
}

/// Stable FNV-1a seed mixing, unlike an implementation-dependent DefaultHasher.
/// Learned cells affect samples directly; complexity does not change persisted keys.
pub fn routing_seed(seed: u64, provider: &str, input: &ClassifyInput<'_>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64 ^ seed;
    for part in [provider, input.query, input.context.unwrap_or("")] {
        for byte in (part.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(part.as_bytes())
        {
            hash = (hash ^ *byte as u64).wrapping_mul(0x100000001b3);
        }
    }
    hash
}

/// Bounded recent conversation context; system/developer instructions and tools
/// are not sent to a classifier endpoint. Includes a wrapped history prefix.
pub fn conversation_context(messages: &[crate::ir::Message]) -> Option<String> {
    let latest = messages.iter().rposition(|m| m.role == "user")?;
    let mut parts = Vec::new();
    if let Some(text) = messages[latest].text() {
        if let Some((history, _)) = text.split_once("\n\nCurrent message: ") {
            parts.push(history.to_string());
        }
    }
    for message in messages[..latest]
        .iter()
        .rev()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .take(4)
    {
        if let Some(text) = message.text() {
            parts.push(format!("{}: {}", message.role, text));
        }
    }
    parts.reverse();
    let text = parts.join("\n");
    let tail: String = text
        .chars()
        .rev()
        .take(4096)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!tail.is_empty()).then_some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response() -> Value {
        json!({"answers": {
            "request_type": {"probabilities": {"code_generation": 0.7, "code_understanding": 0.1, "technical_design": 0.04, "analytical_reasoning": 0.04, "writing": 0.04, "factual_lookup": 0.04, "general": 0.04}, "confidence": 0.999},
            "complexity": {"probabilities": [0.8, 0.1, 0.05, 0.03, 0.02]}
        }})
    }

    #[test]
    fn uses_probabilities_and_ordinal_levels_not_vendor_confidence() {
        let result = decode(&response(), 1.0).unwrap();
        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert_eq!(result.complexity, 1);
        assert!((result.confidence - 0.7).abs() < 1e-6);
        assert!(decode(&response(), 2.0).unwrap().confidence < result.confidence);
    }

    #[test]
    fn rejects_missing_labels_bad_mass_and_invalid_temperature() {
        let mut value = response();
        value["answers"]["request_type"]["probabilities"]
            .as_object_mut()
            .unwrap()
            .remove("general");
        assert!(decode(&value, 1.0).is_err());
        let mut value = response();
        value["answers"]["request_type"]["probabilities"]["general"] = json!(-0.1);
        assert!(decode(&value, 1.0).is_err());
        assert!(decode(&response(), f64::NAN).is_err());
        assert!(decode(&response(), 0.0).is_err());
        let mut value = response();
        value["answers"]["complexity"]["probabilities"] = json!([0.2, 0.2]);
        assert!(decode(&value, 1.0).is_err());
    }

    struct Fake {
        mode: &'static str,
    }
    #[async_trait]
    impl RequestClassifier for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        async fn prepare(&self) -> Result<(), ClassifyError> {
            if self.mode == "startup" {
                Err(ClassifyError::Unavailable)
            } else {
                Ok(())
            }
        }
        async fn classify(
            &self,
            input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            match self.mode {
                "timeout" => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Err(ClassifyError::Timeout)
                }
                "error" => Err(ClassifyError::Unavailable),
                "invalid" => Ok(Classification {
                    request_type: RequestType::Writing,
                    complexity: 9,
                    confidence: f32::NAN,
                }),
                _ => Ok(Classification {
                    request_type: RequestType::Writing,
                    complexity: 2,
                    confidence: if input.context == Some("certain") {
                        0.9
                    } else {
                        0.2
                    },
                }),
            }
        }
    }

    #[tokio::test]
    async fn failures_and_uncertainty_use_exact_legacy_prediction_and_count() {
        for (mode, reason) in [
            ("startup", FallbackReason::Startup),
            ("timeout", FallbackReason::Timeout),
            ("error", FallbackReason::Error),
            ("invalid", FallbackReason::InvalidOutput),
            ("low", FallbackReason::LowConfidence),
        ] {
            let runtime = ClassifierRuntime::new(
                Arc::new(Fake { mode }),
                Duration::from_millis(5),
                0.6,
                42,
                true,
            );
            runtime.prepare(Duration::from_secs(1)).await;
            let result = runtime
                .run(&ClassifyInput {
                    query: "write a python script",
                    context: None,
                })
                .await;
            assert_eq!(result.fallback, Some(reason));
            assert_eq!(
                result.classification,
                RegexClassifier::predict("write a python script")
            );
            assert_eq!(runtime.fallbacks.load(Ordering::Relaxed), 1);
        }
    }

    #[tokio::test]
    async fn confident_result_keeps_context_and_default_regex_never_falls_back() {
        let runtime = ClassifierRuntime::new(
            Arc::new(Fake { mode: "ok" }),
            Duration::from_secs(1),
            0.6,
            42,
            true,
        );
        let input = ClassifyInput {
            query: "write a python script",
            context: Some("certain"),
        };
        let result = runtime.run(&input).await;
        assert_eq!(result.classification.request_type, RequestType::Writing);
        assert_eq!(result.fallback, None);
        let baseline = ClassifierRuntime::default().run(&input).await;
        assert_eq!(
            baseline.classification.request_type,
            RequestType::CodeGeneration
        );
        assert_eq!(baseline.fallback, None);
    }

    #[tokio::test]
    async fn hosted_sends_query_context_and_uses_same_decoder() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/systemone")
            .match_body(mockito::Matcher::PartialJson(
                json!({"state": {"query": "fix it", "context": "comment typo"}}),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response().to_string())
            .create_async()
            .await;
        let backend = HostedClassifier {
            client: reqwest::Client::new(),
            endpoint: format!("{}/v1/systemone", server.url()),
            api_key: String::new(),
            model: String::new(),
            temperature: 1.0,
        };
        let result = backend
            .classify(&ClassifyInput {
                query: "fix it",
                context: Some("comment typo"),
            })
            .await
            .unwrap();
        assert_eq!(result.complexity, 1);
        mock.assert_async().await;
    }

    #[test]
    fn context_excludes_system_and_current_query_preserves_recent_history() {
        let messages: Vec<crate::ir::Message> = serde_json::from_value(json!([
            {"role": "system", "content": "secret instructions"},
            {"role": "user", "content": "A code comment has a typo."},
            {"role": "assistant", "content": "Should I correct it?"},
            {"role": "user", "content": "yes"}
        ]))
        .unwrap();
        let text = conversation_context(&messages).unwrap();
        assert!(text.contains("typo"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("user: yes"));
    }
}
