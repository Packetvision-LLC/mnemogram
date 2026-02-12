# memvid

A command-line tool for building and querying AI memory files. Store documents, search with BM25 + vector ranking, and run RAG queries from a single portable `.mv2` file.

Built in Rust. No database required.

## Install

```bash
npm install -g memvid-cli
```

Or run directly without installing:

```bash
npx memvid-cli --help
```

## Quick Start

```bash
# Create a memory and add some documents
memvid create research.mv2
memvid put research.mv2 --text "Rust achieves memory safety without garbage collection"
memvid put research.mv2 --text "Python excels at rapid prototyping and data analysis"
memvid put research.mv2 --input ./papers/

# Search
memvid find research.mv2 --query "memory safety"

# Ask questions (requires OPENAI_API_KEY for synthesis)
memvid ask research.mv2 --question "Compare Rust and Python for systems programming"

# Check stats
memvid stats research.mv2
```

## Commands

### Creating and Ingesting

```bash
# Create a new memory file
memvid create notes.mv2

# Add text directly
memvid put notes.mv2 --text "Your content here"

# Add from file (supports PDF, DOCX, TXT, MD, HTML, and more)
memvid put notes.mv2 --input document.pdf

# Add entire folder recursively
memvid put notes.mv2 --input ./documents/

# Batch ingest with embeddings for semantic search
memvid put-many notes.mv2 --input ./corpus/ --embedding bge-small
```

### Searching

```bash
# Lexical search (BM25)
memvid find notes.mv2 --query "machine learning"

# Semantic search (requires embeddings)
memvid find notes.mv2 --query "ML algorithms" --mode sem

# Hybrid search (lexical + semantic reranking)
memvid find notes.mv2 --query "neural networks" --mode auto

# Limit results
memvid find notes.mv2 --query "data" --k 5
```

### Question Answering

```bash
# Basic RAG query
memvid ask notes.mv2 --question "What are the key findings?"

# Use a specific model
memvid ask notes.mv2 --question "Summarize the main points" --model openai:gpt-4o

# Get context only (no LLM synthesis)
memvid ask notes.mv2 --question "What is discussed?" --context-only
```

### Inspection and Maintenance

```bash
# View stats
memvid stats notes.mv2

# View timeline of recent additions
memvid timeline notes.mv2 --limit 20

# View a specific frame
memvid view notes.mv2 --frame 42

# Verify file integrity
memvid verify notes.mv2

# Repair indexes
memvid doctor notes.mv2 --rebuild-lex-index
```

## Embedding Models

For semantic search, you need to generate embeddings during ingestion:

```bash
# Local models (fast, no API key needed)
memvid put notes.mv2 --input doc.pdf --embedding bge-small
memvid put notes.mv2 --input doc.pdf --embedding nomic

# OpenAI models (requires OPENAI_API_KEY)
memvid put notes.mv2 --input doc.pdf --embedding openai-small
```

Available local models: `bge-small`, `bge-base`, `nomic`, `gte-large`

Available OpenAI models: `openai-small`, `openai-large`, `openai-ada`

**Windows users:** Local embedding models are not available on Windows due to ONNX runtime limitations. Use OpenAI embeddings instead:

```bash
set OPENAI_API_KEY=sk-...
memvid put notes.mv2 --input doc.pdf --embedding openai-small
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `OPENAI_API_KEY` | Required for OpenAI embeddings and LLM synthesis |
| `OPENAI_BASE_URL` | Custom OpenAI-compatible endpoint |
| `NVIDIA_API_KEY` | For NVIDIA NIM embeddings |
| `MEMVID_MODELS_DIR` | Where to cache local embedding models |
| `MEMVID_API_KEY` | For capacity beyond the free tier |

## Supported Platforms

| Platform | Architecture | Local Embeddings |
|----------|--------------|------------------|
| macOS | ARM64 (Apple Silicon) | Yes |
| macOS | x64 (Intel) | Yes |
| Linux | x64 (glibc) | Yes |
| Windows | x64 | No (use OpenAI) |

## Document Formats

The CLI uses Apache Tika for document extraction and supports:

- PDF, DOCX, XLSX, PPTX
- HTML, XML, Markdown
- Plain text, CSV, JSON
- Images (with OCR when available)
- And many more

## Examples

### Build a Research Knowledge Base

```bash
memvid create papers.mv2
memvid put-many papers.mv2 --input ~/Downloads/arxiv/ --embedding bge-small
memvid ask papers.mv2 --question "What are recent advances in transformer architectures?"
```

### Index Code Documentation

```bash
memvid create docs.mv2
memvid put docs.mv2 --input ./docs/ --label documentation
memvid put docs.mv2 --input ./README.md --label readme
memvid find docs.mv2 --query "authentication" --k 10
```

### Personal Note Archive

```bash
memvid create notes.mv2
memvid put notes.mv2 --text "Meeting with Alice: discussed Q4 roadmap" --label meeting
memvid put notes.mv2 --text "Idea: use vector search for semantic dedup" --label idea
memvid timeline notes.mv2 --limit 50
```

## More Information

- Documentation: https://docs.memvid.com
- GitHub: https://github.com/memvid/memvid
- Discord: https://discord.gg/2mynS7fcK7
- Website: https://memvid.com

## License

Apache-2.0
