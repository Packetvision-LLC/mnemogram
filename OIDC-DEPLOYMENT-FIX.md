# Mnemogram GitHub Actions OIDC Deployment Fix

## Problem
GitHub Actions CI and Deploy workflows are failing:

1. **CI Failure**: CDK can't find Lambda assets at `/home/runner/work/mnemogram/mnemogram/lambdas/target/lambda/api-status`
2. **Deploy Failure**: OIDC authentication error - "Not authorized to perform sts:AssumeRoleWithWebIdentity"

## Root Cause
Missing AWS IAM role `arn:aws:iam::369292120314:role/mnemogram-github-deploy` and GitHub OIDC provider in AWS.

## Solution

### 1. Fixed Rust Compilation Issues (COMPLETED)
- Fixed `AttributeValue::SS` → `AttributeValue::Ss` in api-ingest 
- Fixed HashMap import and type issues in maintenance Lambda
- Fixed unnecessary borrow in maintenance Lambda
- Added dead_code annotations for unused struct fields
- Ran `cargo fmt` to fix formatting

### 2. Deploy GitHub OIDC Infrastructure (REQUIRES STUART)

**Manual Deployment Required**: Run this from the mnemogram repo:

```bash
cd /home/stuart/.openclaw/workspace-cody/mnemogram/infra

# Deploy the GitHub OIDC stack
npx cdk deploy GitHubOidcStack-dev --profile default

# This will create:
# - GitHub OIDC Provider (token.actions.githubusercontent.com)
# - IAM Role: mnemogram-github-deploy 
# - Proper trust policy for stbain/mnemogram and stbain/mnemogram-web repos
```

The stack is defined in:
- `/home/stuart/.openclaw/workspace-cody/mnemogram/infra/bin/github-oidc.ts`
- `/home/stuart/.openclaw/workspace-cody/mnemogram/infra/lib/github-oidc-stack.ts`

### 3. Verification
Once deployed, the GitHub Actions workflows should succeed:
- CI workflow will build and upload Lambda artifacts
- Deploy workflow will assume the new role and deploy infrastructure

## Status
- ✅ Rust compilation fixes committed to dev branch
- ⏳ Pre-deploy validation running (Lambda builds in progress)
- ⏳ Awaiting OIDC infrastructure deployment by Stuart

## Next Steps
1. Stuart deploys OIDC infrastructure using command above
2. Test GitHub Actions workflows (push to main branch)
3. Monitor deployment success via webhook notifications

Created: 2026-02-17 22:30 UTC