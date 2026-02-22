# API Layer Updates for S3 Vectors Integration

This document outlines the API layer updates completed as part of MNEM-212.

## Overview

All API endpoints have been updated to use the new S3 Vectors integration instead of direct MemVid CLI calls. This provides improved performance, scalability, and maintains backward compatibility.

## Updated Lambda Functions

### 1. api-search (GET /search)
- **File**: `lambdas/api-search/src/main.rs`
- **Changes**: 
  - Replaced direct memvid CLI execution with `MemvidClient` from shared module
  - Added migration status checking with graceful degradation
  - Maintained existing API response format
  - Added `searchMethod: "s3_vectors"` field to responses
  - Enhanced error handling for S3 Vectors failures

### 2. search (POST /memories/{id}/search)  
- **File**: `lambdas/search/src/main.rs`
- **Changes**:
  - Updated to use S3 Vectors via `MemvidClient`
  - Added migration status validation
  - Maintained backward compatible response structure
  - Enhanced error handling with appropriate HTTP status codes
  - Added performance monitoring indicators

## API Response Format Changes

### Maintained Compatibility
All existing API response fields are preserved:
- `memoryId`: Memory identifier
- `relevanceScore`/`score`: Similarity score
- `snippet`: Text content snippet
- `timestamp`: Timestamp information
- `frameId`: Frame identifier
- `confidence`: Confidence score

### New Fields Added
- `searchMethod`: Indicates backend used ("s3_vectors")
- Migration status indicators in error responses

### Error Handling Updates

#### Migration Pending (HTTP 503)
```json
{
  "error": "migration_pending",
  "message": "This memory is being migrated to S3 Vectors. Please try again later.",
  "memoryId": "abc123"
}
```

#### Search Unavailable (HTTP 503)
```json
{
  "error": "search_unavailable", 
  "message": "Search service temporarily unavailable"
}
```

## Environment Variables

New environment variables required:
- `VECTOR_BUCKET_NAME`: S3 Vectors bucket name
- `VECTOR_INDEX_NAME`: S3 Vectors index name (default: "memories")
- `EMBEDDING_MODEL_ID`: Bedrock model ID (default: "amazon.titan-embed-text-v2:0")
- `EMBEDDING_DIMENSION`: Vector dimensions (default: "1024")

## Performance Monitoring

### Response Time Tracking
- S3 Vectors queries are logged with timing information
- Error rates monitored through CloudWatch metrics
- Search result counts tracked

### Usage Analytics
- Search method tracking (`s3_vectors` vs legacy)
- Migration status monitoring
- Performance comparison metrics

## Integration Tests

### Test Scenarios Covered
1. **Successful Search**: Migrated memory with valid query
2. **Migration Pending**: Non-migrated memory returns 503
3. **Access Control**: User can only search their own memories
4. **Input Validation**: Empty queries return 400
5. **Error Handling**: S3 Vectors failures return 503
6. **Performance**: Response times within acceptable limits

### Test Data
- Sample memories with known vectors in S3 Vectors
- Test queries with expected similarity scores
- Edge cases (empty results, malformed queries)

## Deployment Considerations

### Backward Compatibility
- API endpoints maintain same URLs and HTTP methods
- Response formats preserved for existing clients
- Graceful handling of non-migrated memories

### Migration Strategy
1. Deploy updated Lambda functions
2. Run data migration pipeline (MNEM-210)
3. Update memory records with `vectorsMigrated: true`
4. Monitor API performance and error rates

### Rollback Plan
- Lambda functions detect migration status
- Non-migrated memories automatically use legacy fallback
- Can rollback individual memories using migration pipeline

## Documentation Updates

### API Documentation
- Updated endpoint descriptions mention S3 Vectors
- New error response codes documented
- Environment variable requirements added
- Performance characteristics updated

### Migration Guide
- Steps for operators to verify API functionality
- Monitoring dashboards for tracking migration progress
- Troubleshooting guide for common issues

## Performance Benchmarks

### Expected Improvements
- **Latency**: 50-80% reduction in search response time
- **Throughput**: 3-5x increase in concurrent search capacity  
- **Scalability**: No Lambda storage constraints
- **Cost**: Reduced Lambda execution time and storage costs

### Monitoring Metrics
- Average search response time
- P95/P99 latency percentiles
- Search success/error rates
- Vector embedding generation time
- S3 Vectors query performance

## Security Considerations

### Access Control
- User isolation maintained through memory ownership checks
- S3 Vectors filtered by memory_id to prevent cross-user access
- IAM policies restrict access to appropriate AWS resources

### Data Protection
- Vector embeddings stored in encrypted S3 Vectors buckets
- No sensitive data exposed in API responses
- Audit logging for all search operations

## Future Enhancements

### Planned Improvements
1. **Caching**: Redis caching for frequently searched queries
2. **Analytics**: Enhanced search analytics and recommendations
3. **Hybrid Search**: Combine vector similarity with metadata filtering
4. **Real-time**: WebSocket support for live search results

### API Evolution
- Gradual phase-out of legacy fields
- Enhanced filtering and sorting options
- Bulk search operations
- Search result export capabilities