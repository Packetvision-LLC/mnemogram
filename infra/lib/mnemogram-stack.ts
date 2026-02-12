import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as apigateway from "aws-cdk-lib/aws-apigateway";
import { Construct } from "constructs";

export class MnemogramStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);

    // ── Storage ──────────────────────────────────────────────────────

    // S3 bucket for .mv2 memory files
    const memoryBucket = new s3.Bucket(this, "MemoryBucket", {
      bucketName: `mnemogram-memories-${this.account}-${this.region}`,
      encryption: s3.BucketEncryption.S3_MANAGED,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      versioned: true,
    });

    // DynamoDB table for user metadata, API keys, and usage tracking
    const metadataTable = new dynamodb.Table(this, "MetadataTable", {
      tableName: "mnemogram-metadata",
      partitionKey: { name: "pk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "sk", type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // ── Auth ─────────────────────────────────────────────────────────

    const userPool = new cognito.UserPool(this, "UserPool", {
      userPoolName: "mnemogram-users",
      selfSignUpEnabled: true,
      signInAliases: { email: true },
      autoVerify: { email: true },
      passwordPolicy: {
        minLength: 8,
        requireUppercase: true,
        requireDigits: true,
        requireSymbols: false,
      },
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    const userPoolClient = new cognito.UserPoolClient(this, "UserPoolClient", {
      userPool,
      userPoolClientName: "mnemogram-api-client",
      authFlows: {
        userSrp: true,
      },
      generateSecret: false,
    });

    // ── Lambda Functions ─────────────────────────────────────────────
    // Placeholder: these point to dummy code paths.
    // In CI/CD, cargo-lambda builds the binaries and CDK picks them up.

    const lambdaDefaults: Partial<lambda.FunctionProps> = {
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.ARM_64,
      memorySize: 256,
      timeout: cdk.Duration.seconds(30),
      environment: {
        MEMORY_BUCKET: memoryBucket.bucketName,
        METADATA_TABLE: metadataTable.tableName,
        USER_POOL_ID: userPool.userPoolId,
      },
    };

    // Status (health check)
    const statusFn = new lambda.Function(this, "StatusFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-status",
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-status", {
        // During synth without a build, use a dummy path fallback
      }),
      description: "Health check endpoint",
    });

    // Ingest
    const ingestFn = new lambda.Function(this, "IngestFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-ingest",
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-ingest"),
      description: "Ingest content into .mv2 memory files",
    });

    // Search
    const searchFn = new lambda.Function(this, "SearchFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-search",
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-search"),
      description: "Hybrid search over memory files",
    });

    // Manage
    const manageFn = new lambda.Function(this, "ManageFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-manage",
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-manage"),
      description: "CRUD for memory files",
    });

    // Authorizer
    const authorizerFn = new lambda.Function(this, "AuthorizerFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-authorizer",
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/authorizer"),
      description: "JWT custom authorizer",
    });

    // Grant permissions
    memoryBucket.grantRead(searchFn);
    memoryBucket.grantReadWrite(ingestFn);
    memoryBucket.grantReadWrite(manageFn);
    metadataTable.grantReadWriteData(ingestFn);
    metadataTable.grantReadData(searchFn);
    metadataTable.grantReadWriteData(manageFn);

    // ── API Gateway ──────────────────────────────────────────────────

    const api = new apigateway.RestApi(this, "MnemogramApi", {
      restApiName: "mnemogram-api",
      description: "Mnemogram REST API",
      deployOptions: {
        stageName: "v1",
        throttlingRateLimit: 100,
        throttlingBurstLimit: 200,
      },
      defaultCorsPreflightOptions: {
        allowOrigins: apigateway.Cors.ALL_ORIGINS,
        allowMethods: apigateway.Cors.ALL_METHODS,
      },
    });

    const authorizer = new apigateway.TokenAuthorizer(this, "JwtAuthorizer", {
      handler: authorizerFn,
      identitySource: "method.request.header.Authorization",
      resultsCacheTtl: cdk.Duration.minutes(5),
    });

    // Routes
    const statusResource = api.root.addResource("status");
    statusResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(statusFn)
    );

    const memoriesResource = api.root.addResource("memories");
    memoriesResource.addMethod(
      "PUT",
      new apigateway.LambdaIntegration(ingestFn),
      { authorizer }
    );
    memoriesResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(manageFn),
      { authorizer }
    );
    memoriesResource.addMethod(
      "DELETE",
      new apigateway.LambdaIntegration(manageFn),
      { authorizer }
    );

    const searchResource = api.root.addResource("search");
    searchResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(searchFn),
      { authorizer }
    );

    // ── Outputs ──────────────────────────────────────────────────────

    new cdk.CfnOutput(this, "ApiUrl", {
      value: api.url,
      description: "API Gateway endpoint URL",
    });

    new cdk.CfnOutput(this, "UserPoolId", {
      value: userPool.userPoolId,
      description: "Cognito User Pool ID",
    });

    new cdk.CfnOutput(this, "UserPoolClientId", {
      value: userPoolClient.userPoolClientId,
      description: "Cognito User Pool Client ID",
    });

    new cdk.CfnOutput(this, "MemoryBucketName", {
      value: memoryBucket.bucketName,
      description: "S3 bucket for .mv2 files",
    });
  }
}
