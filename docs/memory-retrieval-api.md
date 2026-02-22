# Memory Retrieval Operations API

This document describes the memory retrieval operations implemented in MNEM-220.

## Overview

The recall Lambda function provides two main endpoints for memory retrieval using S3 Vectors:

1. **Single Memory Recall** - Retrieve all chunks from a specific memory
2. **Bulk Memory Retrieval** - Retrieve chunks from multiple memories for a user

## Endpoints

### GET /memories/{id}/recall

Retrieve all chunks from a specific memory by ID.

**Path Parameters:**
- `id` - Memory ID (string, required)

**Query Parameters:**
- `limit` - Number of chunks to return (integer, default: 50, max: 1000)
- `offset` - Number of chunks to skip (integer, default: 0)
- `include_metadata` - Include metadata in response (boolean, default: false)

**Response:**
```json
{
  "memoryId": "abc123",
  "results": [
    {
      "memoryId": "abc123",
      "timestamp": "2024-01-01T00:00:00Z",
      "snippet": "Memory content chunk...",
      "score": 0.95,
      "frameId": "chunk_001",
      "metadata": {
        "source": "s3_vectors",
        "retrievalMethod": "vector_similarity"
      }
    }
  ],
  "total": 25,
  "offset": 0,
  "limit": 50,
  "hasMore": false,
  "retrievedAt": "2024-01-01T12:00:00Z"
}
```

**Error Responses:**
- `404` - Memory not found
- `403` - Access denied (not user's memory)
- `503` - Migration pending or service unavailable

### GET /recall

Bulk retrieval of memory chunks across multiple memories for a user.

**Query Parameters:**
- `max_memories` - Maximum memories to retrieve (integer, default: 10, max: 50)
- `chunks_per_memory` - Chunks per memory (integer, default: 20, max: 100)

**Response:**
```json
{
  "userId": "user123",
  "memories": [
    {
      "memoryId": "abc123",
      "name": "My Important Memory",
      "totalChunks": 50,
      "retrievedChunks": 20,
      "status": "success",
      "chunks": [
        {
          "memoryId": "abc123",
          "timestamp": "2024-01-01T00:00:00Z",
          "snippet": "Content...",
          "score": 0.95,
          "frameId": "chunk_001"
        }
      ]
    }
  ],
  "totalMemories": 5,
  "totalChunks": 100,
  "retrievedAt": "2024-01-01T12:00:00Z"
}
```

## Features

### Memory Recall by ID
- Retrieves all vector chunks for a specific memory
- Supports pagination with offset/limit parameters
- Returns chunks ordered by similarity score
- Includes optional metadata

### Bulk Memory Retrieval
- Retrieves chunks from multiple user memories in one request
- Configurable limits for memories and chunks per memory
- Handles mixed migration states (some migrated, some not)
- Efficient batch processing

### Metadata Retrieval
- Optional metadata inclusion for detailed analysis
- Source tracking (s3_vectors)
- Retrieval method indication
- Timestamp preservation

### Result Formatting
- Maintains consistency with existing search APIs
- Backward compatible response structure
- Clear pagination indicators
- Status information for each memory

### Error Handling
- **Migration Pending (503)**: Memory not yet migrated to S3 Vectors
- **Access Denied (403)**: User doesn't own the memory
- **Not Found (404)**: Memory doesn't exist
- **Service Unavailable (503)**: S3 Vectors temporarily unavailable

## Authentication

All endpoints require user authentication via JWT token or API key:
- User ID extracted from `x-user-id` header
- Memory access verified through DynamoDB ownership check
- Data isolation enforced at the user level

## Performance Considerations

### Single Memory Recall
- Optimized for memories with up to 10,000 chunks
- Uses pagination to manage large result sets
- Typical response time: 100-500ms

### Bulk Retrieval
- Limited to 50 memories maximum per request
- Processes memories in parallel where possible
- Gracefully handles mixed migration states
- Typical response time: 200-1000ms depending on memory count

## Data Migration Compatibility

The recall endpoints handle both migrated and non-migrated memories:

- **Migrated Memories**: Use S3 Vectors for fast retrieval
- **Non-Migrated Memories**: Return `migration_pending` status
- **Mixed States**: Bulk operations handle both gracefully

## Usage Examples

### Single Memory Recall
```bash
# Get first 20 chunks from a memory
GET /memories/abc123/recall?limit=20&include_metadata=true

# Get next 20 chunks (pagination)
GET /memories/abc123/recall?limit=20&offset=20
```

### Bulk Retrieval
```bash
# Get chunks from up to 5 memories, 10 chunks each
GET /recall?max_memories=5&chunks_per_memory=10

# Get extensive recall data
GET /recall?max_memories=20&chunks_per_memory=50
```

## Integration with Existing APIs

The recall endpoints complement the existing search APIs:

- **Search APIs**: Query-based semantic search within memories
- **Recall APIs**: Complete retrieval of all memory content
- **Export APIs**: (Future) Full memory export with format conversion

## Monitoring and Analytics

Key metrics tracked:
- Recall request volume and patterns
- Average response times per endpoint
- Memory migration status distribution
- Error rates by error type
- Chunk retrieval efficiency

## Future Enhancements

Planned improvements:
1. **Streaming Responses**: For very large memories
2. **Filtered Retrieval**: Retrieve chunks by date range or metadata
3. **Compressed Responses**: Reduce bandwidth for bulk operations
4. **Caching**: Cache frequently accessed memories
5. **Export Integration**: Direct export from recall data