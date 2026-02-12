import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as apigateway from "aws-cdk-lib/aws-apigateway";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as cloudwatchActions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as sns from "aws-cdk-lib/aws-sns";
import * as snsSubscriptions from "aws-cdk-lib/aws-sns-subscriptions";
import * as backup from "aws-cdk-lib/aws-backup";
import * as iam from "aws-cdk-lib/aws-iam";
import * as events from "aws-cdk-lib/aws-events";
import * as wafv2 from "aws-cdk-lib/aws-wafv2";
import * as logs from "aws-cdk-lib/aws-logs";
import { Construct } from "constructs";

export interface MnemogramStackProps extends cdk.StackProps {
  stage: string;
}

export class MnemogramStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: MnemogramStackProps) {
    super(scope, id, props);

    const stage = props.stage;

    // ── Storage ──────────────────────────────────────────────────────

    // S3 bucket for .mv2 memory files
    const memoryBucket = new s3.Bucket(this, "MemoryBucket", {
      bucketName: `mnemogram-${stage}-memories-${this.account}-${this.region}`,
      encryption: s3.BucketEncryption.S3_MANAGED,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      versioned: true,
      lifecycleRules: [
        {
          id: "TransitionToIA",
          enabled: true,
          transitions: [
            {
              storageClass: s3.StorageClass.INFREQUENT_ACCESS,
              transitionAfter: cdk.Duration.days(90),
            },
          ],
        },
        {
          id: "TransitionToGlacier",
          enabled: true,
          transitions: [
            {
              storageClass: s3.StorageClass.GLACIER,
              transitionAfter: cdk.Duration.days(365),
            },
          ],
        },
        {
          id: "AbortIncompleteMultipartUploads",
          enabled: true,
          abortIncompleteMultipartUploadAfter: cdk.Duration.days(7),
        },
      ],
    });

    // DynamoDB billing mode based on stage
    const billingMode = stage === "prod" ? dynamodb.BillingMode.PROVISIONED : dynamodb.BillingMode.PAY_PER_REQUEST;
    const readCapacity = stage === "prod" ? 5 : undefined;
    const writeCapacity = stage === "prod" ? 5 : undefined;

    // DynamoDB table for user metadata, API keys, and usage tracking
    const metadataTable = new dynamodb.Table(this, "MetadataTable", {
      tableName: `mnemogram-${stage}-metadata`,
      partitionKey: { name: "pk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "sk", type: dynamodb.AttributeType.STRING },
      billingMode,
      readCapacity,
      writeCapacity,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // DynamoDB table for memories metadata
    const memoriesTable = new dynamodb.Table(this, "MemoriesTable", {
      tableName: `mnemogram-${stage}-memories`,
      partitionKey: { name: "memoryId", type: dynamodb.AttributeType.STRING },
      billingMode,
      readCapacity,
      writeCapacity,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // Add GSI on userId for querying user's memories
    memoriesTable.addGlobalSecondaryIndex({
      indexName: "userId-index",
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
      readCapacity: stage === "prod" ? 2 : undefined,
      writeCapacity: stage === "prod" ? 2 : undefined,
    });

    // DynamoDB table for subscription management
    const subscriptionsTable = new dynamodb.Table(this, "SubscriptionsTable", {
      tableName: `mnemogram-${stage}-subscriptions`,
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
      billingMode,
      readCapacity,
      writeCapacity,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // DynamoDB table for API key management  
    const apiKeysTable = new dynamodb.Table(this, "ApiKeysTable", {
      tableName: `mnemogram-${stage}-api-keys`,
      partitionKey: { name: "keyId", type: dynamodb.AttributeType.STRING },
      billingMode,
      readCapacity,
      writeCapacity,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // Add GSI on userId for API keys
    apiKeysTable.addGlobalSecondaryIndex({
      indexName: "userId-index",
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
      readCapacity: stage === "prod" ? 2 : undefined,
      writeCapacity: stage === "prod" ? 2 : undefined,
    });

    // DynamoDB table for usage tracking
    const usageTable = new dynamodb.Table(this, "UsageTable", {
      tableName: `mnemogram-${stage}-usage`,
      partitionKey: { name: "userId", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "date", type: dynamodb.AttributeType.STRING },
      billingMode,
      readCapacity,
      writeCapacity,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      pointInTimeRecovery: true,
    });

    // ── Auth ─────────────────────────────────────────────────────────

    const userPool = new cognito.UserPool(this, "UserPool", {
      userPoolName: `mnemogram-${stage}-users`,
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
      userPoolClientName: `mnemogram-${stage}-web-client`,
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
        domainPrefix: `mnemogram-${stage}-auth-${this.account}`,
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
      tracing: lambda.Tracing.ACTIVE,
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
      functionName: `mnemogram-${stage}-status`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-status", {
        // During synth without a build, use a dummy path fallback
      }),
      description: "Health check endpoint",
    });

    // Ingest (new POST /memories for S3 pre-signed URL flow)
    const ingestFn = new lambda.Function(this, "IngestFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-ingest`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/ingest"),
      description: "Ingest memory metadata and generate S3 upload URL",
    });

    // Search within a memory (new POST /memories/{id}/search)
    const searchMemoryFn = new lambda.Function(this, "SearchMemoryFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-search-memory`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/search"),
      description: "Search within a specific memory",
    });

    // Search (existing GET /search endpoint for backward compatibility)
    const searchFn = new lambda.Function(this, "SearchFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-search`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-search"),
      description: "Hybrid search over memory files",
    });

    // Recall
    const recallFn = new lambda.Function(this, "RecallFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-recall`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/recall"),
      description: "Broader recall across all user memories",
    });

    // Batch Recall
    const batchRecallFn = new lambda.Function(this, "BatchRecallFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-batch-recall`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/batch-recall"),
      description: "Batch recall across all user memories",
    });

    // Manage
    const manageFn = new lambda.Function(this, "ManageFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-manage`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-manage"),
      description: "CRUD for memory files",
    });

    // Authorizer (updated)
    const authorizerFn = new lambda.Function(this, "AuthorizerFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-authorizer`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/authorizer"),
      description: "JWT/API key custom authorizer",
    });

    // Stripe webhook handler
    const stripeWebhookFn = new lambda.Function(this, "StripeWebhookFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-stripe-webhook`,
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
    memoryBucket.grantRead(batchRecallFn);
    memoryBucket.grantRead(searchMemoryFn);
    memoryBucket.grantReadWrite(ingestFn);
    memoryBucket.grantReadWrite(manageFn);
    metadataTable.grantReadWriteData(ingestFn);
    metadataTable.grantReadData(searchFn);
    metadataTable.grantReadData(recallFn);
    metadataTable.grantReadData(batchRecallFn);
    metadataTable.grantReadData(searchMemoryFn);
    metadataTable.grantReadWriteData(manageFn);
    
    // Grant memories table permissions
    memoriesTable.grantReadWriteData(ingestFn);
    memoriesTable.grantReadData(searchFn);
    memoriesTable.grantReadData(recallFn);
    memoriesTable.grantReadData(batchRecallFn);
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
    usageTable.grantReadWriteData(batchRecallFn);
    usageTable.grantReadWriteData(searchMemoryFn);

    // ── API Gateway ──────────────────────────────────────────────────

    // ── API Gateway Access Logging ──────────────────────────────────

    const apiAccessLogGroup = new logs.LogGroup(this, "ApiAccessLogGroup", {
      logGroupName: `/aws/apigateway/mnemogram-${stage}-access-logs`,
      retention: logs.RetentionDays.ONE_WEEK,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    const api = new apigateway.RestApi(this, "MnemogramApi", {
      restApiName: `mnemogram-${stage}-api`,
      description: "Mnemogram REST API",
      deployOptions: {
        stageName: "v1",
        throttlingRateLimit: 100,
        throttlingBurstLimit: 200,
        accessLogDestination: new apigateway.LogGroupLogDestination(apiAccessLogGroup),
        accessLogFormat: apigateway.AccessLogFormat.jsonWithStandardFields({
          caller: false,
          httpMethod: true,
          ip: true,
          protocol: true,
          requestTime: true,
          resourcePath: true,
          responseLength: true,
          status: true,
          user: true,
        }),
      },
      defaultCorsPreflightOptions: {
        allowOrigins: apigateway.Cors.ALL_ORIGINS,
        allowMethods: apigateway.Cors.ALL_METHODS,
      },
    });

    // ── AWS WAF WebACL ──────────────────────────────────────────────

    const webAcl = new wafv2.CfnWebACL(this, "MnemogramWebAcl", {
      name: `mnemogram-${stage}-web-acl`,
      description: "WAF rules for Mnemogram API Gateway",
      scope: "REGIONAL",
      defaultAction: { allow: {} },
      visibilityConfig: {
        sampledRequestsEnabled: true,
        cloudWatchMetricsEnabled: true,
        metricName: `mnemogram-${stage}-web-acl`,
      },
      rules: [
        // Rate limiting: 1000 requests per 5 min per IP
        {
          name: "RateLimitRule",
          priority: 1,
          statement: {
            rateBasedStatement: {
              limit: 1000,
              aggregateKeyType: "IP",
            },
          },
          action: { block: {} },
          visibilityConfig: {
            sampledRequestsEnabled: true,
            cloudWatchMetricsEnabled: true,
            metricName: `mnemogram-${stage}-rate-limit`,
          },
        },
        // AWS managed rule: Common rule set
        {
          name: "AWSManagedRulesCommonRuleSet",
          priority: 2,
          overrideAction: { none: {} },
          statement: {
            managedRuleGroupStatement: {
              vendorName: "AWS",
              name: "AWSManagedRulesCommonRuleSet",
              excludedRules: [], // Can add exclusions if needed
            },
          },
          visibilityConfig: {
            sampledRequestsEnabled: true,
            cloudWatchMetricsEnabled: true,
            metricName: `mnemogram-${stage}-common-rules`,
          },
        },
        // AWS managed rule: Known bad inputs
        {
          name: "AWSManagedRulesKnownBadInputsRuleSet",
          priority: 3,
          overrideAction: { none: {} },
          statement: {
            managedRuleGroupStatement: {
              vendorName: "AWS",
              name: "AWSManagedRulesKnownBadInputsRuleSet",
              excludedRules: [], // Can add exclusions if needed
            },
          },
          visibilityConfig: {
            sampledRequestsEnabled: true,
            cloudWatchMetricsEnabled: true,
            metricName: `mnemogram-${stage}-bad-inputs`,
          },
        },
        // Block requests from known bad IPs (placeholder - can be configured)
        {
          name: "BlockKnownBadIPs",
          priority: 4,
          statement: {
            ipSetReferenceStatement: {
              arn: `arn:aws:wafv2:${this.region}:${this.account}:regional/ipset/mnemogram-${stage}-blocked-ips/placeholder`,
            },
          },
          action: { block: {} },
          visibilityConfig: {
            sampledRequestsEnabled: true,
            cloudWatchMetricsEnabled: true,
            metricName: `mnemogram-${stage}-blocked-ips`,
          },
        },
        // Geo-restriction structure (ready but not enabled)
        {
          name: "GeoRestrictRule",
          priority: 5,
          statement: {
            geoMatchStatement: {
              countryCodes: ["XX"], // Placeholder - change to actual countries to block
            },
          },
          action: { count: {} }, // Count only - not blocking yet
          visibilityConfig: {
            sampledRequestsEnabled: true,
            cloudWatchMetricsEnabled: true,
            metricName: `mnemogram-${stage}-geo-block`,
          },
        },
      ],
    });

    // Create IP Set for blocking known bad IPs (empty by default)
    const blockedIpSet = new wafv2.CfnIPSet(this, "BlockedIPSet", {
      name: `mnemogram-${stage}-blocked-ips`,
      description: "IP addresses to block",
      scope: "REGIONAL",
      ipAddressVersion: "IPV4",
      addresses: [], // Empty by default - can be populated as needed
    });

    // Update the IP set reference in the rule to use the actual ARN
    // Note: This is a workaround for the TypeScript typing issue
    const webAclRules = webAcl.rules as any[];
    if (webAclRules && webAclRules[3]) {
      webAclRules[3].statement.ipSetReferenceStatement.arn = blockedIpSet.attrArn;
    }

    // Associate WAF WebACL with API Gateway
    new wafv2.CfnWebACLAssociation(this, "ApiGatewayWebAclAssociation", {
      resourceArn: `arn:aws:apigateway:${this.region}::/restapis/${api.restApiId}/stages/v1`,
      webAclArn: webAcl.attrArn,
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

    // POST /v1/batch-recall - Batch recall across all memories
    const batchRecallResource = v1Root.addResource("batch-recall");
    batchRecallResource.addMethod(
      "POST",
      new apigateway.LambdaIntegration(batchRecallFn, {
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

    // ── CloudWatch Monitoring ───────────────────────────────────────

    // SNS topic for alarm notifications
    const alertsTopic = new sns.Topic(this, "AlertsTopic", {
      topicName: `mnemogram-${stage}-alerts`,
      displayName: `Mnemogram ${stage.toUpperCase()} Alerts`,
    });

    // Subscribe placeholder email (can be updated later)
    alertsTopic.addSubscription(new snsSubscriptions.EmailSubscription("alerts@mnemogram.com"));

    // CloudWatch Alarms
    
    // Lambda error alarm for all functions
    const lambdaFunctions = [statusFn, ingestFn, searchMemoryFn, searchFn, recallFn, batchRecallFn, manageFn, authorizerFn, stripeWebhookFn];
    
    lambdaFunctions.forEach((lambdaFunction, index) => {
      new cloudwatch.Alarm(this, `LambdaErrorAlarm${index}`, {
        alarmName: `${lambdaFunction.functionName}-errors`,
        alarmDescription: `Lambda errors for ${lambdaFunction.functionName}`,
        metric: lambdaFunction.metricErrors({
          period: cdk.Duration.minutes(5),
        }),
        threshold: 5,
        evaluationPeriods: 1,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      }).addAlarmAction(new cloudwatchActions.SnsAction(alertsTopic));
    });

    // API Gateway 5xx errors
    new cloudwatch.Alarm(this, "ApiGateway5xxAlarm", {
      alarmName: `mnemogram-${stage}-api-5xx-errors`,
      alarmDescription: `API Gateway 5xx errors for ${stage}`,
      metric: new cloudwatch.Metric({
        namespace: "AWS/ApiGateway",
        metricName: "5XXError",
        dimensionsMap: {
          ApiName: api.restApiName,
        },
        statistic: "Sum",
        period: cdk.Duration.minutes(5),
      }),
      threshold: 10,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    }).addAlarmAction(new cloudwatchActions.SnsAction(alertsTopic));

    // DynamoDB throttle alarms for all tables
    const dynamoTables = [metadataTable, memoriesTable, subscriptionsTable, apiKeysTable, usageTable];
    
    dynamoTables.forEach((table, index) => {
      new cloudwatch.Alarm(this, `DynamoThrottleAlarm${index}`, {
        alarmName: `${table.tableName}-throttles`,
        alarmDescription: `DynamoDB throttles for ${table.tableName}`,
        metric: table.metricThrottledRequests({
          period: cdk.Duration.minutes(5),
        }),
        threshold: 1,
        evaluationPeriods: 1,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      }).addAlarmAction(new cloudwatchActions.SnsAction(alertsTopic));
    });

    // CloudWatch Dashboard
    const dashboard = new cloudwatch.Dashboard(this, "MnemogramDashboard", {
      dashboardName: `mnemogram-${stage}-dashboard`,
    });

    // Lambda metrics widgets
    dashboard.addWidgets(
      new cloudwatch.GraphWidget({
        title: "Lambda Invocations",
        left: lambdaFunctions.map(fn => fn.metricInvocations()),
        width: 12,
        height: 6,
      }),
      new cloudwatch.GraphWidget({
        title: "Lambda Duration",
        left: lambdaFunctions.map(fn => fn.metricDuration()),
        width: 12,
        height: 6,
      })
    );

    dashboard.addWidgets(
      new cloudwatch.GraphWidget({
        title: "Lambda Errors",
        left: lambdaFunctions.map(fn => fn.metricErrors()),
        width: 12,
        height: 6,
      }),
      new cloudwatch.GraphWidget({
        title: "API Gateway Latency",
        left: [
          new cloudwatch.Metric({
            namespace: "AWS/ApiGateway",
            metricName: "Latency",
            dimensionsMap: {
              ApiName: api.restApiName,
            },
            statistic: "Average",
          }),
        ],
        width: 12,
        height: 6,
      })
    );

    // DynamoDB metrics
    dashboard.addWidgets(
      new cloudwatch.GraphWidget({
        title: "DynamoDB Consumed Read Capacity",
        left: dynamoTables.map(table => table.metricConsumedReadCapacityUnits()),
        width: 12,
        height: 6,
      }),
      new cloudwatch.GraphWidget({
        title: "DynamoDB Consumed Write Capacity",
        left: dynamoTables.map(table => table.metricConsumedWriteCapacityUnits()),
        width: 12,
        height: 6,
      })
    );

    // ── AWS Backup ──────────────────────────────────────────────────

    // Create backup vault
    const backupVault = new backup.BackupVault(this, "MnemogramBackupVault", {
      backupVaultName: `mnemogram-${stage}-backup-vault`,
      encryptionKey: undefined, // Use default AWS managed key
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    // Create backup plan for daily DynamoDB backups
    const backupPlan = new backup.BackupPlan(this, "MnemogramBackupPlan", {
      backupPlanName: `mnemogram-${stage}-backup-plan`,
      backupVault,
      backupPlanRules: [
        new backup.BackupPlanRule({
          ruleName: "DailyBackups",
          scheduleExpression: events.Schedule.cron({
            minute: "0",
            hour: "2", // 2 AM UTC
          }),
          deleteAfter: cdk.Duration.days(30), // Keep backups for 30 days
          startWindow: cdk.Duration.hours(1),
          completionWindow: cdk.Duration.hours(8),
        }),
      ],
    });

    // Create backup role with necessary permissions
    const backupRole = new iam.Role(this, "BackupRole", {
      roleName: `mnemogram-${stage}-backup-role`,
      assumedBy: new iam.ServicePrincipal("backup.amazonaws.com"),
      managedPolicies: [
        iam.ManagedPolicy.fromAwsManagedPolicyName("service-role/AWSBackupServiceRolePolicyForBackup"),
        iam.ManagedPolicy.fromAwsManagedPolicyName("service-role/AWSBackupServiceRolePolicyForRestores"),
      ],
    });

    // Add all DynamoDB tables to backup selection
    backupPlan.addSelection("DynamoDBBackupSelection", {
      resources: [
        backup.BackupResource.fromDynamoDbTable(metadataTable),
        backup.BackupResource.fromDynamoDbTable(memoriesTable),
        backup.BackupResource.fromDynamoDbTable(subscriptionsTable),
        backup.BackupResource.fromDynamoDbTable(apiKeysTable),
        backup.BackupResource.fromDynamoDbTable(usageTable),
      ],
      role: backupRole,
    });

    // Tag all tables to include them in backup
    const backupTag = { BackupEnabled: "true" };
    cdk.Tags.of(metadataTable).add("BackupEnabled", "true");
    cdk.Tags.of(memoriesTable).add("BackupEnabled", "true");
    cdk.Tags.of(subscriptionsTable).add("BackupEnabled", "true");
    cdk.Tags.of(apiKeysTable).add("BackupEnabled", "true");
    cdk.Tags.of(usageTable).add("BackupEnabled", "true");

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
