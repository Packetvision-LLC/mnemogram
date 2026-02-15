# Multi-Region Architecture - Simplified Approach

## Overview

Mnemogram uses a **simplified multi-region architecture** with independent regional stacks. Each region operates autonomously with its own data and infrastructure, optimizing for cost efficiency and operational simplicity.

## Architecture Principles

### 1. Independent Regional Stacks
- Each region deploys the complete Mnemogram stack independently
- No cross-region dependencies or synchronization
- Region-specific DynamoDB tables and S3 buckets
- Eliminates complex Global Tables setup and associated costs

### 2. Per-Region Data Isolation
- User data remains in the selected region
- No cross-region replication of memory files (.mv2)
- DynamoDB tables: `mnemogram-{stage}-{table}` per region
- S3 buckets: `mnemogram-{stage}-memories-{account}-{region}`

### 3. Client-Side Region Selection
- ClawHub skill auto-detects fastest endpoint on first use
- Users/applications select region and stick with it
- SDK/API calls go directly to chosen regional endpoint
- No global load balancing or routing complexity

## Regional Endpoints

### Production
- **us-east-1**: `https://api.mnemogram.com` (primary)
- **us-west-2**: `https://api-west.mnemogram.com`
- **us-central-1**: `https://api-central.mnemogram.com`

### Development
- **us-east-1**: `https://api-dev.mnemogram.com`
- **us-west-2**: `https://api-dev-west.mnemogram.com`

## Cost Benefits

This simplified approach reduces costs by:
- **No Global Tables**: Eliminates cross-region replication charges
- **No Route 53 routing**: Saves on DNS query costs  
- **Regional data locality**: Minimizes data transfer costs
- **Independent scaling**: Each region scales based on local demand

## Deployment

### Single Region (existing)
```bash
# Deploy to default region
make deploy STAGE=dev

# Deploy to specific region
make deploy-region STAGE=prod REGION=us-west-2
```

### Multi-Region (new)
```bash
# Deploy to all configured regions
make deploy-multiregion STAGE=prod
make deploy-multiregion STAGE=dev

# Preview multi-region deployment
make synth-multiregion STAGE=prod
```

## Infrastructure Components

Each regional deployment includes:

### Storage
- **S3 bucket**: Regional memory file storage (.mv2 files)
- **DynamoDB tables** (per region, no Global Tables):
  - `mnemogram-{stage}-metadata`: User metadata and API keys
  - `mnemogram-{stage}-memories`: Memory metadata and indexing
  - `mnemogram-{stage}-subscriptions`: User subscriptions
  - `mnemogram-{stage}-api-keys`: API key management
  - `mnemogram-{stage}-usage`: Usage tracking and billing

### Compute
- **Lambda functions**: All API handlers and background processors
- **API Gateway**: Regional REST API endpoint
- **SQS queues**: Background processing queues

### Security & Auth
- **Cognito User Pool**: Regional user authentication
- **WAF**: Regional web application firewall
- **IAM roles**: Lambda execution and service roles

## Regional Stack Configuration

### Production Regions (mnemogram-prod-multiregion.ts)
- `MnemogramStack-prod-east1` (us-east-1)
- `MnemogramStack-prod-west2` (us-west-2) 
- `MnemogramStack-prod-central1` (us-central-1)

### Development Regions (mnemogram-dev-multiregion.ts)
- `MnemogramStack-dev-east1` (us-east-1)
- `MnemogramStack-dev-west2` (us-west-2)

## Migration from Global Tables

This simplified architecture represents a **design choice** rather than a migration:
- Never implemented Global Tables (cost prohibitive)
- Each region operates independently from day 1
- User data lives in single region of their choice
- No data migration needed between regions

## Operational Benefits

### Simplicity
- Standard CDK deployment per region
- No complex cross-region orchestration
- Regional monitoring and alerting
- Simplified backup and disaster recovery

### Performance  
- Users connect to geographically closest region
- No cross-region latency for data access
- Regional caching strategies more effective

### Reliability
- Regional outages don't affect other regions
- Independent scaling and capacity planning
- Simplified troubleshooting and debugging

## Client Integration

Applications should:
1. **Auto-detect fastest region** (ClawHub skill does this)
2. **Cache region choice** for subsequent API calls
3. **Stick to chosen region** for data consistency
4. **Handle regional failures** by retrying with different region

Example SDK usage:
```javascript
const client = new MnemogramClient({
  region: 'us-west-2', // or auto-detected
  apiKey: 'your-key'
});
```

## Future Considerations

### If Cross-Region Features Needed
- **Federation layer**: API that queries multiple regions
- **Data replication**: Optional user-controlled backup to secondary region
- **Global directory**: Lightweight service for user→region mapping

### Cost Optimization Opportunities
- **Graviton ARM Lambdas**: 20% cost savings for compute
- **Reserved DynamoDB capacity**: For predictable workloads
- **S3 Intelligent Tiering**: Automatic storage class optimization