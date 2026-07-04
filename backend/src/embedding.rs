//! Embedding client seam (ADR-0001 / ADR-0004). Gemini is the provider
//! (supersedes the Cohere choice in `first_draft.md` §C — recorded at close-out
//! of the extraction slice). The seam distinguishes a *document* task type
//! (storage: braindump/concept/type embeddings) from a *query* task type
//! (retrieval seeds).

use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait EmbeddingClient: Send + Sync {
    /// Embed text for storage (braindump / concept / type).
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a query used to seed retrieval.
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;

    /// Dimensionality of every emitted vector.
    fn dim(&self) -> usize;
}

/// A deterministic, dependency-free embedding for tests and offline runs.
///
/// Tokens are hashed into fixed buckets; texts that share tokens land in the
/// same buckets and so have non-trivial cosine similarity. Identical texts map
/// to identical vectors. Not a real semantic model — only a stable stand-in.
#[derive(Clone, Copy, Debug)]
pub struct FakeEmbedding {
    pub dim: usize,
}

impl Default for FakeEmbedding {
    fn default() -> Self {
        Self { dim: 64 }
    }
}

#[async_trait]
impl EmbeddingClient for FakeEmbedding {
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        Ok(deterministic_vector(text, self.dim))
    }
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(deterministic_vector(text, self.dim))
    }
    fn dim(&self) -> usize {
        self.dim
    }
}

/// Token-bucket embedding: each whitespace/alphanumeric token contributes +1 to
/// the bucket its hash falls in. The result is L2-normalised.
pub fn deterministic_vector(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for token in tokenize(text) {
        let h = fnv1a(token.as_bytes()) as usize;
        v[h % dim] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn tokenize(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h = 0x811c9dc5u32;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Cosine similarity for two equal-length vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_identical_vector() {
        let a = deterministic_vector("q3 is at risk", 8);
        let b = deterministic_vector("q3 is at risk", 8);
        assert_eq!(a, b);
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn shared_token_yields_positive_similarity() {
        // Sharing a token guarantees a shared non-zero bucket → dot > 0.
        let a = deterministic_vector("q3 risk from maria", 8);
        let b = deterministic_vector("q3 something else", 8);
        assert!(cosine(&a, &b) > 0.0);
    }

    #[test]
    fn cosine_is_bounded() {
        let a = deterministic_vector("alpha beta", 16);
        let b = deterministic_vector("gamma delta", 16);
        let sim = cosine(&a, &b);
        assert!((-1e-6..=1.0 + 1e-6).contains(&sim));
    }
}
