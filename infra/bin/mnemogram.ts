#!/usr/bin/env node
import "source-map-support/register";
import * as cdk from "aws-cdk-lib";
import { MnemogramStack } from "../lib/mnemogram-stack";

const app = new cdk.App();

new MnemogramStack(app, "MnemogramStack", {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION ?? "us-east-1",
  },
  description: "Mnemogram — Serverless AI Memory Service",
});
