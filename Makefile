.PHONY: build test lint synth deploy fmt

# Build all Lambdas
build:
	cd lambdas && cargo lambda build --release

# Run tests
test:
	cd lambdas && cargo test --workspace

# Lint (clippy + fmt check)
lint:
	cd lambdas && cargo fmt --check
	cd lambdas && cargo clippy --workspace -- -D warnings

# Format code
fmt:
	cd lambdas && cargo fmt

# CDK synth
synth:
	cd infra && npm run build && npx cdk synth

# CDK deploy (requires AWS credentials)
deploy: build
	cd infra && npm run build && npx cdk deploy --all
