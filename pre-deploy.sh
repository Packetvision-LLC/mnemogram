#!/bin/bash

# Pre-deployment validation script for Mnemogram
# This script runs all necessary checks before allowing deployment

set -e  # Exit on any error

echo "🚀 Starting pre-deployment validation..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print status
print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

# Track validation steps
VALIDATION_FAILED=0

# Step 1: Prerequisites Check
echo "📋 Checking prerequisites..."
command -v cargo >/dev/null 2>&1 || { print_error "cargo not found"; VALIDATION_FAILED=1; }
command -v npm >/dev/null 2>&1 || { print_error "npm not found"; VALIDATION_FAILED=1; }
print_status "Prerequisites check completed"

# Step 2: Dependencies Check
echo "📦 Installing/updating dependencies..."
cd lambdas && cargo check --workspace && cd ..
cd infra && npm ci && cd ..
print_status "Dependencies check completed"

# Step 3: Rust TypeScript Check & Formatting
echo "🦀 Running Rust checks..."
cd lambdas

# Check formatting
echo "  - Checking Rust formatting..."
if ! cargo fmt --check; then
    print_error "Rust formatting check failed"
    print_warning "Run 'cargo fmt' to fix formatting issues"
    VALIDATION_FAILED=1
else
    print_status "Rust formatting check passed"
fi

# Run clippy
echo "  - Running Clippy..."
if ! cargo clippy --workspace -- -D warnings; then
    print_error "Clippy check failed"
    VALIDATION_FAILED=1
else
    print_status "Clippy check passed"
fi

# Run tests
echo "  - Running Rust tests..."
if ! cargo test --workspace; then
    print_error "Rust tests failed"
    VALIDATION_FAILED=1
else
    print_status "Rust tests passed"
fi

cd ..

# Step 4: CDK TypeScript Checks
echo "📦 Running CDK checks..."
cd infra

# TypeScript compilation
echo "  - Compiling TypeScript..."
if ! npm run build; then
    print_error "TypeScript compilation failed"
    VALIDATION_FAILED=1
else
    print_status "TypeScript compilation passed"
fi

# CDK synthesis
echo "  - Running CDK synthesis..."
if ! npx cdk synth > /dev/null; then
    print_error "CDK synthesis failed"
    VALIDATION_FAILED=1
else
    print_status "CDK synthesis passed"
fi

cd ..

# Step 5: Security Audit (if available)
echo "🔒 Running security checks..."
cd lambdas
if command -v cargo-audit >/dev/null 2>&1; then
    if ! cargo audit; then
        print_warning "Security audit found issues (this may not block deployment)"
    else
        print_status "Security audit passed"
    fi
else
    print_warning "cargo-audit not found, skipping security audit"
fi
cd ..

# Final result
if [ $VALIDATION_FAILED -eq 0 ]; then
    echo ""
    echo -e "${GREEN}🎉 All pre-deployment checks passed! Ready for deployment.${NC}"
    exit 0
else
    echo ""
    echo -e "${RED}❌ Pre-deployment validation failed. Please fix the issues above before deploying.${NC}"
    exit 1
fi