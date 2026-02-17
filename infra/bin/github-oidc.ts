#!/usr/bin/env node
import "source-map-support/register";
import * as cdk from "aws-cdk-lib";
import { GitHubOidcStack } from "../lib/github-oidc-stack";

const app = new cdk.App();

const stage = app.node.tryGetContext("stage") || "dev";

// GitHub OIDC stack (deploy infrastructure)
new GitHubOidcStack(app, `GitHubOidcStack-${stage}`, {
  stage: stage,
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION,
  },
});

app.synth();