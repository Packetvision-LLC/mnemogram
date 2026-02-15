#!/usr/bin/env node
import "source-map-support/register";
import * as cdk from "aws-cdk-lib";
import { MnemogramStack } from "../lib/mnemogram-stack";
import { MnemogramApiStack } from "../lib/mnemogram-api-stack";

const app = new cdk.App();

// Multi-region deployment for production
// Each region gets independent stack with own DynamoDB tables and S3 buckets
// No cross-region dependencies or Global Tables

const regions = ["us-east-1", "us-west-2", "us-central-1"];

regions.forEach(region => {
  const regionSuffix = region.replace("us-", "").replace("-", "");
  
  // Main Mnemogram stack (backend infrastructure)
  new MnemogramStack(app, `MnemogramStack-prod-${regionSuffix}`, {
    stage: "prod",
    env: {
      account: process.env.CDK_DEFAULT_ACCOUNT,
      region: region,
    },
    description: `Mnemogram — Serverless AI Memory Service (PRODUCTION - ${region})`,
  });

  // API-only stack for regional endpoints
  new MnemogramApiStack(app, `MnemogramApiStack-prod-${regionSuffix}`, {
    stage: "prod",
    env: {
      account: process.env.CDK_DEFAULT_ACCOUNT,
      region: region,
    },
    description: `Mnemogram API Stack (PRODUCTION - ${region})`,
  });
});

app.synth();