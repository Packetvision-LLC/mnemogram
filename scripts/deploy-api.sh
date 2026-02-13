#!/bin/bash

# Build script for Mnemogram API deployment
set -e

echo "🔧 Setting up Rust build environment..."
export PATH="/home/linuxbrew/.linuxbrew/Cellar/binutils/2.46.0/bin:$PATH"

echo "📦 Building Rust Lambda functions..."
cd ../
make build-lambda

echo "🏗️ Deploying API stack..."
cd infra
npm install
npx cdk deploy MnemogramApiStack-dev --require-approval never

echo "✅ API deployment complete!"
echo "🌐 Test endpoints:"
echo "  Health: https://api.mnemogram.ai/health"
echo "  Status: https://api.mnemogram.ai/status"
echo "  API v1: https://api.mnemogram.ai/v1/* (requires API key)"