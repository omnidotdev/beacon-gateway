//! Text embedding for semantic memory search

use crate::{Error, Result};

/// Embedding dimension for text-embedding-3-small
pub const EMBEDDING_DIM: usize = 1536;

/// Text embedder using `OpenAI`'s embedding API
#[derive(Debug, Clone)]
pub struct Embedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl Embedder {
    /// Create a new embedder with `OpenAI` API key
    ///
    /// # Errors
    ///
    /// Returns error if API key is empty
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config("OpenAI API key required for embeddings".to_string()));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model: "text-embedding-3-small".to_string(),
        })
    }

    /// Create an embedder with a custom model
    ///
    /// # Errors
    ///
    /// Returns error if API key is empty
    pub fn with_model(api_key: String, model: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config("OpenAI API key required for embeddings".to_string()));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model,
        })
    }

    /// Generate embedding for a single text
    ///
    /// # Errors
    ///
    /// Returns error if API call fails
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text]).await?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| Error::Database("empty embedding response".to_string()))
    }

    /// Generate embeddings for multiple texts
    ///
    /// # Errors
    ///
    /// Returns error if API call fails
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        #[derive(serde::Serialize)]
        struct EmbeddingRequest<'a> {
            model: &'a str,
            input: &'a [&'a str],
        }

        #[derive(serde::Deserialize)]
        struct EmbeddingResponse {
            data: Vec<EmbeddingData>,
        }

        #[derive(serde::Deserialize)]
        struct EmbeddingData {
            embedding: Vec<f32>,
            index: usize,
        }

        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };

        let response = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Database(format!("Embedding API error {status}: {body}")));
        }

        let mut result: EmbeddingResponse = response.json().await?;

        // Sort by index to maintain input order
        result.data.sort_by_key(|d| d.index);

        Ok(result.data.into_iter().map(|d| d.embedding).collect())
    }

    /// Serialize embedding to bytes for `SQLite` storage
    #[must_use]
    pub fn to_bytes(embedding: &[f32]) -> Vec<u8> {
        embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect()
    }

    /// Deserialize embedding from bytes
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| {
                let arr: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
                f32::from_le_bytes(arr)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_roundtrip() {
        let embedding = vec![1.0, 2.5, -3.14, 0.0, 100.0];
        let bytes = Embedder::to_bytes(&embedding);
        let restored = Embedder::from_bytes(&bytes);

        assert_eq!(embedding.len(), restored.len());
        for (a, b) in embedding.iter().zip(restored.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_empty_api_key() {
        let result = Embedder::new(String::new());
        assert!(result.is_err());
    }
}
