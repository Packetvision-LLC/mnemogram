#!/usr/bin/env node
import "source-map-support/register";
import * as cdk from "aws-cdk-lib";
import { MnemogramStack } from "../lib/mnemogram-stack";
import { MnemogramApiStack } from "../lib/mnemogram-api-stack";

const app = new cdk.App();

const stage = app.node.tryGetContext("stage") || "dev";

// Main Mnemogram stack (existing infrastructure)
new MnemogramStack(app, `MnemogramStack-${stage}`, {
  stage: stage,
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION,
  },
});

// New API-only stack for api.mnemogram.ai
new MnemogramApiStack(app, `MnemogramApiStack-${stage}`, {
  stage: stage,
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION,
  },
});

app.synth();