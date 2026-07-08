//! Real Gemini-backed implementation of the LLM/embedding seam (issue #6).
//! One client serves cleaning (ADR-0007), structured-output extraction
//! (ADR-0001 / ADR-0002 / ADR-0010), grounded synthesis (ADR-0005), and
//! embeddings (ADR-0001 / ADR-0003 / ADR-0004) — the single [`crate::llm::Llm`]
//! trait (issue #39 collapsed the former three traits into it).
//!
//! Provider is Gemini — this supersedes the Cohere choice in `first_draft.md`
//! §C (recorded at close-out of the extraction slice). The embedding model's
//! task-type parameter distinguishes a storage/document task type
//! (braindump/concept/type embeddings) from a query task type (retrieval
//! seeds), per the issue spec.
//!
//! The HTTP + JSON-parsing of the extraction response is split so the parsing
//! half is hermetically testable: [`parse_extraction_response`] takes the
//! decoded Gemini JSON and yields an [`ExtractionResult`]; only the network
//! call is untestable without a key.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use crate::llm::Llm;

const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_TEXT_MODEL: &str = "gemini-2.0-flash";
const DEFAULT_EMBED_MODEL: &str = "text-embedding-004";

/// Output dimensionality for a Gemini embedding model. Used to size the
/// `vec0` virtual tables at startup (`ensure_vec_tables`) BEFORE any embed
/// API call — so it must be derivable from `GEMINI_EMBED_MODEL` without
/// network I/O. A wrong dim panics at the first insert (vec0 rejects
/// mismatched-length vectors), which is exactly the issue #35 crash-loop.
///
/// Unknown models error loudly rather than silently defaulting: a silent
/// wrong dim IS the bug, so guessing is worse than failing to boot.
///
/// `gemini-embedding-001` also supports a configurable `outputDimensionality`
/// request field (max 3072); we use its documented default. Operators who
/// override the output dim must extend this table.
fn embed_dim_for(model: &str) -> Result<usize> {
    match model {
        "text-embedding-004" => Ok(768),
        // Both gemini-embedding-001 and gemini-embedding-2 default to a
        // 3072-dim output (per the Gemini API embeddings docs); `embed()`
        // does not send `outputDimensionality`, so the API returns 3072.
        "gemini-embedding-001" | "gemini-embedding-2" => Ok(3072),
        other => Err(Error::Internal(format!(
            "GEMINI_EMBED_MODEL {other:?} has no known output dimensionality; \
             add it to embed_dim_for() in backend/src/gemini.rs"
        ))),
    }
}

const CLEAN_SYSTEM: &str = "You clean a braindump's verbatim text into a readable rendering. Preserve meaning and order; fix only transcription artifacts (STT hallucinations, typos, casing, punctuation). Return only the cleaned text, no commentary.";

/// Reasoning effort for thinking-capable Gemini 2.5+ models, mapped to a
/// `thinkingConfig.thinkingBudget` token count. `GEMINI_REASONING_EFFORT` is
/// optional: when unset, no `thinkingConfig` is sent (preserves behavior for
/// non-thinking models like 2.0-flash); when set to `none`, thinking is
/// explicitly disabled via budget 0. Budgets are tuned for 2.5-flash (range
/// 0–24576); operators using 2.5-pro may raise `high` toward its 32768 cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// Parse the `GEMINI_REASONING_EFFORT` env value, case-insensitively.
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            other => Err(Error::Internal(format!(
                "GEMINI_REASONING_EFFORT must be one of none|low|medium|high, got {other:?}"
            ))),
        }
    }

    /// Token budget for `thinkingConfig.thinkingBudget`.
    fn budget(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Low => 1024,
            Self::Medium => 8192,
            Self::High => 24576,
        }
    }
}

/// Real Gemini client for the single LLM/embedding seam (issue #39 collapsed
/// the former three traits into one). Constructed from env; `from_env`
/// returns `None` when `GEMINI_API_KEY` is unset, so dev/CI without a key
/// falls back to the fake client (the ingest pipeline stays hermetic).
#[derive(Clone)]
pub struct GeminiClient {
    api_key: String,
    text_model: String,
    embed_model: String,
    /// Cached output dim of `embed_model` (validated at construction via
    /// [`embed_dim_for`]) so [`dim`] stays synchronous and cheap — the vec0
    /// tables are created at startup before any embed API call.
    embed_dim: usize,
    /// `None` = `GEMINI_REASONING_EFFORT` unset → don't send `thinkingConfig`.
    reasoning: Option<ReasoningEffort>,
    http: reqwest::Client,
}

impl GeminiClient {
    /// Build from env. `Ok(None)` if `GEMINI_API_KEY` is unset (dev/CI
    /// fallback to the fake clients). `Err` if the key is set but
    /// `GEMINI_EMBED_MODEL` is not in [`embed_dim_for`]'s table — a wrong
    /// dim is the issue #35 crash-loop, so an unknown model must fail loudly
    /// at startup rather than silently booting with mismatched vec0 tables.
    pub fn from_env() -> Result<Option<Self>> {
        let api_key = match std::env::var("GEMINI_API_KEY") {
            Ok(k) => k,
            Err(_) => return Ok(None),
        };
        let text_model =
            std::env::var("GEMINI_TEXT_MODEL").unwrap_or_else(|_| DEFAULT_TEXT_MODEL.to_string());
        let embed_model =
            std::env::var("GEMINI_EMBED_MODEL").unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_string());
        let embed_dim = embed_dim_for(&embed_model)?;
        let reasoning = match std::env::var("GEMINI_REASONING_EFFORT") {
            Ok(raw) => match ReasoningEffort::parse(&raw) {
                Ok(effort) => Some(effort),
                Err(e) => {
                    tracing::warn!(error = %e, "ignoring GEMINI_REASONING_EFFORT; no thinkingConfig will be sent");
                    None
                }
            },
            Err(_) => None,
        };
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Internal(format!("reqwest client build failed: {e}")))?;
        Ok(Some(Self {
            api_key,
            text_model,
            embed_model,
            embed_dim,
            reasoning,
            http,
        }))
    }

    async fn generate(&self, system: &str, user: &str, mut config: Value) -> Result<String> {
        if let Some(effort) = self.reasoning {
            if let Some(obj) = config.as_object_mut() {
                obj.insert(
                    "thinkingConfig".to_string(),
                    json!({"thinkingBudget": effort.budget()}),
                );
            }
        }
        let url = format!(
            "{GEMINI_BASE}/models/{}:generateContent?key={}",
            self.text_model, self.api_key
        );
        let body = json!({
            "contents": [{"parts": [{"text": user}]}],
            "systemInstruction": {"parts": [{"text": system}]},
            "generationConfig": config,
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest)?;
        let status = resp.status();
        let text = resp.text().await.map_err(map_reqwest)?;
        if !status.is_success() {
            return Err(classify_status(status, &text, "gemini generate"));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| Error::Internal(format!("gemini generate decode: {e}: {text}")))?;
        value["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                Error::Internal(format!("gemini generate: no text in response: {value}"))
            })
    }

    async fn embed(&self, text: &str, task_type: &str) -> Result<Vec<f32>> {
        let url = format!(
            "{GEMINI_BASE}/models/{}:embedContent?key={}",
            self.embed_model, self.api_key
        );
        let body = json!({
            "content": {"parts": [{"text": text}]},
            "taskType": task_type,
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest)?;
        let status = resp.status();
        let body_text = resp.text().await.map_err(map_reqwest)?;
        if !status.is_success() {
            return Err(classify_status(status, &body_text, "gemini embed"));
        }
        let value: EmbedResponse = serde_json::from_str(&body_text)
            .map_err(|e| Error::Internal(format!("gemini embed decode: {e}: {body_text}")))?;
        Ok(value.embedding.values)
    }
}

/// Issue #85: map a transport-level reqwest failure to a transient error —
/// connection resets, DNS, TLS, timeouts are all "try again in 3 minutes," not
/// "give up." Never terminal a braindump on a transport blip.
fn map_reqwest(e: reqwest::Error) -> Error {
    Error::TransientLlm(format!("gemini transport: {e}"))
}

/// Issue #85: classify a non-2xx Gemini status into retryable vs non-retryable.
/// 5xx (server error) and 429 (rate-limited / overloaded) are transient — the
/// provider is briefly unavailable and a retry after the backoff interval is
/// expected to succeed. Any other 4xx is non-retryable: a 400/401/403 is a
/// bad-request or auth problem the retry loop cannot fix, so it terminal's the
/// braindump as `failed` rather than spinning forever.
fn classify_status(status: reqwest::StatusCode, body: &str, ctx: &str) -> Error {
    if status.as_u16() == 429 || status.is_server_error() {
        Error::TransientLlm(format!("{ctx} {status}: {body}"))
    } else {
        Error::Internal(format!("{ctx} {status}: {body}"))
    }
}

#[derive(Deserialize)]
struct EmbedResponse {
    embedding: EmbedValues,
}

#[derive(Deserialize)]
struct EmbedValues {
    values: Vec<f32>,
}

#[async_trait]
impl Llm for GeminiClient {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        let out = self
            .generate(CLEAN_SYSTEM, verbatim, json!({"temperature": 0}))
            .await?;
        Ok(out.trim().to_string())
    }

    async fn generate_pinned(&self, system: &str, user: &str) -> Result<String> {
        // ADR-0003: retagging runs against a *pinned* model snapshot. The model
        // name is configurable via GEMINI_TEXT_MODEL; pinning to a dated
        // snapshot (e.g. gemini-2.0-flash-001) rather than a `-latest` alias
        // is an operator concern at construction time.
        self.generate(system, user, json!({"temperature": 0})).await
    }

    async fn synthesize(&self, system: &str, user: &str) -> Result<String> {
        // ADR-0005: grounded synthesis is temperature 0 — claims must be
        // reproducible, and the citation/silence rules in the system prompt
        // are load-bearing, not ornamental. Free-handed variance is exactly
        // what the grounded-synthesis contract forbids.
        self.generate(system, user, json!({"temperature": 0})).await
    }

    async fn extract(&self, verbatim: &str, ontology_slugs: &[String]) -> Result<ExtractionResult> {
        let system = format!(
            "You extract concepts and typed, directional edges from a braindump.\n\
             - Concepts are recurring nouns/topics the user thinks about; emit each once.\n\
             - Each edge's `type` MUST be one of the sanctioned ontology slugs, drawn from \
             this list and nothing else: [{}]. Never invent a type.\n\
             - `source` and `target` must match concept `label`s you emitted in this response.\n\
             Return JSON: {{\"concepts\": [{{\"label\": string}}], \"edges\": [{{\"source\": string, \"type\": string, \"target\": string}}]}}.",
            ontology_slugs.join(", ")
        );
        let out = self
            .generate(
                &system,
                verbatim,
                json!({
                    "temperature": 0,
                    "responseMimeType": "application/json",
                    "responseSchema": extraction_schema(),
                }),
            )
            .await?;
        let value: Value = serde_json::from_str(&out).map_err(|e| {
            Error::Internal(format!("gemini extract: response was not JSON: {e}: {out}"))
        })?;
        parse_extraction_response(&value)
    }

    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        self.embed(text, "RETRIEVAL_DOCUMENT").await
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed(text, "RETRIEVAL_QUERY").await
    }

    fn dim(&self) -> usize {
        self.embed_dim
    }
}

/// The Gemini structured-output `responseSchema` for extraction. Built per-call
/// (cheap) rather than as a `const` because `serde_json::Value` is not
/// const-constructible.
fn extraction_schema() -> Value {
    json!({
        "type": "OBJECT",
        "properties": {
            "concepts": {
                "type": "ARRAY",
                "items": {
                    "type": "OBJECT",
                    "properties": {"label": {"type": "STRING"}},
                    "required": ["label"]
                }
            },
            "edges": {
                "type": "ARRAY",
                "items": {
                    "type": "OBJECT",
                    "properties": {
                        "source": {"type": "STRING"},
                        "type": {"type": "STRING"},
                        "target": {"type": "STRING"}
                    },
                    "required": ["source", "type", "target"]
                }
            }
        },
        "required": ["concepts", "edges"]
    })
}

/// Parse a decoded Gemini structured-output response into an
/// [`ExtractionResult`]. Pure (no I/O) so it is hermetically testable. Missing
/// `concepts`/`edges` arrays are treated as empty (Gemini may omit empty
/// arrays); malformed entries are errors.
pub fn parse_extraction_response(value: &Value) -> Result<ExtractionResult> {
    let concepts = value
        .get("concepts")
        .and_then(|c| c.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .map(|c| {
            let label = c
                .get("label")
                .and_then(|l| l.as_str())
                .ok_or_else(|| {
                    Error::Internal("extraction: concept missing string `label`".into())
                })?
                .trim()
                .to_string();
            if label.is_empty() {
                return Err(Error::Internal("extraction: concept label is empty".into()));
            }
            Ok(ExtractedConcept { label })
        })
        .collect::<Result<Vec<_>>>()?;

    let edges = value
        .get("edges")
        .and_then(|e| e.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .map(|e| {
            let from_label = required_string(e, "source")?;
            let type_slug = required_string(e, "type")?;
            let to_label = required_string(e, "target")?;
            Ok(ExtractedEdge {
                from_label,
                type_slug,
                to_label,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ExtractionResult { concepts, edges })
}

fn required_string(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::Internal(format!(
                "extraction: edge missing non-empty string `{field}`"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_response() {
        let value = json!({
            "concepts": [{"label": "Q3 review"}, {"label": "Maria"}],
            "edges": [{"source": "Maria", "type": "endangers", "target": "Q3 review"}]
        });
        let result = parse_extraction_response(&value).unwrap();
        assert_eq!(
            result.concepts,
            [
                ExtractedConcept {
                    label: "Q3 review".into()
                },
                ExtractedConcept {
                    label: "Maria".into()
                }
            ]
        );
        assert_eq!(
            result.edges,
            [ExtractedEdge {
                from_label: "Maria".into(),
                type_slug: "endangers".into(),
                to_label: "Q3 review".into()
            }]
        );
    }

    #[test]
    fn parse_empty_arrays() {
        let value = json!({"concepts": [], "edges": []});
        let result = parse_extraction_response(&value).unwrap();
        assert!(result.concepts.is_empty());
        assert!(result.edges.is_empty());
    }

    #[test]
    fn parse_missing_arrays_treated_as_empty() {
        // Gemini may omit an empty array despite the schema.
        let value = json!({});
        let result = parse_extraction_response(&value).unwrap();
        assert!(result.concepts.is_empty());
        assert!(result.edges.is_empty());
    }

    #[test]
    fn parse_edge_missing_type_is_error() {
        let value = json!({
            "concepts": [{"label": "Maria"}],
            "edges": [{"source": "Maria", "target": "Maria"}]
        });
        let err = parse_extraction_response(&value).unwrap_err();
        assert!(err.to_string().contains("`type`"), "{}", err);
    }

    #[test]
    fn parse_blank_label_is_rejected() {
        let value = json!({"concepts": [{"label": "   "}], "edges": []});
        assert!(parse_extraction_response(&value).is_err());
    }

    #[test]
    fn parse_trims_whitespace() {
        let value = json!({
            "concepts": [{"label": "  Q3 review  "}],
            "edges": [{"source": " Maria ", "type": " endangers ", "target": " Q3 review "}]
        });
        let result = parse_extraction_response(&value).unwrap();
        assert_eq!(result.concepts[0].label, "Q3 review");
        assert_eq!(result.edges[0].from_label, "Maria");
        assert_eq!(result.edges[0].type_slug, "endangers");
        assert_eq!(result.edges[0].to_label, "Q3 review");
    }

    #[test]
    fn reasoning_effort_parses_case_insensitively() {
        assert_eq!(
            ReasoningEffort::parse(" none ").unwrap(),
            ReasoningEffort::None
        );
        assert_eq!(ReasoningEffort::parse("LOW").unwrap(), ReasoningEffort::Low);
        assert_eq!(
            ReasoningEffort::parse("Medium").unwrap(),
            ReasoningEffort::Medium
        );
        assert_eq!(
            ReasoningEffort::parse("high").unwrap(),
            ReasoningEffort::High
        );
    }

    #[test]
    fn reasoning_effort_rejects_unknown_keyword() {
        assert!(ReasoningEffort::parse("ultra").is_err());
        assert!(ReasoningEffort::parse("").is_err());
    }

    #[test]
    fn reasoning_effort_budgets_are_ordered_and_none_is_zero() {
        assert_eq!(ReasoningEffort::None.budget(), 0);
        assert!(ReasoningEffort::Low.budget() < ReasoningEffort::Medium.budget());
        assert!(ReasoningEffort::Medium.budget() < ReasoningEffort::High.budget());
    }

    #[test]
    fn embed_dim_for_default_text_embedding_004_is_768() {
        assert_eq!(embed_dim_for(DEFAULT_EMBED_MODEL).unwrap(), 768);
    }

    #[test]
    fn embed_dim_for_gemini_embedding_001_is_3072() {
        // The 3072-dim model from the real infrastructure/.env that crash-looped
        // the backend before issue #35 — must not map to the 768 default.
        assert_eq!(embed_dim_for("gemini-embedding-001").unwrap(), 3072);
    }

    #[test]
    fn embed_dim_for_gemini_embedding_2_is_3072() {
        // gemini-embedding-2 is the multimodal successor and, per the Gemini
        // API docs, shares the same 3072-dim default as gemini-embedding-001.
        // This is the model actually configured in the live infrastructure/.env,
        // so locking it in guards the production startup path directly.
        assert_eq!(embed_dim_for("gemini-embedding-2").unwrap(), 3072);
    }

    #[test]
    fn embed_dim_for_unknown_model_errors_loudly() {
        // A wrong dim is exactly the bug #35 fixes, so an unknown model must
        // NOT silently fall back to a default — it errors so startup fails
        // loudly rather than creating vec0 tables at the wrong dimensionality.
        assert!(embed_dim_for("some-future-model-2099").is_err());
    }
}
