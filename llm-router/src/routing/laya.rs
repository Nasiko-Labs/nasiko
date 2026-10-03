//! Laya local backend for the request classifier — in-process ONNX inference, no network.
//!
//! Laya (Convai Innovations, Apache-2.0) is a non-autoregressive "System 1" decision model
//! that answers the same typed questions Jev does — a Choice over options and a Score over an
//! ordered scale — in one encoder forward pass. This adapter runs the published ONNX export
//! (`receptron/laya-onnx`: ModernBERT-large encoder + decision head, fp32, ~1.7 GB) through
//! ONNX Runtime, loaded dynamically, with the checkpoint's own tokenizer. It sends exactly the
//! rubric [`super::rubric`] that the Jev adapter sends, so the two are comparable.
//!
//! Verified against the reference implementations before this was written (October 2026):
//! the sequence layout and temperature handling are a port of the Python package
//! (`laya` 0.3.24, `common.build_sequence` / `agent._decode_answers`) and of the TypeScript
//! port (`@receptron/laya` 0.1.2, `sequence.ts`); the ONNX bundle reproduces the pinned
//! PyTorch checkpoint (`convaiinnovations/laya@55cf4c4`) to within 5e-5 on every probability
//! on the public sample. See `docs/classifier/LAYA.md` for the setup and the measurements.
//!
//! **Sequence** (one row per question, batched into one run):
//! `[CLS] <type> question: <instructions> [SEP] [MASK] opt0 [MASK] opt1 … [SEP] <state> [SEP]`,
//! where the state is `{"query": …, "context": …}` serialized like Python's `json.dumps`.
//! Each option's `[MASK]` position is a *marker*; the model emits one logit per marker.
//! The literal `[MASK]` text is scrubbed from user content so it cannot inject a marker.
//!
//! **Budgets.** Options are capped at 48 tokens each and the whole head at `head_max_len`
//! (192) tokens; the state gets what is left of `max_len` (512) and is cut at the end. With
//! this rubric the head costs ~300 tokens for the type question, so roughly 200 tokens of
//! query+context survive — much less than Jev's 32k. That is a real limitation of this
//! backend for long contexts and is reported per call as `input_truncated` (state truncated).
//!
//! **Probabilities.** Per-cardinality temperature from `laya_config.json` (clamped to
//! `[0.5, 5]` exactly as the Python package does; the shipped `choice:11+` entry is out of
//! range and irrelevant for 7 options), then softmax. Public confidence is the probability
//! of the chosen type (Laya's own `answer_confidence`, the quantity its calibration targets);
//! the entropy-based `confidence` the vendor also reports is kept as `vendor_*_confidence`.
//! Complexity is the mode of the Score distribution (ties low), the same frozen rule as Jev.
//!
//! **Concurrency.** The session runs on the blocking pool under a semaphore; a permit is
//! acquired *inside* the deadline before any work is queued, so a flood of timed-out callers
//! can never leave an unbounded backlog of inference jobs. A job that outlives its caller's
//! deadline still finishes (ONNX Runtime cannot be interrupted) but holds its permit until
//! it does, which is what bounds the backlog.
//!
//! **Initialization** loads the tokenizer, config and graph once (seconds, ~2 GB RSS) and is
//! only attempted when `CLASSIFIER_BACKEND=laya`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Semaphore;

use super::classifier::{
    BackendDiagnostics, Classification, Classified, ClassifyError, ClassifyInput,
    RequestClassifier, RequestType,
};
use super::jev::complexity_level_from_probabilities;
use super::rubric::{
    COMPLEXITY_CRITERIA, COMPLEXITY_INSTRUCTIONS, TYPE_INSTRUCTIONS, type_criteria,
};
use crate::config::ClassifierConfig;

/// Per-option token cap before the head budget is applied (reference: `[:48]`).
const OPTION_TOKEN_CAP: usize = 48;
/// Temperature clamp, as the Python reference applies it (`clamp_temperature`).
const TEMP_MIN: f32 = 0.5;
const TEMP_MAX: f32 = 5.0;
const MASK_TEXT: &str = "[MASK]";

/// Question kinds as the model encodes them (`qtype` input and temperature index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QType {
    Choice = 0,
    Score = 1,
}

impl QType {
    fn name(self) -> &'static str {
        match self {
            QType::Choice => "choice",
            QType::Score => "score",
        }
    }
}

/// `laya_config.json` as the export writes it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LayaBundleConfig {
    pub max_len: usize,
    pub head_max_len: usize,
    /// Per-qtype fallback temperature (`[choice, score, noul]`).
    pub temperature: Vec<f32>,
    /// Per-cardinality temperature, e.g. `"choice:6-10"`.
    #[serde(default)]
    pub temperature_by_options: std::collections::BTreeMap<String, f32>,
}

impl LayaBundleConfig {
    /// The temperature for a question with `k` options — reference `temp_bucket` plus clamp.
    pub fn temperature_for(&self, q: QType, k: usize) -> f32 {
        let size = if k <= 2 {
            "2"
        } else if k <= 5 {
            "3-5"
        } else if k <= 10 {
            "6-10"
        } else {
            "11+"
        };
        let t = self
            .temperature_by_options
            .get(&format!("{}:{size}", q.name()))
            .copied()
            .or_else(|| self.temperature.get(q as usize).copied())
            .unwrap_or(1.0);
        if !t.is_finite() {
            return 1.0;
        }
        t.clamp(TEMP_MIN, TEMP_MAX)
    }
}

/// Optional provenance written by `scripts/laya-setup.sh` next to the graph.
#[derive(Debug, Clone, Deserialize, Default)]
struct BundleManifest {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    base_model: String,
}

/// Special token ids the sequence builder needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialIds {
    pub cls: u32,
    pub sep: u32,
    pub mask: u32,
    pub pad: u32,
}

/// One question as the builder sees it: rendered option texts in label order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqQuestion {
    pub qtype: QType,
    pub instructions: String,
    pub options: Vec<String>,
}

/// Rendered options, reference `render_options`: `key: description` for choice,
/// `level i: criterion` for score.
pub fn type_question() -> SeqQuestion {
    SeqQuestion {
        qtype: QType::Choice,
        instructions: TYPE_INSTRUCTIONS.to_string(),
        options: type_criteria()
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect(),
    }
}

pub fn complexity_question() -> SeqQuestion {
    SeqQuestion {
        qtype: QType::Score,
        instructions: COMPLEXITY_INSTRUCTIONS.to_string(),
        options: COMPLEXITY_CRITERIA
            .iter()
            .enumerate()
            .map(|(i, c)| format!("level {i}: {c}"))
            .collect(),
    }
}

/// The state as Python's `json.dumps({"query": q, "context": c}, ensure_ascii=False)` renders
/// it (`", "` / `": "` separators, insertion order, non-ASCII kept). The reference serializes
/// a dict state this way; matching it byte for byte keeps tokenization identical.
pub fn serialize_state(query: &str, context: Option<&str>) -> String {
    let mut s = String::from("{\"query\": ");
    s.push_str(&serde_json::to_string(query).unwrap_or_default());
    if let Some(c) = context {
        s.push_str(", \"context\": ");
        s.push_str(&serde_json::to_string(c).unwrap_or_default());
    }
    s.push('}');
    s
}

/// A built sequence: token ids, marker positions, and how many state tokens were dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltSequence {
    pub ids: Vec<u32>,
    pub markers: Vec<usize>,
    pub state_tokens: usize,
    pub state_tokens_used: usize,
}

impl BuiltSequence {
    pub fn truncated(&self) -> bool {
        self.state_tokens_used < self.state_tokens
    }
}

/// Reference `build_sequence`, model-free so it is unit-tested with a fake encoder.
/// `encode` must tokenize *without* special tokens.
pub fn build_sequence(
    encode: &dyn Fn(&str) -> Vec<u32>,
    ids: SpecialIds,
    state: &str,
    q: &SeqQuestion,
    max_len: usize,
    head_max_len: usize,
) -> BuiltSequence {
    let scrub = |s: &str| s.replace(MASK_TEXT, " ");
    let mut head = encode(&format!(
        "{} question: {}",
        q.qtype.name(),
        scrub(&q.instructions)
    ));
    let mut opts: Vec<Vec<u32>> = q
        .options
        .iter()
        .map(|o| {
            let mut v = vec![ids.mask];
            let enc = encode(&format!(" {}", scrub(o)));
            v.extend(enc.into_iter().take(OPTION_TOKEN_CAP));
            v
        })
        .collect();
    let total = |xs: &[Vec<u32>]| xs.iter().map(Vec::len).sum::<usize>();
    let mut opt_budget = head_max_len as i64 - total(&opts) as i64;
    if opt_budget < 16 {
        let per = ((head_max_len.saturating_sub(16)) / opts.len().max(1)).max(4);
        for o in &mut opts {
            o.truncate(per);
        }
        opt_budget = head_max_len as i64 - total(&opts) as i64;
    }
    head.truncate(opt_budget.max(8) as usize);
    let mut seq = Vec::with_capacity(max_len);
    seq.push(ids.cls);
    seq.extend(head);
    seq.push(ids.sep);
    let mut markers = Vec::with_capacity(opts.len());
    for o in &opts {
        markers.push(seq.len());
        seq.extend(o);
    }
    seq.push(ids.sep);
    let room = max_len.saturating_sub(seq.len() + 1);
    let state_ids = encode(&scrub(state));
    let state_tokens = state_ids.len();
    let used = state_tokens.min(room);
    seq.extend(state_ids.into_iter().take(used));
    seq.push(ids.sep);
    seq.truncate(max_len);
    markers.retain(|m| *m < max_len);
    BuiltSequence {
        ids: seq,
        markers,
        state_tokens,
        state_tokens_used: used,
    }
}

/// Softmax over `logits / temperature`.
pub fn probabilities(logits: &[f32], temperature: f32) -> Vec<f32> {
    let z: Vec<f32> = logits.iter().map(|v| v / temperature).collect();
    let max = z.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = z.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = e.iter().sum();
    if sum <= 0.0 || !sum.is_finite() {
        return vec![1.0 / logits.len().max(1) as f32; logits.len()];
    }
    e.iter().map(|v| v / sum).collect()
}

/// Laya's entropy confidence: `1 - H(p) / ln k` (reference `confidence_from_probs`).
pub fn entropy_confidence(p: &[f32]) -> f32 {
    let k = p.len();
    if k < 2 {
        return 1.0;
    }
    let ent: f32 = p.iter().map(|x| -x * x.max(1e-12).ln()).sum();
    (1.0 - ent / (k as f32).ln()).clamp(0.0, 1.0)
}

/// The decoded answers of one run, before they become a [`Classification`].
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub type_probabilities: Vec<f32>,
    pub complexity_probabilities: Vec<f32>,
}

/// Validate and decode the model's `logits` ([2, K]) for the type and complexity rows.
pub fn decode_logits(
    logits: &[f32],
    k_stride: usize,
    type_k: usize,
    cx_k: usize,
    cfg: &LayaBundleConfig,
) -> Result<Decoded, ClassifyError> {
    if logits.len() < 2 * k_stride || k_stride < type_k.max(cx_k) {
        return Err(ClassifyError::InvalidResponse(format!(
            "logits shape {} does not cover 2×{k_stride} markers",
            logits.len()
        )));
    }
    let row = |r: usize, k: usize| &logits[r * k_stride..r * k_stride + k];
    for v in row(0, type_k).iter().chain(row(1, cx_k)) {
        if !v.is_finite() {
            return Err(ClassifyError::InvalidResponse("non-finite logit".into()));
        }
    }
    Ok(Decoded {
        type_probabilities: probabilities(
            row(0, type_k),
            cfg.temperature_for(QType::Choice, type_k),
        ),
        complexity_probabilities: probabilities(
            row(1, cx_k),
            cfg.temperature_for(QType::Score, cx_k),
        ),
    })
}

/// Turn decoded distributions into the shared contract. Argmax ties go to the first option.
pub fn classification_from(d: &Decoded, model_version: &str, input_tokens: u64) -> Classified {
    let mut best = 0usize;
    for (i, p) in d.type_probabilities.iter().enumerate() {
        if *p > d.type_probabilities[best] {
            best = i;
        }
    }
    let request_type = RequestType::ALL[best.min(RequestType::ALL.len() - 1)];
    let complexity = complexity_level_from_probabilities(&d.complexity_probabilities);
    let expected = d
        .complexity_probabilities
        .iter()
        .enumerate()
        .map(|(i, p)| (i as f32 + 1.0) * p)
        .sum::<f32>();
    Classified {
        classification: Classification {
            request_type,
            complexity,
            confidence: d.type_probabilities[best].clamp(0.0, 1.0),
        },
        diagnostics: Some(BackendDiagnostics {
            model_version: Some(model_version.to_string()),
            type_probabilities: RequestType::ALL
                .iter()
                .copied()
                .zip(d.type_probabilities.iter().copied())
                .collect(),
            complexity_probabilities: d.complexity_probabilities.clone(),
            complexity_expected: Some(expected),
            vendor_type_confidence: Some(entropy_confidence(&d.type_probabilities)),
            vendor_complexity_confidence: Some(entropy_confidence(&d.complexity_probabilities)),
            input_tokens: Some(input_tokens),
            output_tokens: Some(0),
            attempts: 1,
            input_truncated: false,
        }),
    }
}

/// The collated batch for one classification: two rows (type, complexity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub rows: usize,
    pub len: usize,
    pub k: usize,
    pub input_ids: Vec<i64>,
    pub attention_mask: Vec<i64>,
    pub marker_pos: Vec<i64>,
    pub marker_mask: Vec<bool>,
    pub qtype: Vec<i64>,
    pub input_tokens: u64,
    pub truncated: bool,
}

/// Reference `collate_items`: right-pad to the longest row and the widest option set.
pub fn collate(seqs: &[(QType, BuiltSequence)], pad: u32) -> Batch {
    let rows = seqs.len();
    let len = seqs.iter().map(|(_, s)| s.ids.len()).max().unwrap_or(0);
    let k = seqs.iter().map(|(_, s)| s.markers.len()).max().unwrap_or(0);
    let mut b = Batch {
        rows,
        len,
        k,
        input_ids: vec![pad as i64; rows * len],
        attention_mask: vec![0; rows * len],
        marker_pos: vec![0; rows * k],
        marker_mask: vec![false; rows * k],
        qtype: vec![0; rows],
        input_tokens: 0,
        truncated: false,
    };
    for (r, (q, s)) in seqs.iter().enumerate() {
        for (j, id) in s.ids.iter().enumerate() {
            b.input_ids[r * len + j] = *id as i64;
            b.attention_mask[r * len + j] = 1;
        }
        for (j, m) in s.markers.iter().enumerate() {
            b.marker_pos[r * k + j] = *m as i64;
            b.marker_mask[r * k + j] = true;
        }
        b.qtype[r] = *q as i64;
        b.input_tokens += s.ids.len() as u64;
        b.truncated |= s.truncated();
    }
    b
}

/// Static facts about a loaded bundle, for status displays.
#[derive(Debug, Clone, PartialEq)]
pub struct BundleInfo {
    pub model_dir: PathBuf,
    pub model_version: String,
    pub graph_bytes: u64,
    pub weights_bytes: u64,
    pub max_len: usize,
    pub head_max_len: usize,
}

/// Everything the blocking inference job needs, shared by `Arc` so a job that outlives its
/// caller's deadline still owns what it runs on.
struct Inner {
    session: Mutex<ort::session::Session>,
    tokenizer: tokenizers::Tokenizer,
    ids: SpecialIds,
    cfg: LayaBundleConfig,
    info: BundleInfo,
}

/// The ONNX session plus everything needed to build its inputs. Build once; share.
pub struct LayaClassifier {
    inner: Arc<Inner>,
    deadline: Duration,
    gate: Arc<Semaphore>,
    load_time: Duration,
}

impl std::fmt::Debug for LayaClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayaClassifier")
            .field("model_dir", &self.inner.info.model_dir)
            .field("model_version", &self.inner.info.model_version)
            .field("deadline", &self.deadline)
            .finish()
    }
}

static ORT_READY: OnceLock<Result<(), String>> = OnceLock::new();

/// Load ONNX Runtime once per process: from the configured library path, else let `ort`
/// resolve it (`ORT_DYLIB_PATH` or the platform's default library name).
fn init_ort(dylib: &str) -> Result<(), ClassifyError> {
    let r = ORT_READY.get_or_init(|| {
        let builder = if dylib.trim().is_empty() {
            ort::init()
        } else {
            ort::init_from(dylib.trim())
                .map_err(|e| format!("cannot load ONNX Runtime from '{dylib}': {e}"))?
        };
        let _ = builder.with_name("nasiko-llm-router").commit();
        Ok(())
    });
    r.clone().map_err(ClassifyError::Init)
}

impl LayaClassifier {
    /// Validate the bundle directory and load everything. Fails (an init error the service
    /// counts) on a missing or corrupt bundle, an unloadable runtime or a graph the runtime
    /// rejects. No network I/O.
    pub fn new(cfg: &ClassifierConfig) -> Result<Self, ClassifyError> {
        let started = Instant::now();
        let dir = Path::new(cfg.model_path.trim());
        if cfg.model_path.trim().is_empty() {
            return Err(ClassifyError::Init(
                "CLASSIFIER_MODEL_PATH is not set (required for CLASSIFIER_BACKEND=laya; see llm-router/scripts/laya-setup.sh)".into(),
            ));
        }
        let need = |name: &str| -> Result<PathBuf, ClassifyError> {
            let p = dir.join(name);
            if !p.is_file() {
                return Err(ClassifyError::Init(format!(
                    "Laya bundle is incomplete: '{}' is missing (run llm-router/scripts/laya-setup.sh)",
                    p.display()
                )));
            }
            Ok(p)
        };
        let graph = need("laya.onnx")?;
        let weights = need("laya.onnx.data")?;
        let config_path = need("laya_config.json")?;
        let tok_path = need("tokenizer/tokenizer.json")?;

        let bundle: LayaBundleConfig = std::fs::read_to_string(&config_path)
            .map_err(|e| ClassifyError::Init(format!("laya_config.json: {e}")))
            .and_then(|s| {
                serde_json::from_str(&s)
                    .map_err(|e| ClassifyError::Init(format!("laya_config.json is not valid: {e}")))
            })?;
        if bundle.max_len < 64 || bundle.head_max_len < 32 || bundle.head_max_len >= bundle.max_len
        {
            return Err(ClassifyError::Init(format!(
                "laya_config.json has unusable limits (max_len {}, head_max_len {})",
                bundle.max_len, bundle.head_max_len
            )));
        }
        let tokenizer = tokenizers::Tokenizer::from_file(&tok_path)
            .map_err(|e| ClassifyError::Init(format!("tokenizer/tokenizer.json: {e}")))?;
        let special = |t: &str| {
            tokenizer
                .token_to_id(t)
                .ok_or_else(|| ClassifyError::Init(format!("tokenizer lacks the {t} token")))
        };
        let ids = SpecialIds {
            cls: special("[CLS]")?,
            sep: special("[SEP]")?,
            mask: special("[MASK]")?,
            pad: special("[PAD]")?,
        };

        init_ort(&cfg.ort_dylib)?;
        let mut builder = ort::session::Session::builder()
            .map_err(|e| ClassifyError::Init(format!("onnxruntime session builder: {e}")))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| ClassifyError::Init(format!("onnxruntime options: {e}")))?;
        if cfg.threads > 0 {
            builder = builder
                .with_intra_threads(cfg.threads)
                .map_err(|e| ClassifyError::Init(format!("onnxruntime threads: {e}")))?;
        }
        let session = builder
            .commit_from_file(&graph)
            .map_err(|e| ClassifyError::Init(format!("cannot load '{}': {e}", graph.display())))?;

        let manifest: BundleManifest = std::fs::read_to_string(dir.join("BUNDLE.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let graph_bytes = std::fs::metadata(&graph).map(|m| m.len()).unwrap_or(0);
        let weights_bytes = std::fs::metadata(&weights).map(|m| m.len()).unwrap_or(0);
        let model_version = if manifest.repo.is_empty() {
            format!("laya-onnx (unrecorded provenance; {} bytes)", weights_bytes)
        } else {
            format!(
                "{}@{} (export of {})",
                manifest.repo,
                manifest.revision.chars().take(7).collect::<String>(),
                manifest.base_model
            )
        };
        let info = BundleInfo {
            model_dir: dir.to_path_buf(),
            model_version,
            graph_bytes,
            weights_bytes,
            max_len: bundle.max_len,
            head_max_len: bundle.head_max_len,
        };
        tracing::info!(
            target: "nasiko::llm_router::classifier",
            model_dir = %dir.display(), model_version = %info.model_version,
            weights_mb = weights_bytes / 1_000_000, load_ms = started.elapsed().as_millis() as u64,
            "laya: ONNX bundle loaded"
        );
        Ok(Self {
            inner: Arc::new(Inner {
                session: Mutex::new(session),
                tokenizer,
                ids,
                cfg: bundle,
                info,
            }),
            deadline: Duration::from_millis(cfg.timeout_ms.max(1)),
            gate: Arc::new(Semaphore::new(cfg.max_concurrency.max(1))),
            load_time: started.elapsed(),
        })
    }

    pub fn info(&self) -> &BundleInfo {
        &self.inner.info
    }

    pub fn load_time(&self) -> Duration {
        self.load_time
    }

    /// Build the two-row batch for `input` (model-free apart from tokenization).
    pub fn batch_for(&self, input: &ClassifyInput<'_>) -> Result<Batch, ClassifyError> {
        self.inner.batch_for(input)
    }
}

impl Inner {
    fn encode(&self, text: &str) -> Vec<u32> {
        self.tokenizer
            .encode_fast(text, false)
            .map(|e| e.get_ids().to_vec())
            .unwrap_or_default()
    }

    fn batch_for(&self, input: &ClassifyInput<'_>) -> Result<Batch, ClassifyError> {
        let state = serialize_state(input.query, input.context);
        let enc = |t: &str| self.encode(t);
        let tq = type_question();
        let cq = complexity_question();
        let ts = build_sequence(
            &enc,
            self.ids,
            &state,
            &tq,
            self.cfg.max_len,
            self.cfg.head_max_len,
        );
        let cs = build_sequence(
            &enc,
            self.ids,
            &state,
            &cq,
            self.cfg.max_len,
            self.cfg.head_max_len,
        );
        if ts.markers.len() != tq.options.len() || cs.markers.len() != cq.options.len() {
            return Err(ClassifyError::InvalidResponse(
                "rubric options do not fit the bundle's head_max_len".into(),
            ));
        }
        Ok(collate(
            &[(QType::Choice, ts), (QType::Score, cs)],
            self.ids.pad,
        ))
    }

    /// Run the graph on the current thread (blocking). Returns the raw `logits` row-major.
    fn run_blocking(&self, b: &Batch) -> Result<Vec<f32>, ClassifyError> {
        use ort::value::Tensor;
        let t = |e: ort::Error| ClassifyError::InvalidResponse(format!("onnxruntime: {e}"));
        let input_ids = Tensor::from_array(([b.rows, b.len], b.input_ids.clone())).map_err(t)?;
        let attention =
            Tensor::from_array(([b.rows, b.len], b.attention_mask.clone())).map_err(t)?;
        let marker_pos = Tensor::from_array(([b.rows, b.k], b.marker_pos.clone())).map_err(t)?;
        let marker_mask = Tensor::from_array(([b.rows, b.k], b.marker_mask.clone())).map_err(t)?;
        let qtype = Tensor::from_array(([b.rows], b.qtype.clone())).map_err(t)?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| ClassifyError::InvalidResponse("onnxruntime session poisoned".into()))?;
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention,
                "marker_pos" => marker_pos,
                "marker_mask" => marker_mask,
                "qtype" => qtype,
            ])
            .map_err(t)?;
        let logits = outputs.get("logits").ok_or_else(|| {
            ClassifyError::InvalidResponse("model returned no `logits` output".into())
        })?;
        let (shape, data) = logits.try_extract_tensor::<f32>().map_err(t)?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims.len() != 2 || dims[0] as usize != b.rows || dims[1] as usize != b.k {
            return Err(ClassifyError::InvalidResponse(format!(
                "logits shape {dims:?}, expected [{}, {}]",
                b.rows, b.k
            )));
        }
        Ok(data.to_vec())
    }
}

#[async_trait]
impl RequestClassifier for LayaClassifier {
    fn name(&self) -> &str {
        "laya"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.classify_detailed(input)
            .await
            .map(|c| c.classification)
    }

    async fn classify_detailed(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classified, ClassifyError> {
        let started = Instant::now();
        let deadline = started + self.deadline;
        // Bound the backlog: no permit, no queued job.
        let permit = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            self.gate.clone().acquire_owned(),
        )
        .await
        .map_err(|_| ClassifyError::Timeout)?
        .map_err(|_| ClassifyError::Init("laya semaphore closed".into()))?;

        let batch = self.inner.batch_for(input)?;
        // The job owns an Arc to the session and the permit, so it can outlive this call and
        // still release the permit when it is done.
        let inner = self.inner.clone();
        let b = batch.clone();
        let job = tokio::task::spawn_blocking(move || {
            let r = inner.run_blocking(&b);
            drop(permit);
            r
        });
        let remaining = deadline.saturating_duration_since(Instant::now());
        let logits = match tokio::time::timeout(remaining, job).await {
            Ok(Ok(r)) => r?,
            Ok(Err(join)) => {
                return Err(ClassifyError::InvalidResponse(format!(
                    "inference task failed: {join}"
                )));
            }
            Err(_) => return Err(ClassifyError::Timeout),
        };
        let decoded = decode_logits(
            &logits,
            batch.k,
            RequestType::ALL.len(),
            COMPLEXITY_CRITERIA.len(),
            &self.inner.cfg,
        )?;
        let mut out =
            classification_from(&decoded, &self.inner.info.model_version, batch.input_tokens);
        if let Some(d) = out.diagnostics.as_mut() {
            // The state did not fit the model's 512-token window: reported so a caller can see
            // that the verdict rests on a prefix of the request.
            d.input_truncated = batch.truncated;
        }
        out.classification.validate()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LayaBundleConfig {
        LayaBundleConfig {
            max_len: 512,
            head_max_len: 192,
            temperature: vec![1.6369, 1.2514, 1.9834],
            temperature_by_options: [
                ("choice:3-5", 1.7601),
                ("choice:6-10", 1.0000158),
                ("score:3-5", 1.2514),
                ("noul:2", 1.9834),
                ("choice:11+", 0.1006),
                ("choice:2", 1.9064),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        }
    }

    /// A fake tokenizer: one id per whitespace-separated word, stable across calls, never the
    /// special ids (which are 1..=4).
    fn fake_encode(text: &str) -> Vec<u32> {
        text.split_whitespace()
            .map(|w| {
                let mut h: u32 = 7;
                for b in w.bytes() {
                    h = h.wrapping_mul(31).wrapping_add(b as u32);
                }
                100 + h % 50_000
            })
            .collect()
    }
    const IDS: SpecialIds = SpecialIds {
        cls: 1,
        sep: 2,
        mask: 3,
        pad: 4,
    };

    #[test]
    fn temperature_buckets_and_clamp_match_the_reference() {
        let c = cfg();
        assert!((c.temperature_for(QType::Choice, 7) - 1.0000158).abs() < 1e-6);
        assert!((c.temperature_for(QType::Score, 5) - 1.2514).abs() < 1e-6);
        assert!((c.temperature_for(QType::Choice, 2) - 1.9064).abs() < 1e-6);
        // The shipped out-of-range entry is clamped, not used raw.
        assert_eq!(c.temperature_for(QType::Choice, 12), TEMP_MIN);
        // Missing bucket falls back to the per-qtype value.
        let mut c2 = c.clone();
        c2.temperature_by_options.clear();
        assert!((c2.temperature_for(QType::Choice, 7) - 1.6369).abs() < 1e-6);
        c2.temperature.clear();
        assert_eq!(c2.temperature_for(QType::Score, 5), 1.0);
    }

    #[test]
    fn state_serialization_matches_python_json_dumps() {
        assert_eq!(serialize_state("hi", None), r#"{"query": "hi"}"#);
        assert_eq!(
            serialize_state("a \"q\"\n", Some("ctx é 🚀")),
            "{\"query\": \"a \\\"q\\\"\\n\", \"context\": \"ctx é 🚀\"}"
        );
    }

    #[test]
    fn sequence_layout_follows_the_reference_template() {
        let q = SeqQuestion {
            qtype: QType::Choice,
            instructions: "Which one?".into(),
            options: vec!["a: first".into(), "b: second".into()],
        };
        let s = build_sequence(&fake_encode, IDS, "the state here", &q, 512, 192);
        // [CLS] head [SEP] [MASK] opt0 [MASK] opt1 [SEP] state [SEP]
        assert_eq!(s.ids[0], IDS.cls);
        let head_len = fake_encode("choice question: Which one?").len();
        assert_eq!(s.ids[1 + head_len], IDS.sep);
        assert_eq!(
            s.markers,
            vec![
                2 + head_len,
                2 + head_len + 1 + fake_encode(" a: first").len()
            ]
        );
        for m in &s.markers {
            assert_eq!(s.ids[*m], IDS.mask);
        }
        assert_eq!(*s.ids.last().unwrap(), IDS.sep);
        assert_eq!(s.state_tokens, 3);
        assert_eq!(s.state_tokens_used, 3);
        assert!(!s.truncated());
        // Deterministic.
        assert_eq!(
            s,
            build_sequence(&fake_encode, IDS, "the state here", &q, 512, 192)
        );
    }

    #[test]
    fn mask_text_is_scrubbed_from_user_content() {
        let q = SeqQuestion {
            qtype: QType::Score,
            instructions: "x [MASK] y".into(),
            options: vec!["level 0: lo [MASK]".into(), "level 1: hi".into()],
        };
        let s = build_sequence(&fake_encode, IDS, "state [MASK] text", &q, 512, 192);
        // Exactly one [MASK] per option — none smuggled in by content.
        assert_eq!(s.ids.iter().filter(|i| **i == IDS.mask).count(), 2);
    }

    #[test]
    fn state_is_truncated_to_the_remaining_room_and_reported() {
        let q = type_question();
        let long_state = (0..1000)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let s = build_sequence(&fake_encode, IDS, &long_state, &q, 512, 192);
        assert_eq!(s.ids.len(), 512);
        assert!(s.truncated());
        assert_eq!(s.markers.len(), 7);
        assert!(s.state_tokens_used < s.state_tokens);
        assert_eq!(*s.ids.last().unwrap(), IDS.sep);
    }

    #[test]
    fn oversized_options_are_shrunk_evenly_and_the_head_keeps_at_least_eight_tokens() {
        let big = (0..100)
            .map(|i| format!("o{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let q = SeqQuestion {
            qtype: QType::Choice,
            instructions: (0..300)
                .map(|i| format!("i{i}"))
                .collect::<Vec<_>>()
                .join(" "),
            options: (0..7).map(|_| big.clone()).collect(),
        };
        let s = build_sequence(&fake_encode, IDS, "s", &q, 512, 192);
        assert_eq!(s.markers.len(), 7);
        // Head (between CLS and first SEP) is at least 8 tokens, options were capped evenly.
        let first_sep = s.ids.iter().position(|i| *i == IDS.sep).unwrap();
        assert!(first_sep > 8);
        let per = (192 - 16) / 7;
        assert_eq!(s.markers[1] - s.markers[0], per);
    }

    #[test]
    fn collate_right_pads_rows_and_markers() {
        let a = BuiltSequence {
            ids: vec![1, 10, 2, 3, 11, 3, 12, 2, 20, 2],
            markers: vec![3, 5],
            state_tokens: 1,
            state_tokens_used: 1,
        };
        let b = BuiltSequence {
            ids: vec![1, 10, 2, 3, 11, 2, 20, 2],
            markers: vec![3],
            state_tokens: 2,
            state_tokens_used: 1,
        };
        let batch = collate(&[(QType::Choice, a), (QType::Score, b)], 4);
        assert_eq!((batch.rows, batch.len, batch.k), (2, 10, 2));
        assert_eq!(&batch.input_ids[10..], &[1, 10, 2, 3, 11, 2, 20, 2, 4, 4]);
        assert_eq!(&batch.attention_mask[10..], &[1, 1, 1, 1, 1, 1, 1, 1, 0, 0]);
        assert_eq!(batch.marker_pos, vec![3, 5, 3, 0]);
        assert_eq!(batch.marker_mask, vec![true, true, true, false]);
        assert_eq!(batch.qtype, vec![0, 1]);
        assert_eq!(batch.input_tokens, 18);
        assert!(batch.truncated);
    }

    #[test]
    fn decode_applies_temperature_softmax_and_rejects_bad_logits() {
        let c = cfg();
        // logits row-major [2, 7]; complexity uses the first 5 of row 1.
        let mut logits = vec![0.0f32; 14];
        logits[2] = 3.0; // technical_design
        logits[7 + 3] = 2.0; // level index 3 ⇒ complexity 4
        let d = decode_logits(&logits, 7, 7, 5, &c).unwrap();
        assert_eq!(d.type_probabilities.len(), 7);
        assert_eq!(d.complexity_probabilities.len(), 5);
        assert!((d.type_probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let c1 = classification_from(&d, "v", 42);
        assert_eq!(c1.classification.request_type, RequestType::TechnicalDesign);
        assert_eq!(c1.classification.complexity, 4);
        assert!((c1.classification.confidence - d.type_probabilities[2]).abs() < 1e-6);
        let diag = c1.diagnostics.unwrap();
        assert_eq!(diag.input_tokens, Some(42));
        assert_eq!(diag.model_version.as_deref(), Some("v"));
        assert!(diag.vendor_type_confidence.unwrap() > 0.0);
        // A higher temperature flattens the distribution.
        let hot = probabilities(&logits[..7], 5.0);
        assert!(hot[2] < d.type_probabilities[2]);
        // Shape and finiteness are validated.
        assert!(decode_logits(&logits[..10], 7, 7, 5, &c).is_err());
        logits[0] = f32::NAN;
        assert!(decode_logits(&logits, 7, 7, 5, &c).is_err());
    }

    #[test]
    fn entropy_confidence_matches_the_reference_definition() {
        assert_eq!(entropy_confidence(&[1.0]), 1.0);
        assert!((entropy_confidence(&[1.0, 0.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(entropy_confidence(&[0.25, 0.25, 0.25, 0.25]).abs() < 1e-6);
        let mid = entropy_confidence(&[0.7, 0.2, 0.1]);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn missing_or_corrupt_bundle_is_an_init_error_without_touching_the_runtime() {
        let base = ClassifierConfig {
            backend: crate::config::ClassifierBackend::Laya,
            ..Default::default()
        };
        let e = LayaClassifier::new(&base).unwrap_err();
        assert!(matches!(e, ClassifyError::Init(m) if m.contains("CLASSIFIER_MODEL_PATH")));

        let dir = std::env::temp_dir().join(format!("laya-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tokenizer")).unwrap();
        let c = ClassifierConfig {
            model_path: dir.to_string_lossy().into_owned(),
            ..base.clone()
        };
        let e = LayaClassifier::new(&c).unwrap_err();
        assert!(
            matches!(&e, ClassifyError::Init(m) if m.contains("laya.onnx") && m.contains("missing")),
            "{e:?}"
        );

        for f in ["laya.onnx", "laya.onnx.data", "tokenizer/tokenizer.json"] {
            std::fs::write(dir.join(f), b"not a real file").unwrap();
        }
        std::fs::write(dir.join("laya_config.json"), b"{not json").unwrap();
        let e = LayaClassifier::new(&c).unwrap_err();
        assert!(
            matches!(&e, ClassifyError::Init(m) if m.contains("laya_config.json")),
            "{e:?}"
        );

        std::fs::write(
            dir.join("laya_config.json"),
            br#"{"max_len":512,"head_max_len":192,"temperature":[1,1,1]}"#,
        )
        .unwrap();
        let e = LayaClassifier::new(&c).unwrap_err();
        assert!(
            matches!(&e, ClassifyError::Init(m) if m.contains("tokenizer")),
            "{e:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
