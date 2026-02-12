import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as origins from "aws-cdk-lib/aws-cloudfront-origins";
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
      architecture: lambda.Architecture.ARM_64, // MNEM-150: Graviton ARM
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
      ephemeralStorageSize: cdk.Size.gibibytes(1), // MNEM-151: 1GB for .mv2 caching
    });

    // Search (existing GET /search endpoint for backward compatibility)
    const searchFn = new lambda.Function(this, "SearchFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-search`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/api-search"),
      description: "Hybrid search over memory files",
      ephemeralStorageSize: cdk.Size.gibibytes(1), // MNEM-151: 1GB for .mv2 caching
    });

    // Recall
    const recallFn = new lambda.Function(this, "RecallFn", {
      ...lambdaDefaults,
      functionName: `mnemogram-${stage}-recall`,
      code: lambda.Code.fromAsset("../lambdas/target/lambda/recall"),
      description: "Broader recall across all user memories",
      ephemeralStorageSize: cdk.Size.gibibytes(1), // MNEM-151: 1GB for .mv2 caching
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

    // ── Lambda Function URLs ────────────────────────────────────────

    // Create Function URLs for all Lambda functions
    const statusFnUrl = statusFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // Public health check
    });

    const ingestFnUrl = ingestFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // Auth handled in function
      cors: {
        allowCredentials: false,
        allowedHeaders: ["Content-Type", "Authorization", "X-API-Key", "X-API-Version"],
        allowedMethods: [lambda.HttpMethod.POST, lambda.HttpMethod.PUT, lambda.HttpMethod.OPTIONS],
        allowedOrigins: ["*"],
        maxAge: cdk.Duration.days(1),
      },
    });

    const searchMemoryFnUrl = searchMemoryFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // Auth handled in function
      cors: {
        allowCredentials: false,
        allowedHeaders: ["Content-Type", "Authorization", "X-API-Key", "X-API-Version"],
        allowedMethods: [lambda.HttpMethod.POST, lambda.HttpMethod.OPTIONS],
        allowedOrigins: ["*"],
        maxAge: cdk.Duration.days(1),
      },
    });

    const searchFnUrl = searchFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // Auth handled in function
      cors: {
        allowCredentials: false,
        allowedHeaders: ["Content-Type", "Authorization", "X-API-Key", "X-API-Version"],
        allowedMethods: [lambda.HttpMethod.GET, lambda.HttpMethod.OPTIONS],
        allowedOrigins: ["*"],
        maxAge: cdk.Duration.days(1),
      },
    });

    const recallFnUrl = recallFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // Auth handled in function
      cors: {
        allowCredentials: false,
        allowedHeaders: ["Content-Type", "Authorization", "X-API-Key", "X-API-Version"],
        allowedMethods: [lambda.HttpMethod.GET, lambda.HttpMethod.POST, lambda.HttpMethod.OPTIONS],
        allowedOrigins: ["*"],
        maxAge: cdk.Duration.days(1),
      },
    });

    const manageFnUrl = manageFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // Auth handled in function
      cors: {
        allowCredentials: false,
        allowedHeaders: ["Content-Type", "Authorization", "X-API-Key", "X-API-Version"],
        allowedMethods: [lambda.HttpMethod.GET, lambda.HttpMethod.DELETE, lambda.HttpMethod.OPTIONS],
        allowedOrigins: ["*"],
        maxAge: cdk.Duration.days(1),
      },
    });

    const stripeWebhookFnUrl = stripeWebhookFn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE, // No auth for webhooks
      cors: {
        allowCredentials: false,
        allowedHeaders: ["Content-Type", "Stripe-Signature"],
        allowedMethods: [lambda.HttpMethod.POST, lambda.HttpMethod.OPTIONS],
        allowedOrigins: ["*"],
        maxAge: cdk.Duration.days(1),
      },
    });

    // ── CloudFront Distribution ─────────────────────────────────────

    // Create CloudFront distribution to route to Lambda Function URLs
    const distribution = new cloudfront.Distribution(this, "MnemogramDistribution", {
      comment: `Mnemogram ${stage} API Distribution`,
      defaultBehavior: {
        // Default to status function for health checks
        origin: new origins.FunctionUrlOrigin(statusFnUrl),
        allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
        cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
        cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
        originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        compress: true,
      },
      additionalBehaviors: {
        // v1/status -> status function
        "v1/status": {
          origin: new origins.FunctionUrlOrigin(statusFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_GET_HEAD_OPTIONS,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },
        
        // v1/memories (POST/PUT) -> ingest function
        "v1/memories": {
          origin: new origins.FunctionUrlOrigin(ingestFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },
        
        // v1/memories/* (GET/DELETE for management) -> manage function  
        "v1/memories/*": {
          origin: new origins.FunctionUrlOrigin(manageFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },

        // v1/memories/*/search -> search memory function
        "v1/memories/*/search": {
          origin: new origins.FunctionUrlOrigin(searchMemoryFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },

        // v1/search -> search function
        "v1/search": {
          origin: new origins.FunctionUrlOrigin(searchFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },

        // v1/recall -> recall function
        "v1/recall": {
          origin: new origins.FunctionUrlOrigin(recallFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },

        // webhook/stripe -> stripe webhook function
        "webhook/stripe": {
          origin: new origins.FunctionUrlOrigin(stripeWebhookFnUrl),
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachedMethods: cloudfront.CachedMethods.CACHE_GET_HEAD_OPTIONS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        },
      },
      priceClass: cloudfront.PriceClass.PRICE_CLASS_100, // US, Canada, Europe only
      enabled: true,
      httpVersion: cloudfront.HttpVersion.HTTP2_AND_3,
    });

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
    const lambdaFunctions = [statusFn, ingestFn, searchMemoryFn, searchFn, recallFn, manageFn, authorizerFn, stripeWebhookFn];
    
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

    // CloudFront error rate alarm (replacing API Gateway 5xx errors)
    new cloudwatch.Alarm(this, "CloudFront4xxErrorAlarm", {
      alarmName: `mnemogram-${stage}-cloudfront-4xx-errors`,
      alarmDescription: `CloudFront 4xx errors for ${stage}`,
      metric: new cloudwatch.Metric({
        namespace: "AWS/CloudFront",
        metricName: "4xxErrorRate",
        dimensionsMap: {
          DistributionId: distribution.distributionId,
        },
        statistic: "Sum",
        period: cdk.Duration.minutes(5),
      }),
      threshold: 10, // Percentage
      evaluationPeriods: 2,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    }).addAlarmAction(new cloudwatchActions.SnsAction(alertsTopic));

    new cloudwatch.Alarm(this, "CloudFront5xxErrorAlarm", {
      alarmName: `mnemogram-${stage}-cloudfront-5xx-errors`,
      alarmDescription: `CloudFront 5xx errors for ${stage}`,
      metric: new cloudwatch.Metric({
        namespace: "AWS/CloudFront",
        metricName: "5xxErrorRate",
        dimensionsMap: {
          DistributionId: distribution.distributionId,
        },
        statistic: "Sum",
        period: cdk.Duration.minutes(5),
      }),
      threshold: 5, // Percentage
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
        title: "CloudFront Requests",
        left: [
          new cloudwatch.Metric({
            namespace: "AWS/CloudFront",
            metricName: "Requests",
            dimensionsMap: {
              DistributionId: distribution.distributionId,
            },
            statistic: "Sum",
          }),
        ],
        width: 12,
        height: 6,
      })
    );

    // Add CloudFront error rate widgets
    dashboard.addWidgets(
      new cloudwatch.GraphWidget({
        title: "CloudFront Error Rates",
        left: [
          new cloudwatch.Metric({
            namespace: "AWS/CloudFront",
            metricName: "4xxErrorRate",
            dimensionsMap: {
              DistributionId: distribution.distributionId,
            },
            statistic: "Average",
          }),
          new cloudwatch.Metric({
            namespace: "AWS/CloudFront",
            metricName: "5xxErrorRate",
            dimensionsMap: {
              DistributionId: distribution.distributionId,
            },
            statistic: "Average",
          }),
        ],
        width: 12,
        height: 6,
      }),
      new cloudwatch.GraphWidget({
        title: "CloudFront Cache Hit Rate",
        left: [
          new cloudwatch.Metric({
            namespace: "AWS/CloudFront",
            metricName: "CacheHitRate",
            dimensionsMap: {
              DistributionId: distribution.distributionId,
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

    new cdk.CfnOutput(this, "CloudFrontDomainName", {
      value: distribution.distributionDomainName,
      description: "CloudFront distribution domain name",
    });

    new cdk.CfnOutput(this, "CloudFrontDistributionId", {
      value: distribution.distributionId,
      description: "CloudFront distribution ID",
    });

    // Export individual Function URLs for debugging/testing
    new cdk.CfnOutput(this, "StatusFunctionUrl", {
      value: statusFnUrl.url,
      description: "Status function URL",
    });

    new cdk.CfnOutput(this, "IngestFunctionUrl", {
      value: ingestFnUrl.url,
      description: "Ingest function URL",
    });

    new cdk.CfnOutput(this, "SearchFunctionUrl", {
      value: searchFnUrl.url,
      description: "Search function URL",
    });

    new cdk.CfnOutput(this, "SearchMemoryFunctionUrl", {
      value: searchMemoryFnUrl.url,
      description: "Search memory function URL",
    });

    new cdk.CfnOutput(this, "RecallFunctionUrl", {
      value: recallFnUrl.url,
      description: "Recall function URL",
    });

    new cdk.CfnOutput(this, "ManageFunctionUrl", {
      value: manageFnUrl.url,
      description: "Manage function URL",
    });

    new cdk.CfnOutput(this, "StripeWebhookFunctionUrl", {
      value: stripeWebhookFnUrl.url,
      description: "Stripe webhook function URL",
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
