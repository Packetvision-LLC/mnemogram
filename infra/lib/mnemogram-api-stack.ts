import * as cdk from "aws-cdk-lib";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as apigateway from "aws-cdk-lib/aws-apigateway";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as origins from "aws-cdk-lib/aws-cloudfront-origins";
import * as certificatemanager from "aws-cdk-lib/aws-certificatemanager";
import * as route53 from "aws-cdk-lib/aws-route53";
import * as targets from "aws-cdk-lib/aws-route53-targets";
import { Construct } from "constructs";

export interface MnemogramApiStackProps extends cdk.StackProps {
  stage: string;
}

export class MnemogramApiStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: MnemogramApiStackProps) {
    super(scope, id, props);

    const stage = props.stage;
    const domainName = `api.mnemogram.ai`;

    // ── Lambda Functions (API-only) ──────────────────────────────────

    const commonEnv = {
      STAGE: stage,
      RUST_LOG: "info",
      // Add DynamoDB table names and other env vars as needed
    };

    // Health/Status endpoint
    const statusFn = new lambda.Function(this, "StatusFn", {
      runtime: lambda.Runtime.PROVIDED_AL2_X86_64,
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../target/lambda/api-status"),
      environment: commonEnv,
      timeout: cdk.Duration.seconds(10),
    });

    // Ingest endpoint
    const ingestFn = new lambda.Function(this, "IngestFn", {
      runtime: lambda.Runtime.PROVIDED_AL2_X86_64,
      handler: "bootstrap", 
      code: lambda.Code.fromAsset("../target/lambda/api-ingest"),
      environment: commonEnv,
      timeout: cdk.Duration.seconds(30),
    });

    // Search endpoint
    const searchFn = new lambda.Function(this, "SearchFn", {
      runtime: lambda.Runtime.PROVIDED_AL2_X86_64,
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../target/lambda/api-search"),
      environment: commonEnv,
      timeout: cdk.Duration.seconds(30),
    });

    // Cards endpoint
    const cardsFn = new lambda.Function(this, "CardsFn", {
      runtime: lambda.Runtime.PROVIDED_AL2_X86_64,
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../target/lambda/api-cards"),
      environment: commonEnv,
      timeout: cdk.Duration.seconds(15),
    });

    // Facts endpoint
    const factsFn = new lambda.Function(this, "FactsFn", {
      runtime: lambda.Runtime.PROVIDED_AL2_X86_64,
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../target/lambda/api-facts"),
      environment: commonEnv,
      timeout: cdk.Duration.seconds(15),
    });

    // State management endpoint
    const stateFn = new lambda.Function(this, "StateFn", {
      runtime: lambda.Runtime.PROVIDED_AL2_X86_64,
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../target/lambda/api-state"),
      environment: commonEnv,
      timeout: cdk.Duration.seconds(15),
    });

    // ── API Gateway ─────────────────────────────────────────────────

    const api = new apigateway.RestApi(this, "MnemogramApi", {
      restApiName: `mnemogram-api-${stage}`,
      description: "Mnemogram Memory API",
      defaultCorsPreflightOptions: {
        allowOrigins: apigateway.Cors.ALL_ORIGINS,
        allowMethods: apigateway.Cors.ALL_METHODS,
        allowHeaders: ["Content-Type", "Authorization", "X-Api-Key"],
      },
      apiKeySourceType: apigateway.ApiKeySourceType.HEADER,
    });

    // API Key for authentication
    const apiKey = new apigateway.ApiKey(this, "ApiKey", {
      apiKeyName: `mnemogram-${stage}-key`,
      description: "API Key for Mnemogram API access",
    });

    // Usage plan
    const usagePlan = new apigateway.UsagePlan(this, "UsagePlan", {
      name: `mnemogram-${stage}-usage`,
      description: "Usage plan for Mnemogram API",
      throttle: {
        rateLimit: 100,
        burstLimit: 200,
      },
      quota: {
        limit: 10000,
        period: apigateway.Period.DAY,
      },
    });

    usagePlan.addApiKey(apiKey);
    usagePlan.addApiStage({
      stage: api.deploymentStage,
    });

    // Routes
    // Health check (no API key required)
    const healthResource = api.root.addResource("health");
    healthResource.addMethod("GET", new apigateway.LambdaIntegration(statusFn));

    // Status (no API key required) 
    const statusResource = api.root.addResource("status");
    statusResource.addMethod("GET", new apigateway.LambdaIntegration(statusFn));

    // V1 API routes (require API key)
    const v1Resource = api.root.addResource("v1");

    const ingestResource = v1Resource.addResource("ingest");
    ingestResource.addMethod("POST", new apigateway.LambdaIntegration(ingestFn), {
      apiKeyRequired: true,
    });

    const searchResource = v1Resource.addResource("search");
    searchResource.addMethod("POST", new apigateway.LambdaIntegration(searchFn), {
      apiKeyRequired: true,
    });

    const cardsResource = v1Resource.addResource("cards");
    cardsResource.addMethod("GET", new apigateway.LambdaIntegration(cardsFn), {
      apiKeyRequired: true,
    });
    cardsResource.addMethod("POST", new apigateway.LambdaIntegration(cardsFn), {
      apiKeyRequired: true,
    });

    const factsResource = v1Resource.addResource("facts");
    factsResource.addMethod("GET", new apigateway.LambdaIntegration(factsFn), {
      apiKeyRequired: true,
    });
    factsResource.addMethod("POST", new apigateway.LambdaIntegration(factsFn), {
      apiKeyRequired: true,
    });

    const stateResource = v1Resource.addResource("state");
    stateResource.addMethod("GET", new apigateway.LambdaIntegration(stateFn), {
      apiKeyRequired: true,
    });
    stateResource.addMethod("PUT", new apigateway.LambdaIntegration(stateFn), {
      apiKeyRequired: true,
    });

    // ── SSL Certificate ─────────────────────────────────────────────

    // Look up existing hosted zone for mnemogram.ai
    const hostedZone = route53.HostedZone.fromLookup(this, "HostedZone", {
      domainName: "mnemogram.ai",
    });

    // Certificate for api.mnemogram.ai (must be in us-east-1 for CloudFront)
    const certificate = new certificatemanager.Certificate(this, "ApiCertificate", {
      domainName: domainName,
      validation: certificatemanager.CertificateValidation.fromDns(hostedZone),
    });

    // ── CloudFront Distribution ─────────────────────────────────────

    const distribution = new cloudfront.Distribution(this, "ApiDistribution", {
      defaultBehavior: {
        origin: new origins.RestApiOrigin(api),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED, // API responses shouldn't be cached
        allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
        originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
      },
      domainNames: [domainName],
      certificate: certificate,
      minimumProtocolVersion: cloudfront.SecurityPolicyProtocol.TLS_V1_2_2021,
      comment: `Mnemogram API (${stage})`,
    });

    // ── DNS Records ─────────────────────────────────────────────────

    new route53.ARecord(this, "ApiAliasRecord", {
      zone: hostedZone,
      recordName: "api",
      target: route53.RecordTarget.fromAlias(
        new targets.CloudFrontTarget(distribution)
      ),
    });

    // ── Outputs ──────────────────────────────────────────────────────

    new cdk.CfnOutput(this, "ApiGatewayUrl", {
      value: api.url,
      description: "API Gateway URL",
    });

    new cdk.CfnOutput(this, "ApiDomainUrl", {
      value: `https://${domainName}`,
      description: "API Domain URL",
    });

    new cdk.CfnOutput(this, "ApiKeyId", {
      value: apiKey.keyId,
      description: "API Key ID",
    });

    new cdk.CfnOutput(this, "CloudFrontUrl", {
      value: `https://${distribution.distributionDomainName}`,
      description: "CloudFront Distribution URL",
    });
  }
}