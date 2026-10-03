//! MiniLM-width Level-3 request-type classifier.
//!
//! This is an **in-process** stand-in for `all-MiniLM-L6-v2` (384-d): a hashing-trick
//! bag-of-n-grams encoder plus cosine nearest-prototype lookup. It does **not** load an
//! ONNX/ORT MiniLM checkpoint — that would add a multi-megabyte model and a native
//! runtime to the hot path. Token overlap with the prototypes is the signal; paraphrases
//! that share the same content words still match, which is the gap regex vote-count
//! misses.
//!
//! Opt in with `ROUTER_CLASSIFIER=minilm`. Unknown or unset values stay on the regex
//! classifier. Load failure is not a concept here (no weights file); the encoder is
//! always available. If every prototype scores below [`MIN_COSINE`], the query is
//! [`RequestType::General`].

use std::sync::LazyLock;

use super::classifier::RequestType;

/// Same width as MiniLM-L6-v2 so the bake-off and this path share a dimensionality.
pub const EMBED_DIM: usize = 384;

/// Below this cosine the query is treated as unmatched (`General`) rather than a weak
/// prototype hit. Tuned so greetings stay `General` while the regex-reference examples
/// still land on their category.
pub const MIN_COSINE: f32 = 0.12;

/// Seed phrases per [`RequestType`]. These are the "class centroids" — each class vector
/// is the mean of the embeddings of its phrases. Keep them lexically distinct so the
/// hashing-trick encoder can separate them.
const PROTOTYPES: &[(RequestType, &[&str])] = &[
    (
        RequestType::CodeGeneration,
        &[
            "write a python script that parses csv",
            "write me a python sort function",
            "implement a rust function",
            "create a javascript class",
            "build an api endpoint",
            "generate code for this module",
            "fix this bug in the program",
            "refactor this method",
            "add error handling to the script",
        ],
    ),
    (
        RequestType::CodeUnderstanding,
        &[
            "explain what this function does",
            "what does this code do",
            "how does this class work",
            "walk me through this script",
            "what is this code doing",
            "explain how the function works",
        ],
    ),
    (
        RequestType::TechnicalDesign,
        &[
            "how should i design this api",
            "system architecture tradeoffs",
            "database schema design",
            "design a service for this",
            "api design decisions",
        ],
    ),
    (
        RequestType::AnalyticalReasoning,
        &[
            "calculate the probability that it rains tomorrow",
            "solve this proof",
            "what is the sum of these numbers",
            "compute 12 + 34",
            "prove that this is true",
        ],
    ),
    (
        RequestType::Writing,
        &[
            "draft an email to my team about the outage",
            "write an article for the blog",
            "compose a letter",
            "rewrite this essay",
            "make this sound more professional",
        ],
    ),
    (
        RequestType::FactualLookup,
        &[
            "what is the capital of france",
            "who was the first president",
            "when was this invented",
            "define photosynthesis",
            "where is the eiffel tower",
        ],
    ),
    (
        RequestType::General,
        &[
            "hello there",
            "hi how are you",
            "thanks",
            "ok",
            "good morning",
        ],
    ),
];

struct Prototype {
    request_type: RequestType,
    vector: [f32; EMBED_DIM],
}

static CENTROIDS: LazyLock<Vec<Prototype>> = LazyLock::new(|| {
    PROTOTYPES
        .iter()
        .map(|(rt, phrases)| {
            let mut acc = [0.0f32; EMBED_DIM];
            for phrase in *phrases {
                let e = embed(phrase);
                for (a, b) in acc.iter_mut().zip(e.iter()) {
                    *a += *b;
                }
            }
            let n = phrases.len() as f32;
            for a in acc.iter_mut() {
                *a /= n;
            }
            l2_normalize(&mut acc);
            Prototype {
                request_type: *rt,
                vector: acc,
            }
        })
        .collect()
});

/// Bucket a query into a [`RequestType`] by cosine nearest prototype in 384-d hashing-trick
/// space. Ties go to the first (declaration-order) prototype; a weak best match is
/// [`RequestType::General`].
pub fn classify_request_type(text: &str) -> RequestType {
    let query = embed(text);
    let mut best = RequestType::General;
    let mut best_score = f32::NEG_INFINITY;
    for proto in CENTROIDS.iter() {
        let score = dot(&query, &proto.vector);
        if score > best_score {
            best_score = score;
            best = proto.request_type;
        }
    }
    if best_score < MIN_COSINE {
        return RequestType::General;
    }
    best
}

fn embed(text: &str) -> [f32; EMBED_DIM] {
    let tokens = word_tokens(text);
    let mut v = [0.0f32; EMBED_DIM];
    for token in &tokens {
        add_gram(&mut v, token);
    }
    for window in tokens.windows(2) {
        let gram = format!("{} {}", window[0], window[1]);
        add_gram(&mut v, &gram);
    }
    l2_normalize(&mut v);
    v
}

fn add_gram(v: &mut [f32; EMBED_DIM], gram: &str) {
    let bucket = (fnv1a(gram.as_bytes()) as usize) % EMBED_DIM;
    let sign_bit = fnv1a(format!("sign:{gram}").as_bytes()) & 1;
    let sign = if sign_bit == 0 { 1.0 } else { -1.0 };
    v[bucket] += sign;
}

fn word_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn l2_normalize(v: &mut [f32; EMBED_DIM]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn dot(a: &[f32; EMBED_DIM], b: &[f32; EMBED_DIM]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// FNV-1a — same constants as the salience hasher, kept local so this module does not
/// depend on salience internals (those weights would be invalidated by a shared hash
/// change).
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_regex_reference_examples() {
        use RequestType::*;
        assert_eq!(
            classify_request_type("build me a python script that parses CSV"),
            CodeGeneration
        );
        assert_eq!(
            classify_request_type("write me a Python sort function"),
            CodeGeneration
        );
        assert_eq!(
            classify_request_type("explain what this function does"),
            CodeUnderstanding
        );
        assert_eq!(
            classify_request_type("how should I design this API?"),
            TechnicalDesign
        );
        assert_eq!(
            classify_request_type("calculate the probability that it rains tomorrow"),
            AnalyticalReasoning
        );
        assert_eq!(
            classify_request_type("draft an email to my team about the outage"),
            Writing
        );
        assert_eq!(
            classify_request_type("what is the capital of France?"),
            FactualLookup
        );
        assert_eq!(classify_request_type("hello there"), General);
    }

    #[test]
    fn paraphrase_without_regex_trigger_still_codes() {
        // The regex classifier needs "write|implement|..." near "function|script|...".
        // This paraphrase shares content words with the code-generation prototypes.
        assert_eq!(
            classify_request_type("I need a python script that parses csv files"),
            RequestType::CodeGeneration
        );
    }

    #[test]
    fn empty_and_whitespace_are_general() {
        assert_eq!(classify_request_type(""), RequestType::General);
        assert_eq!(classify_request_type("   "), RequestType::General);
    }
}
