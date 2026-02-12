#!/usr/bin/env node
import "source-map-support/register";
import * as cdk from "aws-cdk-lib";
import { MnemogramStack } from "../lib/mnemogram-stack";

const app = new cdk.App();

// Get stage from context or default to dev
const stage = app.node.tryGetContext("stage") || "dev";

if (!["dev", "staging", "prod"].includes(stage)) {
  throw new Error(`Invalid stage: ${stage}. Must be one of: dev, staging, prod`);
}

new MnemogramStack(app, `MnemogramStack-${stage}`, {
  stage,
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION ?? "us-east-1",
  },
  description: `Mnemogram — Serverless AI Memory Service (${stage.toUpperCase()})`,
});
