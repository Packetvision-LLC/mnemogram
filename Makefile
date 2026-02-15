.PHONY: build test lint synth deploy fmt deploy-multiregion synth-multiregion

# Build all Lambdas
build:
	cd lambdas && cargo lambda build --release

# Build for Lambda deployment (creates target/lambda/* structure)
build-lambda:
	cd lambdas && cargo lambda build --release --output-format zip

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

# CDK synth (single region)
synth:
	cd infra && npm run build && npx cdk synth

# CDK synth (multi-region)
synth-multiregion:
	cd infra && npm run build && npx cdk synth --app "npx ts-node bin/mnemogram-$(STAGE)-multiregion.ts"

# CDK deploy (single region, requires AWS credentials)
deploy: build
	cd infra && npm run build && npx cdk deploy --all

# CDK deploy (multi-region, requires AWS credentials and STAGE env var)
# Usage: make deploy-multiregion STAGE=prod
# Usage: make deploy-multiregion STAGE=dev
deploy-multiregion: build
ifndef STAGE
	$(error STAGE is required. Use: make deploy-multiregion STAGE=prod or STAGE=dev)
endif
	cd infra && npm run build && npx cdk deploy --all --app "npx ts-node bin/mnemogram-$(STAGE)-multiregion.ts"

# Deploy to specific region
# Usage: make deploy-region STAGE=prod REGION=us-west-2
deploy-region: build
ifndef STAGE
	$(error STAGE is required. Use: make deploy-region STAGE=prod REGION=us-west-2)
endif
ifndef REGION
	$(error REGION is required. Use: make deploy-region STAGE=prod REGION=us-west-2)
endif
	cd infra && npm run build && CDK_DEFAULT_REGION=$(REGION) npx cdk deploy --all
