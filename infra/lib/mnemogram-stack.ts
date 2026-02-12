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

    // DynamoDB table for memories metadata
    const memoriesTable = new dynamodb.Table(this, "MemoriesTable", {
      tableName: "mnemogram-memories",
      partitionKey: { name: "memoryId", type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // Add GSI on userId for querying user's memories
    memoriesTable.addGlobalSecondaryIndex({
      indexName: "userId-index",
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
    });

    // DynamoDB table for subscription management
    const subscriptionsTable = new dynamodb.Table(this, "SubscriptionsTable", {
      tableName: "mnemogram-subscriptions",
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // DynamoDB table for API key management  
    const apiKeysTable = new dynamodb.Table(this, "ApiKeysTable", {
      tableName: "mnemogram-api-keys",
      partitionKey: { name: "keyId", type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // Add GSI on userId for API keys
    apiKeysTable.addGlobalSecondaryIndex({
      indexName: "userId-index",
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
    });

    // DynamoDB table for usage tracking
    const usageTable = new dynamodb.Table(this, "UsageTable", {
      tableName: "mnemogram-usage",
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "date", type: dynamodb.AttributeType.STRING },
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
      standardAttributes: {
        email: {
          required: true,
          mutable: true,
        },
        fullname: {
          required: false,
          mutable: true,
        },
      },
      customAttributes: {
        'subscription_tier': new cognito.StringAttribute({ minLen: 1, maxLen: 20 }),
        'created_at': new cognito.StringAttribute({ minLen: 1, maxLen: 50 }),
      },
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    const userPoolClient = new cognito.UserPoolClient(this, "UserPoolClient", {
      userPool,
      userPoolClientName: "mnemogram-web-client",
      authFlows: {
        userSrp: true,
      },
      generateSecret: false,
      oAuth: {
        flows: {
          authorizationCodeGrant: true,
        },
        scopes: [cognito.OAuthScope.EMAIL, cognito.OAuthScope.OPENID, cognito.OAuthScope.PROFILE],
        callbackUrls: ['http://localhost:3000/dashboard', 'https://mnemogram.com/dashboard'],
        logoutUrls: ['http://localhost:3000/', 'https://mnemogram.com/'],
      },
    });

    const userPoolDomain = new cognito.UserPoolDomain(this, "UserPoolDomain", {
      userPool,
      cognitoDomain: {
        domainPrefix: `mnemogram-auth-${this.account}`,
      },
    });

    // ── Lambda Functions ─────────────────────────────────────────────
    // Placeholder: these point to dummy code paths.
    // In CI/CD, cargo-lambda builds the binaries and CDK picks them up.

    const lambdaDefaults: lambda.FunctionProps = {
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.ARM_64,
      memorySize: 256,
      timeout: cdk.Duration.seconds(30),
      handler: "bootstrap",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-status"),
      environment: {
        MEMORY_BUCKET: memoryBucket.bucketName,
        METADATA_TABLE: metadataTable.tableName,
        MEMORIES_TABLE: memoriesTable.tableName,
        SUBSCRIPTIONS_TABLE: subscriptionsTable.tableName,
        API_KEYS_TABLE: apiKeysTable.tableName,
        USAGE_TABLE: usageTable.tableName,
        USER_POOL_ID: userPool.userPoolId,
      },
    };

    // Status (health check)
    const statusFn = new lambda.Function(this, "StatusFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-status",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-status", {
        // During synth without a build, use a dummy path fallback
      }),
      description: "Health check endpoint",
    });

    // Ingest (new POST /memories for S3 pre-signed URL flow)
    const ingestFn = new lambda.Function(this, "IngestFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-ingest",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/ingest"),
      description: "Ingest memory metadata and generate S3 upload URL",
    });

    // Search within a memory (new POST /memories/{id}/search)
    const searchMemoryFn = new lambda.Function(this, "SearchMemoryFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-search-memory",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/search"),
      description: "Search within a specific memory",
    });

    // Search (existing GET /search endpoint for backward compatibility)
    const searchFn = new lambda.Function(this, "SearchFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-search",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-search"),
      description: "Hybrid search over memory files",
    });

    // Recall
    const recallFn = new lambda.Function(this, "RecallFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-recall",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/recall"),
      description: "Broader recall across all user memories",
    });

    // Manage
    const manageFn = new lambda.Function(this, "ManageFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-manage",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-manage"),
      description: "CRUD for memory files",
    });

    // Authorizer (updated)
    const authorizerFn = new lambda.Function(this, "AuthorizerFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-authorizer",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/authorizer"),
      description: "JWT/API key custom authorizer",
    });

    // Stripe webhook handler
    const stripeWebhookFn = new lambda.Function(this, "StripeWebhookFn", {
      ...lambdaDefaults,
      functionName: "mnemogram-stripe-webhook",
      code: lambda.Code.fromAsset("../lambdas/target/lambda/stripe-webhook"),
      description: "Stripe webhook event processor",
      timeout: cdk.Duration.seconds(60),
      environment: {
        ...lambdaDefaults.environment,
        STRIPE_WEBHOOK_SECRET: process.env.STRIPE_WEBHOOK_SECRET || "",
      },
    });

    // Grant permissions
    memoryBucket.grantRead(searchFn);
    memoryBucket.grantRead(recallFn);
    memoryBucket.grantRead(searchMemoryFn);
    memoryBucket.grantReadWrite(ingestFn);
    memoryBucket.grantReadWrite(manageFn);
    metadataTable.grantReadWriteData(ingestFn);
    metadataTable.grantReadData(searchFn);
    metadataTable.grantReadData(recallFn);
    metadataTable.grantReadData(searchMemoryFn);
    metadataTable.grantReadWriteData(manageFn);
    
    // Grant memories table permissions
    memoriesTable.grantReadWriteData(ingestFn);
    memoriesTable.grantReadData(searchFn);
    memoriesTable.grantReadData(recallFn);
    memoriesTable.grantReadData(searchMemoryFn);
    memoriesTable.grantReadWriteData(manageFn);
    
    // Grant API keys table access to authorizer
    apiKeysTable.grantReadData(authorizerFn);
    
    // Grant DynamoDB permissions for new tables
    subscriptionsTable.grantReadWriteData(stripeWebhookFn);
    subscriptionsTable.grantReadData(manageFn);
    apiKeysTable.grantReadWriteData(manageFn);
    usageTable.grantReadWriteData(manageFn);
    usageTable.grantReadWriteData(ingestFn);
    usageTable.grantReadWriteData(searchFn);
    usageTable.grantReadWriteData(recallFn);
    usageTable.grantReadWriteData(searchMemoryFn);

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

    // Add X-API-Version response header for all responses
    const responseHeaders = {
      "X-API-Version": "1.0"
    };

    // Create v1 API root resource for versioning
    const v1Root = api.root.addResource("v1");

    const authorizer = new apigateway.TokenAuthorizer(this, "JwtAuthorizer", {
      handler: authorizerFn,
      identitySource: "method.request.header.Authorization",
      resultsCacheTtl: cdk.Duration.minutes(5),
    });

    // Routes
    const statusResource = v1Root.addResource("status");
    statusResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(statusFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      {
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );

    const memoriesResource = v1Root.addResource("memories");
    
    // POST /v1/memories - Create memory and get upload URL
    memoriesResource.addMethod(
      "POST",
      new apigateway.LambdaIntegration(ingestFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );
    
    // GET /v1/memories - List user's memories
    memoriesResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(manageFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );
    
    // PUT /v1/memories - Update existing memory (upload content)
    memoriesResource.addMethod(
      "PUT",
      new apigateway.LambdaIntegration(ingestFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );
    
    // DELETE /v1/memories - Delete memory
    memoriesResource.addMethod(
      "DELETE",
      new apigateway.LambdaIntegration(manageFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );

    // POST /v1/memories/{id}/search - Search within specific memory
    const memoryIdResource = memoriesResource.addResource("{id}");
    const memorySearchResource = memoryIdResource.addResource("search");
    memorySearchResource.addMethod(
      "POST",
      new apigateway.LambdaIntegration(searchMemoryFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );

    // GET /v1/search - Search across memories (existing API)
    const searchResource = v1Root.addResource("search");
    searchResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(searchFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );

    // POST /v1/recall - Recall across all memories
    const recallResource = v1Root.addResource("recall");
    recallResource.addMethod(
      "POST",
      new apigateway.LambdaIntegration(recallFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );
    
    // GET /v1/recall - Recall across all memories (existing API)
    recallResource.addMethod(
      "GET",
      new apigateway.LambdaIntegration(recallFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      { 
        authorizer,
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
    );

    // Webhook routes (no auth required) - keep at root level for backward compatibility
    const webhookResource = api.root.addResource("webhook");
    const stripeWebhookResource = webhookResource.addResource("stripe");
    stripeWebhookResource.addMethod(
      "POST",
      new apigateway.LambdaIntegration(stripeWebhookFn, {
        integrationResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": `'1.0'`,
            },
          },
        ],
      }),
      {
        methodResponses: [
          {
            statusCode: "200",
            responseParameters: {
              "method.response.header.X-API-Version": true,
            },
          },
        ],
      }
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

    new cdk.CfnOutput(this, "UserPoolDomainName", {
      value: userPoolDomain.domainName,
      description: "Cognito User Pool Domain Name",
    });

    new cdk.CfnOutput(this, "MemoryBucketName", {
      value: memoryBucket.bucketName,
      description: "S3 bucket for .mv2 files",
    });
  }
}
