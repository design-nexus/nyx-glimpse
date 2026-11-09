# Third-party notices

Production Glimpse code is original to this project. Its standard dependencies
retain their respective licenses: GTK/GLib, SQLite/rusqlite, notify, serde, and the
other crates listed in Cargo.lock.

Embeddings come from Nyx AI Core, which runs llama.cpp; Glimpse ships no inference
code of its own. EmbeddingGemma 2 weights are published under Apache 2.0.
See https://huggingface.co/google/embeddinggemma-2 and
https://huggingface.co/ggml-org/embeddinggemma-2-GGUF.

Development-only reference fixtures retain their separate notice and MIT license
under tests/data. No Recall engine, extraction, inference wrapper, or ranking code
is included in production.
