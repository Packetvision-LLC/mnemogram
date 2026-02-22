# Export Architecture Decision: Sync vs Async .mv2 File Generation

**Date:** 2026-02-20  
**Author:** Ralph (Autonomous Coding Agent)  
**Ticket:** MNEM-225  

## Problem Statement

Need to determine the optimal architecture for Export-to-MV2 feature that allows users to download their memory data as local .mv2 files. The core decision is between real-time Lambda export vs background processing for .mv2 file generation.

## Context

The Mnemogram platform is migrating from MemVid to S3 Vectors for improved scalability. To maintain the "no cloud lock-in" promise, users must be able to export their data as .mv2 files. The architecture decision impacts user experience, system reliability, and implementation complexity.

## Key Technical Constraints

### Lambda Limitations
- **Timeout:** 15-minute maximum execution time
- **Memory:** 10GB maximum memory allocation 
- **Storage:** 10GB ephemeral storage (/tmp)
- **Concurrent executions:** 1000 default (can be increased)

### Data Characteristics
- **User datasets:** Highly variable (10MB - 50GB+)
- **Vector data:** Dense numerical arrays requiring processing
- **Metadata:** JSON structures with relationships
- **File format:** Binary .mv2 format with compression

## Architecture Options

### Option 1: Synchronous Lambda Export

**Implementation:**
```
User Request → Lambda → S3 Vector Query → .mv2 Generation → S3 Upload → Presigned URL
```

**Pros:**
- Immediate user feedback
- Simple implementation
- No additional infrastructure
- Direct user experience (click → download)
- Error handling is immediate

**Cons:**
- Limited to datasets that can be processed within 15 minutes
- Memory constraints for large datasets
- User must wait for processing (blocking UX)
- Timeout risk for large exports
- No resumability on failure

**Best for:** Small to medium datasets (<1GB, <100K memories)

### Option 2: Asynchronous Background Processing

**Implementation:**
```
User Request → Queue Job → Background Worker → S3 Vector Query → .mv2 Generation → 
S3 Upload → Email/UI Notification → User Download
```

**Pros:**
- Handles datasets of any size
- Non-blocking user experience
- Resumable processing on failure
- Better resource utilization
- Can batch multiple exports
- Progress tracking possible

**Cons:**
- More complex infrastructure (SQS, background workers)
- Delayed user gratification
- Additional monitoring required
- Email dependency for notifications
- Storage costs for completed exports

**Best for:** Large datasets (>1GB, >100K memories)

## Decision Criteria Analysis

### 1. User Experience Requirements
- **Immediate feedback:** Sync wins for small datasets
- **Large dataset support:** Async required for enterprise users
- **Reliability:** Async provides better reliability for large exports

### 2. Technical Feasibility
- **Lambda constraints:** Sync limited by timeout/memory
- **Scalability:** Async scales better
- **Error recovery:** Async provides better error handling

### 3. Data Size Distribution (Estimated)
- **90% of users:** <500MB datasets (sync feasible)
- **8% of users:** 500MB-5GB datasets (sync risky)
- **2% of users:** >5GB datasets (sync impossible)

### 4. Implementation Complexity
- **Sync:** Low complexity, faster to implement
- **Async:** Medium complexity, requires additional infrastructure

## Recommended Architecture: Hybrid Approach

### Implementation Strategy

**Phase 1: Intelligent Routing**
```typescript
export async function exportRequest(userId: string) {
  const dataSize = await estimateUserDataSize(userId);
  
  if (dataSize < SYNC_THRESHOLD) {
    return syncExport(userId);
  } else {
    return asyncExport(userId);
  }
}
```

**Thresholds:**
- **Sync threshold:** 500MB or 50K memories (whichever is lower)
- **Async fallback:** Everything above threshold

### Phase 1: Sync Export (Quick Wins)

**Lambda Function:** `export-sync`
- **Memory:** 3GB allocation
- **Timeout:** 10 minutes
- **Storage:** Use streaming to S3 (no local storage)
- **Response:** Direct presigned URL

**Implementation Steps:**
1. Query S3 Vectors for user data
2. Stream data directly to .mv2 format
3. Upload to S3 with TTL (24 hours)
4. Return presigned download URL

### Phase 2: Async Export (Large Datasets)

**Infrastructure:**
- **SQS Queue:** Export job queue
- **Lambda Worker:** `export-async` (15min timeout)
- **DynamoDB:** Job status tracking
- **S3:** Completed exports with TTL

**User Flow:**
1. Submit export request → Job ID returned
2. Background processing → Progress updates via API
3. Completion notification → Email + UI notification
4. Download available for 48 hours

## Risk Mitigation

### Memory Issues
- **Streaming processing:** Never load entire dataset into memory
- **Batch processing:** Process vectors in chunks
- **Progressive upload:** Upload .mv2 file in segments

### Timeout Issues
- **Size estimation:** Pre-calculate processing time
- **Auto-fallback:** Switch to async if sync fails
- **Retry logic:** Exponential backoff for failures

### Storage Costs
- **TTL policies:** Auto-delete exports after 24-48 hours
- **Compression:** Optimize .mv2 file size
- **Lifecycle policies:** Move to IA storage class

## Implementation Timeline

### Week 1: Foundation
- Data size estimation API
- Sync export Lambda (small datasets)
- S3 presigned URL generation

### Week 2: Async Infrastructure
- SQS queue setup
- Async export worker Lambda
- Job status tracking (DynamoDB)

### Week 3: User Experience
- Frontend progress tracking
- Email notifications
- Error handling and retry

### Week 4: Testing & Optimization
- Load testing with various dataset sizes
- Performance optimization
- Monitoring and alerting

## Monitoring & Metrics

### Key Metrics
- Export success rate by size category
- Average processing time
- Lambda timeout rate
- User satisfaction (survey)
- Storage costs

### Alerts
- High failure rate for sync exports
- Lambda timeout threshold exceeded
- Queue depth for async processing
- Storage costs trending up

## Security Considerations

### Data Privacy
- **Presigned URLs:** Short TTL (1-24 hours)
- **Access control:** User-specific exports only
- **Encryption:** In-transit and at-rest

### Authentication
- **JWT validation:** Verify user identity
- **Rate limiting:** Prevent abuse
- **Audit logging:** Track all export requests

## Cost Analysis

### Sync Export (per 100MB)
- Lambda: $0.0001 (compute)
- S3: $0.0023 (storage for 24hrs)
- Data transfer: $0.009
- **Total:** ~$0.01 per export

### Async Export (per 1GB)
- Lambda: $0.001 (compute)
- SQS: $0.0000004 (messages)
- S3: $0.023 (storage for 48hrs)
- Data transfer: $0.09
- **Total:** ~$0.11 per export

## Decision: Hybrid Architecture

**Recommended approach:**
1. **Start with sync-only** for MVP (covers 90% of use cases)
2. **Add async processing** for large datasets in Phase 2
3. **Use intelligent routing** to optimize user experience
4. **Monitor and adjust** thresholds based on real usage

This approach balances user experience, technical constraints, and implementation complexity while providing a clear path for scaling.

## Quality Gates

- [x] Architecture documented
- [x] Trade-offs analyzed  
- [x] Implementation approach defined
- [x] Timeline and phases outlined
- [x] Risk mitigation strategies defined
- [x] Cost analysis completed
- [x] Security considerations addressed