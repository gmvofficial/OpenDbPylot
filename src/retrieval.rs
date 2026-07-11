//! Hybrid lexical + semantic retrieval.
//!
//! Ranks stored items by fusing two signals with **Reciprocal Rank Fusion**:
//!
//! - **BM25** keyword scoring — catches exact schema terms ("order_items",
//!   "cancelled") that weak embeddings miss entirely;
//! - **cosine similarity** over embeddings — catches paraphrases ("how many
//!   purchases" ≈ "count of orders") that share no keywords.
//!
//! RRF combines *ranks*, not raw scores, so the two signals don't need to be
//! on comparable scales. An item ranked highly by either signal surfaces; an
//! item ranked highly by both wins.

/// Lowercased alphanumeric tokens; `order_items` → `["order", "items"]`.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// BM25 score of `query` tokens against each pre-tokenized document.
/// Standard parameters k1 = 1.2, b = 0.75; IDF uses the `ln(1 + …)` form so
/// scores stay non-negative even for very common terms.
pub fn bm25_scores(query: &[String], docs: &[Vec<String>]) -> Vec<f32> {
    const K1: f32 = 1.2;
    const B: f32 = 0.75;

    let n_docs = docs.len();
    if n_docs == 0 || query.is_empty() {
        return vec![0.0; n_docs];
    }
    let avg_len: f32 =
        docs.iter().map(|d| d.len() as f32).sum::<f32>() / n_docs as f32;
    let avg_len = avg_len.max(1.0);

    query
        .iter()
        .fold(vec![0.0f32; n_docs], |mut scores, term| {
            // Document frequency of this term.
            let df = docs.iter().filter(|d| d.iter().any(|t| t == term)).count() as f32;
            if df == 0.0 {
                return scores;
            }
            let idf = (1.0 + (n_docs as f32 - df + 0.5) / (df + 0.5)).ln();
            for (i, doc) in docs.iter().enumerate() {
                let tf = doc.iter().filter(|t| *t == term).count() as f32;
                if tf > 0.0 {
                    let norm = K1 * (1.0 - B + B * doc.len() as f32 / avg_len);
                    scores[i] += idf * (tf * (K1 + 1.0)) / (tf + norm);
                }
            }
            scores
        })
}

/// Fuse a cosine ranking and a BM25 ranking with RRF (k = 60) and return the
/// indices of the top `n` items.
///
/// Items with a zero BM25 score (no keyword overlap at all) get no lexical
/// contribution — they can still rank purely on the semantic side. Ties break
/// by cosine score for determinism.
pub fn hybrid_rank(cosine: &[f32], bm25: &[f32], n: usize) -> Vec<usize> {
    const K: f32 = 60.0;
    debug_assert_eq!(cosine.len(), bm25.len());
    let len = cosine.len();

    // rank_of[i] = 1-based position of item i when sorted by the given scores.
    let rank_of = |scores: &[f32], skip_zero: bool| -> Vec<Option<usize>> {
        let mut order: Vec<usize> = (0..len).collect();
        order.sort_by(|&a, &b| {
            scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut ranks = vec![None; len];
        for (pos, &i) in order.iter().enumerate() {
            if !(skip_zero && scores[i] <= 0.0) {
                ranks[i] = Some(pos + 1);
            }
        }
        ranks
    };

    let cos_ranks = rank_of(cosine, false);
    let bm_ranks = rank_of(bm25, true);

    let mut fused: Vec<(usize, f32)> = (0..len)
        .map(|i| {
            let mut score = 0.0;
            if let Some(r) = cos_ranks[i] {
                score += 1.0 / (K + r as f32);
            }
            if let Some(r) = bm_ranks[i] {
                score += 1.0 / (K + r as f32);
            }
            (i, score)
        })
        .collect();

    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Tie-break on the semantic score, then index (stable output).
            .then_with(|| cosine[b.0].partial_cmp(&cosine[a.0]).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.0.cmp(&b.0))
    });
    fused.into_iter().take(n).map(|(i, _)| i).collect()
}

/// One-call convenience used by the vector stores: hybrid-rank `texts` (the
/// searchable text of each entry) against the query and return top-`n` indices.
pub fn hybrid_top_n(
    query_text: &str,
    query_emb: &[f32],
    texts: &[&str],
    embeddings: &[&[f32]],
    n: usize,
) -> Vec<usize> {
    let cosine: Vec<f32> = embeddings
        .iter()
        .map(|e| crate::embedding::cosine_similarity(query_emb, e))
        .collect();
    let query_tokens = tokenize(query_text);
    let doc_tokens: Vec<Vec<String>> = texts.iter().map(|t| tokenize(t)).collect();
    let bm25 = bm25_scores(&query_tokens, &doc_tokens);
    hybrid_rank(&cosine, &bm25, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_splits_identifiers() {
        assert_eq!(tokenize("order_items.unit_price"), vec!["order", "items", "unit", "price"]);
        assert_eq!(tokenize("Cancelled, refunded!"), vec!["cancelled", "refunded"]);
    }

    #[test]
    fn bm25_prefers_rare_exact_terms() {
        let docs: Vec<Vec<String>> = [
            "orders status one of completed shipped pending cancelled refunded",
            "customers have a name a country and a city",
            "revenue is quantity times unit price",
        ]
        .iter()
        .map(|d| tokenize(d))
        .collect();

        let scores = bm25_scores(&tokenize("which orders were cancelled"), &docs);
        assert!(scores[0] > scores[1], "{scores:?}");
        assert!(scores[0] > scores[2], "{scores:?}");
    }

    #[test]
    fn rrf_lets_keywords_beat_noisy_embeddings() {
        // Embeddings are all identical (pure noise) — BM25 must decide.
        let cosine = vec![0.5, 0.5, 0.5];
        let bm25 = vec![0.0, 3.2, 0.1];
        let top = hybrid_rank(&cosine, &bm25, 3);
        assert_eq!(top[0], 1);
    }

    #[test]
    fn rrf_lets_semantics_win_without_keyword_overlap() {
        // No keyword overlap anywhere — cosine must decide.
        let cosine = vec![0.1, 0.9, 0.3];
        let bm25 = vec![0.0, 0.0, 0.0];
        let top = hybrid_rank(&cosine, &bm25, 3);
        assert_eq!(top, vec![1, 2, 0]);
    }

    #[test]
    fn agreement_on_both_signals_wins() {
        // Item 2 is second-best on BOTH signals. Item 0 tops cosine but has no
        // keyword overlap; item 1 tops BM25 but ranks last on cosine.
        // Consistent agreement wins the fusion.
        let cosine = vec![0.9, 0.1, 0.8, 0.5];
        let bm25 = vec![0.0, 5.0, 4.0, 0.1];
        let top = hybrid_rank(&cosine, &bm25, 4);
        assert_eq!(top[0], 2, "{top:?}");
    }

    #[test]
    fn hybrid_top_n_end_to_end() {
        let texts = vec![
            "CREATE TABLE order_items (id INTEGER, order_id INTEGER, quantity INTEGER, unit_price REAL)",
            "CREATE TABLE customers (id INTEGER, name TEXT, country TEXT)",
        ];
        // Identical embeddings → the exact keyword match must win.
        let e = vec![1.0f32, 0.0];
        let embs: Vec<&[f32]> = vec![&e, &e];
        let top = hybrid_top_n("total quantity in order items", &e, &texts, &embs, 2);
        assert_eq!(top[0], 0);
    }
}
