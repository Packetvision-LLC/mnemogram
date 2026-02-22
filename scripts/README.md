# Mnemogram S3 Vectors Migration Pipeline

This directory contains the data migration pipeline for converting MemVid (.mv2) data to S3 Vectors format as part of MNEM-210.

## Migration Script

### migrate-to-s3-vectors.js

A comprehensive Node.js script that migrates all existing MemVid data to S3 Vectors format.

## Features

- **Zero Data Loss**: Validates each migration step to ensure no data is lost
- **Batch Processing**: Processes memories in configurable batches for large datasets
- **Progress Tracking**: Saves progress after each batch for resumability
- **Rollback Capabilities**: Can rollback individual memory migrations
- **Data Integrity Verification**: Validates vectors exist in S3 Vectors after migration
- **Dry Run Mode**: Test migrations without making changes
- **Comprehensive Reporting**: Generates detailed migration reports

## Prerequisites

- Node.js 18+ with AWS SDK v2
- AWS credentials configured
- S3 Vectors bucket and index created
- DynamoDB memories table accessible
- Bedrock access for embedding generation

## Environment Variables

```bash
MEMORIES_TABLE=mnemogram-memories-dev
VECTOR_BUCKET_NAME=mnemogram-vectors-dev
VECTOR_INDEX_NAME=memories
AWS_REGION=us-east-1
```

## Usage

### Run Full Migration

```bash
./migrate-to-s3-vectors.js
```

### Configuration Options

```bash
./migrate-to-s3-vectors.js \
  --batch-size 25 \
  --max-retries 5 \
  --dry-run true \
  --region us-west-2
```

### Resume Interrupted Migration

The script automatically resumes from the last completed batch if interrupted:

```bash
./migrate-to-s3-vectors.js
# Will resume from migration-progress.json
```

### Rollback Single Memory

```bash
./migrate-to-s3-vectors.js --rollback --memory-id abc123def456
```

## Output Files

- `migration-progress.json` - Progress tracking for resumability
- `migration-validation.json` - Validation results
- `migration-report-YYYY-MM-DD.json` - Comprehensive migration report

## Migration Process

1. **Discovery**: Scans DynamoDB for memories with .mv2 files
2. **Validation**: Verifies S3 Vectors and Bedrock accessibility
3. **Batch Processing**: Processes memories in configurable batches
4. **Per-Memory Steps**:
   - Download .mv2 file from S3
   - Extract text chunks (placeholder implementation)
   - Generate embeddings via Bedrock
   - Store vectors in S3 Vectors
   - Update DynamoDB record
   - Validate migration
5. **Final Validation**: Verifies all migrations completed successfully
6. **Reporting**: Generates comprehensive migration report

## Error Handling

- Automatic retry with exponential backoff
- Error logging with context
- Graceful handling of partial failures
- Progress preservation for resumability

## Monitoring

The script provides detailed logging:

```
🚀 Starting Mnemogram Data Migration Pipeline
📊 Found 1,250 memories to migrate
🔒 Validating prerequisites...
✅ S3 Vectors accessible
✅ Bedrock embedding service accessible
📦 Processing 25 batches of 50 memories each
🔄 Processing batch 1/25 (50 memories)
✅ Migrated memory abc123 (My Important Memory)
📊 Batch 1 completed: 48/50 succeeded
...
✅ Migration pipeline completed successfully
```

## Safety Features

- **Dry Run Mode**: Test without making changes
- **Rollback**: Undo individual memory migrations
- **Validation**: Verify each step completed correctly
- **Progress Tracking**: Resume interrupted migrations
- **Error Recovery**: Retry failed operations with backoff

## Performance

- Batch processing for efficiency
- Concurrent processing within batches
- Configurable batch sizes and retry logic
- Progress tracking minimizes duplicate work

## Example Migration Report

```json
{
  "migration": {
    "startTime": "2026-02-22T09:30:00.000Z",
    "endTime": "2026-02-22T10:45:30.000Z", 
    "duration": 4530000,
    "dryRun": false
  },
  "statistics": {
    "totalMemories": 1250,
    "processed": 1250,
    "succeeded": 1247,
    "failed": 3,
    "skipped": 0
  },
  "errors": [...],
  "configuration": {...}
}
```

## Troubleshooting

### Common Issues

1. **S3 Vectors not accessible**: Verify bucket and index exist
2. **Bedrock permission denied**: Check IAM policies for Bedrock access
3. **DynamoDB scan timeout**: Use smaller batch sizes
4. **Memory allocation**: Process smaller batches for large datasets

### Recovery

If migration fails partway through:

1. Check `migration-progress.json` for last completed batch
2. Review errors in migration report
3. Fix underlying issues
4. Re-run script to resume from last checkpoint

## Testing

Test migrations safely using dry run mode:

```bash
./migrate-to-s3-vectors.js --dry-run true --batch-size 5
```

This will simulate the migration without making any changes.