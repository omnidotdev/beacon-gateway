---
"beacon-gateway": minor
---

Add BM25 keyword search and expand bundled skills to 10

- BM25 scorer (`src/db/bm25.rs`) for ranked keyword matching in memory search
- `search_keyword()` method on `MemoryRepo` replaces naive LIKE matching with BM25 ranking, falling back to LIKE for partial/substring matches
- `search_hybrid()` now uses BM25 keyword results merged with vector similarity
- 9 new bundled skills: summarize, translate, code-review, explain, meeting-notes, proofread, data-analysis, email-draft, debug
