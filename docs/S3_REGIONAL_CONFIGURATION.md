# Per-Region S3 Bucket Configuration

## Overview

Mnemogram uses **per-region S3 buckets** for .mv2 memory file storage without cross-region replication. This design prioritizes cost optimization and data locality while maintaining performance.

## S3 Bucket Strategy

### Regional Isolation
- **Bucket naming**: `mnemogram-{stage}-memories-{account}-{region}`
- **Data locality**: User data stays in selected region
- **No replication**: Eliminates cross-region transfer costs
- **Independent lifecycle**: Each region manages its own data

### Example Bucket Names
```
Production:
- mnemogram-prod-memories-369292120314-us-east-1
- mnemogram-prod-memories-369292120314-us-west-2
- mnemogram-prod-memories-369292120314-us-central-1

Development:
- mnemogram-dev-memories-369292120314-us-east-1
- mnemogram-dev-memories-369292120314-us-west-2
```

## Cost Optimization Features

### 1. Intelligent Tiering (ENABLED)
```typescript
intelligentTieringConfigurations: [
  {
    id: "IntelligentTiering",
    status: s3.IntelligentTieringStatus.ENABLED,
    optionalFields: [s3.IntelligentTieringOptionalFields.BUCKET_KEY_STATUS],
  },
]
```

**Benefits**:
- Automatic optimization between Frequent and Infrequent Access
- No retrieval fees for Standard-IA transitions
- Monitoring and automation fee: $0.0025 per 1,000 objects/month
- Typical savings: 20-30% for mixed access patterns

### 2. Lifecycle Rules (OPTIMIZED)

#### Infrequent Access Transition
```typescript
transitionAfter: cdk.Duration.days(30) // Reduced from 90 days
```
- **Cost savings**: ~50% for files accessed <1x/month
- **Retrieval cost**: $0.01/GB (acceptable for occasional access)

#### Glacier Transition  
```typescript
transitionAfter: cdk.Duration.days(180) // Reduced from 365 days
```
- **Cost savings**: ~77% for archival storage
- **Retrieval time**: 1-5 minutes (expedited)
- **Use case**: Historical memories, backup retention

#### Deep Archive Transition
```typescript
transitionAfter: cdk.Duration.days(365) // New tier for long-term storage
```
- **Cost savings**: ~95% for long-term archival
- **Retrieval time**: 12 hours (standard)
- **Use case**: Compliance, legal hold, disaster recovery

#### Version Management
```typescript
noncurrentVersionTransitions: [
  {
    storageClass: s3.StorageClass.INFREQUENT_ACCESS,
    transitionAfter: cdk.Duration.days(7),
  },
  {
    storageClass: s3.StorageClass.GLACIER,
    transitionAfter: cdk.Duration.days(30),
  },
],
noncurrentVersionExpiration: cdk.Duration.days(90)
```
- **Prevents version sprawl**: Old versions automatically archived
- **Cost control**: Limits storage of unused versions
- **Recovery window**: 90 days for accidental deletion protection

## Security Configuration

### Encryption
- **Method**: S3-managed encryption (SSE-S3)
- **Key management**: AWS handles key rotation
- **Cost**: No additional charges
- **Compliance**: Encryption at rest and in transit

### Access Control
- **Public access**: Completely blocked
- **IAM integration**: Lambda functions granted specific bucket permissions
- **Cross-region access**: Each Lambda only accesses regional bucket

```typescript
blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL
```

## Storage Classes and Cost Breakdown

### Per-Region Monthly Costs (100GB example)

#### Standard Tier (0-30 days)
```
Storage:    100GB × $0.023/GB = $2.30
Requests:   10K PUT × $0.0005/K = $0.05
            100K GET × $0.0004/K = $0.40
Total:      $2.75/month
```

#### Standard-IA (30-180 days) 
```
Storage:    100GB × $0.0125/GB = $1.25  
Retrieval:  10GB × $0.01/GB = $0.10
Total:      $1.35/month (51% savings)
```

#### Glacier (180-365 days)
```
Storage:    100GB × $0.004/GB = $0.40
Retrieval:  1GB × $0.03/GB = $0.03  
Total:      $0.43/month (84% savings)
```

#### Deep Archive (365+ days)
```
Storage:    100GB × $0.00099/GB = $0.099
Retrieval:  Rare access
Total:      $0.10/month (96% savings)
```

## Regional Data Flow

### User Memory Storage
1. **Upload**: Client → Regional API → Regional S3 bucket
2. **Processing**: Regional Lambda → Regional S3 bucket  
3. **Retrieval**: Regional Lambda ← Regional S3 bucket → Client
4. **No cross-region**: All operations within selected region

### Backup and Disaster Recovery
- **Regional backup**: AWS Backup service per region
- **No cross-region replication**: Cost-prohibitive for user data
- **User responsibility**: Export/import between regions if needed
- **Business continuity**: Regional redundancy within AZ

## Performance Characteristics

### Latency Benefits
- **Regional access**: <50ms for same-region operations
- **No cross-region**: Eliminates 100-200ms cross-region latency
- **CloudFront integration**: CDN caching for frequently accessed files

### Throughput
- **Upload**: Direct to regional S3 (no bottlenecks)
- **Download**: Regional Lambda → S3 (optimized path)
- **Parallel processing**: Independent regional scaling

## Monitoring and Observability

### CloudWatch Metrics (Per Region)
- **Storage utilization**: Total GB and object count
- **Request metrics**: PUT/GET/DELETE operations
- **Cost tracking**: Storage class distribution
- **Lifecycle transitions**: Monitoring automated tiering

### Cost Allocation Tags
```typescript
Tags: {
  'Project': 'Mnemogram',
  'Stage': props.stage,
  'Region': this.region,
  'Component': 'MemoryStorage'
}
```

## Operational Procedures

### Data Migration (Cross-Region)
```bash
# If user needs to change regions (rare)
aws s3 sync s3://source-region-bucket/user-id/ s3://target-region-bucket/user-id/
```

### Backup Verification
```bash
# Check backup status per region  
aws backup describe-backup-jobs --region us-east-1
aws backup describe-backup-jobs --region us-west-2
```

### Cost Analysis
```bash
# Monthly storage cost per region
aws s3api list-objects-v2 --bucket mnemogram-prod-memories-account-region --query 'sum(Contents[].Size)'
```

## Best Practices

### For Users
1. **Stick to chosen region**: Avoid cross-region operations
2. **Regular cleanup**: Delete unused memories to optimize costs
3. **Access patterns**: Consider Intelligent Tiering benefits

### For Operations
1. **Regional monitoring**: Independent alerting per region
2. **Cost tracking**: Monitor lifecycle transition effectiveness  
3. **Performance tuning**: Optimize based on regional usage patterns

## Migration Path (Future)

### If Cross-Region Features Needed
1. **Federation API**: Query multiple regional buckets
2. **Optional replication**: User-controlled backup to secondary region
3. **Metadata sync**: Light-weight cross-region memory index

### Cost-Benefit Analysis
- **Current approach**: $25/month per region for 100GB
- **With replication**: +$15/month per region for cross-region transfer
- **Trade-off**: 60% cost increase for redundancy

## Conclusion

The per-region S3 bucket approach delivers:

✅ **Cost Optimization**: 20-30% savings through Intelligent Tiering and lifecycle rules  
✅ **Performance**: <50ms regional latency, no cross-region delays  
✅ **Simplicity**: Independent regional operations, no complex replication  
✅ **Scalability**: Each region scales independently based on demand  
✅ **Compliance**: Regional data residency requirements satisfied

This design supports the overall Mnemogram goal of cost-effective, high-performance AI memory service with geographical optimization.