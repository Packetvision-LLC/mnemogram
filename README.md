# Mnemogram

**Serverless AI memory service powered by [MemVid](https://github.com/Olow304/memvid)**

Mnemogram provides a managed REST API for storing, searching, and retrieving AI memories using MemVid's `.mv2` format — all running on AWS serverless infrastructure with zero servers to manage.

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│  Client/SDK  │────▶│  CloudFront   │────▶│   API Gateway    │
└─────────────┘     └──────────────┘     └────────┬────────┘
                                                   │
                                          ┌────────▼────────┐
                                          │   Authorizer     │
                                          │  (Cognito JWT)   │
                                          └────────┬────────┘
                           ┌───────────────────────┼───────────────────────┐
                           │                       │                       │
                    ┌──────▼──────┐         ┌──────▼──────┐         ┌──────▼──────┐
                    │  api-ingest  │         │  api-search  │         │  api-manage  │
                    │  PUT /memories│        │  GET /search  │        │  CRUD /memories│
                    └──────┬──────┘         └──────┬──────┘         └──────┬──────┘
                           │                       │                       │
                    ┌──────▼──────────────────────▼──────────────────────▼──────┐
                    │                        S3 (.mv2 files)                     │
                    │                     DynamoDB (metadata)                    │
                    └──────────────────────────────────────────────────────────┘
```

### Tech Stack

| Layer | Technology |
|-------|-----------|
| **Lambda Runtime** | Rust via `cargo-lambda` — sub-50ms cold starts |
| **Infrastructure** | AWS CDK (TypeScript) |
| **API** | API Gateway REST + Lambda |
| **Auth** | Amazon Cognito (JWT) + custom authorizer |
| **Storage** | S3 (`.mv2` files) + DynamoDB (metadata) |
| **CDN** | CloudFront |
| **CI/CD** | GitHub Actions → CDK deploy |

### Design Principles

- **One Lambda per route** — independent scaling, isolated failures, fine-grained IAM
- **The `.mv2` file IS the database** — DynamoDB only for metadata and usage tracking
- **Monorepo** — infra, lambdas, SDKs, and skill in one place
- **Shared crate** — all MemVid interaction in `lambdas/shared/`

## Project Structure

```
mnemogram/
├── infra/              # AWS CDK stacks (TypeScript)
│   ├── bin/            # CDK app entrypoint
│   └── lib/            # Stack definitions
├── lambdas/            # Rust Lambda functions (Cargo workspace)
│   ├── shared/         # Shared types, clients, MemVid wrapper
│   ├── api-ingest/     # PUT /memories — write/append content
│   ├── api-search/     # GET /search — hybrid BM25+vector search
│   ├── api-manage/     # CRUD for memory files
│   ├── api-status/     # GET /status — health check
│   └── authorizer/     # JWT custom authorizer
├── sdks/
│   ├── typescript/     # @mnemogram/sdk
│   └── python/         # mnemogram (PyPI)
├── skill/              # ClawHub client skill package
├── docs/               # Documentation
└── scripts/            # Dev and deployment helpers
```

## Getting Started

### Prerequisites

- **Rust** (stable) + `cargo-lambda` — `cargo install cargo-lambda`
- **Node.js 22+** & npm
- **AWS CDK CLI** — `npm install -g aws-cdk`
- **AWS credentials** configured (`aws configure`)

### Build Lambdas

```bash
cd lambdas
cargo lambda build --release --arm64
```

### Deploy Infrastructure

```bash
cd infra
npm install
npx cdk bootstrap   # first time only
npx cdk deploy --all
```

### Local Development

```bash
# Run a single Lambda locally
cd lambdas
cargo lambda watch -f api-status

# In another terminal, invoke it
cargo lambda invoke api-status --data-ascii '{"httpMethod":"GET","path":"/status"}'
```

## API Endpoints

| Method | Path | Lambda | Auth | Description |
|--------|------|--------|------|-------------|
| `GET` | `/status` | api-status | None | Health check |
| `PUT` | `/memories` | api-ingest | JWT | Ingest content into .mv2 |
| `GET` | `/memories` | api-manage | JWT | List memory files |
| `DELETE` | `/memories` | api-manage | JWT | Delete memory files |
| `GET` | `/search` | api-search | JWT | Hybrid search over memories |

## CI/CD

GitHub Actions workflows in `.github/workflows/`:

| Workflow | Trigger | What it does |
|----------|---------|-------------|
| **ci.yml** | PR & push to main | `cargo fmt`, `clippy`, `test` + CDK `build` & `synth` |
| **deploy.yml** | Push to main | Build Lambdas → CDK deploy (OIDC, manual approval) |
| **rust-audit.yml** | Weekly (Monday) | `cargo audit` security scan |

### Quick Commands (Makefile)

```bash
make build    # Build all Lambdas with cargo-lambda
make test     # Run Rust tests
make lint     # clippy + fmt check
make fmt      # Auto-format Rust code
make synth    # CDK synth
make deploy   # Build + CDK deploy
```

### Deploy Setup

The deploy workflow uses OIDC federation — no long-lived AWS keys. You need to:

1. Create an IAM role with GitHub OIDC trust policy (`arn:aws:iam::ACCOUNT_ID:role/mnemogram-github-deploy`)
2. Add a `production` environment in GitHub repo settings (for the approval gate)
3. Update the `AWS_ROLE_ARN` in `deploy.yml` with your actual account ID

## License

Apache License 2.0 — see [LICENSE](LICENSE).

---

Built by [Packetvision LLC](https://packetvision.com)
# Last deployment: Tue Feb 17 03:21:00 UTC 2026
