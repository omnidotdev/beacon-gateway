//! BM25 relevance scorer for memory search
//!
//! Implements Okapi BM25 (k1=1.2, b=0.75) for ranked keyword search
//! over an in-memory corpus of memories. Adapted from agent-core's
//! knowledge chunk scorer.

use std::collections::{HashMap, HashSet};

/// BM25 tuning parameter: term frequency saturation
const K1: f32 = 1.2;

/// BM25 tuning parameter: document length normalization
const B: f32 = 0.75;

/// BM25 relevance scorer for memories
pub struct Bm25Scorer {
    idf: HashMap<String, f32>,
    docs: Vec<DocStats>,
    avg_dl: f32,
}

struct DocStats {
    term_freq: HashMap<String, u32>,
    len: u32,
}

impl Bm25Scorer {
    /// Build a scorer from memory content strings
    ///
    /// Each entry in `documents` is the searchable text for one memory
    /// (typically content + tags concatenated).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn new(documents: &[String]) -> Self {
        let n = documents.len() as f32;
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        let mut docs = Vec::with_capacity(documents.len());
        let mut total_len: u32 = 0;

        for doc in documents {
            let tokens = tokenize(doc);
            let len = u32::try_from(tokens.len()).unwrap_or(u32::MAX);
            total_len = total_len.saturating_add(len);

            let mut term_freq: HashMap<String, u32> = HashMap::new();
            let mut seen = HashSet::new();

            for token in &tokens {
                *term_freq.entry(token.clone()).or_insert(0) += 1;
                if seen.insert(token.clone()) {
                    *doc_freq.entry(token.clone()).or_insert(0) += 1;
                }
            }

            docs.push(DocStats { term_freq, len });
        }

        let avg_dl = if docs.is_empty() {
            1.0
        } else {
            #[allow(clippy::cast_possible_truncation)]
            {
                f64::from(total_len) as f32 / docs.len() as f32
            }
        };

        let mut idf = HashMap::new();
        for (term, df) in &doc_freq {
            #[allow(clippy::cast_precision_loss)]
            let df_f = *df as f32;
            let val = ((n - df_f + 0.5) / (df_f + 0.5)).ln_1p();
            idf.insert(term.clone(), val);
        }

        Self { idf, docs, avg_dl }
    }

    /// Score a query against all documents
    ///
    /// Returns `(index, score)` pairs sorted by score descending.
    /// Only returns entries with a positive score.
    #[must_use]
    pub fn score(&self, query: &str) -> Vec<(usize, f32)> {
        let query_tokens = tokenize(query);

        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut scores: Vec<(usize, f32)> = self
            .docs
            .iter()
            .enumerate()
            .filter_map(|(i, doc)| {
                let score = self.score_doc(doc, &query_tokens);
                if score > 0.0 { Some((i, score)) } else { None }
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }

    #[allow(clippy::cast_precision_loss)]
    fn score_doc(&self, doc: &DocStats, query_tokens: &[String]) -> f32 {
        let mut score = 0.0_f32;

        for token in query_tokens {
            let Some(&idf) = self.idf.get(token) else {
                continue;
            };

            let tf = *doc.term_freq.get(token).unwrap_or(&0) as f32;
            let dl = doc.len as f32;

            let numerator = tf * (K1 + 1.0);
            let denominator = K1.mul_add(1.0 - B + B * dl / self.avg_dl, tf);
            score += idf * numerator / denominator;
        }

        score
    }
}

/// Tokenize text: lowercase, strip non-alphanumeric, filter empty
fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_term_scoring() {
        let docs = vec![
            "User prefers dark mode".to_string(),
            "Works at Acme Corp".to_string(),
        ];
        let scorer = Bm25Scorer::new(&docs);
        let results = scorer.score("dark");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0);
        assert!(results[0].1 > 0.0);
    }

    #[test]
    fn multi_term_ranking() {
        let docs = vec![
            "User prefers dark mode".to_string(),
            "Works at Acme Corp in dark office".to_string(),
            "Acme Corp uses dark mode everywhere".to_string(),
        ];
        let scorer = Bm25Scorer::new(&docs);
        let results = scorer.score("dark mode Acme");

        // Doc with most matching terms should score highest
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 2); // "Acme Corp uses dark mode everywhere"
    }

    #[test]
    fn empty_query() {
        let docs = vec!["something".to_string()];
        let scorer = Bm25Scorer::new(&docs);
        assert!(scorer.score("").is_empty());
    }

    #[test]
    fn empty_corpus() {
        let scorer = Bm25Scorer::new(&[]);
        assert!(scorer.score("anything").is_empty());
    }

    #[test]
    fn no_match() {
        let docs = vec!["User prefers vim".to_string()];
        let scorer = Bm25Scorer::new(&docs);
        assert!(scorer.score("emacs").is_empty());
    }

    #[test]
    fn rare_terms_score_higher() {
        let docs = vec![
            "the common word appears here".to_string(),
            "the common word appears here too".to_string(),
            "rare unique MCG token".to_string(),
        ];
        let scorer = Bm25Scorer::new(&docs);

        let common_results = scorer.score("the");
        let rare_results = scorer.score("mcg");

        // Rare term should have higher individual score
        assert!(!common_results.is_empty());
        assert!(!rare_results.is_empty());
        assert!(rare_results[0].1 > common_results[0].1);
    }
}
