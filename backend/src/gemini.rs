//! Real Gemini-backed implementations of the LLM, Extractor, and Embedding
//! seams (issue #6). One client serves all three: cleaning (ADR-0007),
//! structured-output extraction (ADR-0001 / ADR-0002 / ADR-0010), and
//! embeddings (ADR-0001 / ADR-0003 / ADR-0004).
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

use crate::embedding::EmbeddingClient;
use crate::error::{Error, Result};
use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult, Extractor};
use crate::llm::LlmClient;

const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_TEXT_MODEL: &str = "gemini-2.0-flash";
const DEFAULT_EMBED_MODEL: &str = "text-embedding-004";
/// Output dimensionality of `text-embedding-004`.
const GEMINI_EMBED_DIM: usize = 768;

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

/// Real Gemini client for all three seams. Constructed from env; `from_env`
/// returns `None` when `GEMINI_API_KEY` is unset, so dev/CI without a key
/// falls back to the fake clients (the ingest pipeline stays hermetic).
#[derive(Clone)]
pub struct GeminiClient {
    api_key: String,
    text_model: String,
    embed_model: String,
    /// `None` = `GEMINI_REASONING_EFFORT` unset → don't send `thinkingConfig`.
    reasoning: Option<ReasoningEffort>,
    http: reqwest::Client,
}

impl GeminiClient {
    /// Build from env. `None` if `GEMINI_API_KEY` is unset (dev/CI fallback).
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("GEMINI_API_KEY").ok()?;
        let text_model =
            std::env::var("GEMINI_TEXT_MODEL").unwrap_or_else(|_| DEFAULT_TEXT_MODEL.to_string());
        let embed_model =
            std::env::var("GEMINI_EMBED_MODEL").unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_string());
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
            .map_err(|e| tracing::warn!(error = %e, "reqwest client build failed; no Gemini"))
            .ok()?;
        Some(Self {
            api_key,
            text_model,
            embed_model,
            reasoning,
            http,
        })
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
            return Err(Error::Internal(format!("gemini generate {status}: {text}")));
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
            return Err(Error::Internal(format!(
                "gemini embed {status}: {body_text}"
            )));
        }
        let value: EmbedResponse = serde_json::from_str(&body_text)
            .map_err(|e| Error::Internal(format!("gemini embed decode: {e}: {body_text}")))?;
        Ok(value.embedding.values)
    }
}

fn map_reqwest(e: reqwest::Error) -> Error {
    Error::Internal(format!("gemini transport: {e}"))
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
impl LlmClient for GeminiClient {
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
}

#[async_trait]
impl EmbeddingClient for GeminiClient {
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        self.embed(text, "RETRIEVAL_DOCUMENT").await
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed(text, "RETRIEVAL_QUERY").await
    }

    fn dim(&self) -> usize {
        GEMINI_EMBED_DIM
    }
}

#[async_trait]
impl Extractor for GeminiClient {
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
        assert_eq!(ReasoningEffort::parse(" none ").unwrap(), ReasoningEffort::None);
        assert_eq!(ReasoningEffort::parse("LOW").unwrap(), ReasoningEffort::Low);
        assert_eq!(ReasoningEffort::parse("Medium").unwrap(), ReasoningEffort::Medium);
        assert_eq!(ReasoningEffort::parse("high").unwrap(), ReasoningEffort::High);
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
}
