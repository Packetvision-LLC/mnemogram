# Mnemogram API Documentation

This directory contains the comprehensive API documentation for the Mnemogram serverless AI memory service.

## Files

- **`openapi.yaml`** - Complete OpenAPI 3.0 specification defining all API endpoints, request/response schemas, and authentication requirements
- **`api-docs.html`** - Interactive HTML documentation with Swagger UI for testing and exploring the API
- **`README.md`** - This file

## Viewing the Documentation

### Interactive HTML Documentation

1. Open `api-docs.html` in a web browser
2. The page will load the OpenAPI specification and render an interactive interface
3. You can explore endpoints, view schemas, and test API calls directly from the browser

### OpenAPI Specification

The `openapi.yaml` file can be used with any OpenAPI-compatible tool:

- **Swagger Editor**: Import the YAML file at https://editor.swagger.io/
- **Postman**: Import the OpenAPI spec to generate a collection
- **Code Generation**: Use tools like `openapi-generator` to create client SDKs
- **API Testing**: Tools like Insomnia, Thunder Client, or REST Client can import OpenAPI specs

## API Overview

### Base URLs
- **Production**: `https://api.mnemogram.ai`
- **Development**: `https://api-dev.mnemogram.ai`

### Authentication
Most endpoints require authentication via Bearer token in the Authorization header:
```
Authorization: Bearer <your-jwt-token>
```

### Main Endpoints

1. **Health Check** (`GET /status`) - Service status and version
2. **Memory Upload** (`PUT /memories`) - Upload .mv2 memory files
3. **Search** (`GET /search`) - Semantic search within memories
4. **Memory Cards** (`GET /v1/memories/{id}/cards`) - AI-extracted insights (Pro/Enterprise)
5. **Facts** (`GET /v1/memories/{id}/facts`) - Structured data extraction (Pro/Enterprise)
6. **Entity State** (`GET /v1/memories/{id}/state/{entity}`) - O(1) entity lookups (Pro/Enterprise)

### Service Tiers

- **Free**: Basic memory storage and search
- **Pro**: Advanced AI features (cards, facts, entity state)
- **Enterprise**: High-volume usage and priority support

## Usage Examples

### Upload a Memory File

```bash
curl -X PUT https://api.mnemogram.ai/memories \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/octet-stream" \
  --data-binary @my-memory.mv2
```

### Search Memories

```bash
curl "https://api.mnemogram.ai/search?q=artificial%20intelligence&memoryId=550e8400-e29b-41d4-a716-446655440000" \
  -H "Authorization: Bearer <token>"
```

### Get Memory Cards

```bash
curl "https://api.mnemogram.ai/v1/memories/550e8400-e29b-41d4-a716-446655440000/cards" \
  -H "Authorization: Bearer <token>"
```

## Development

To update the documentation:

1. Modify `openapi.yaml` with your changes
2. Validate the specification using tools like `swagger-codegen validate` or online validators
3. Test the interactive documentation by opening `api-docs.html`
4. Commit both files to version control

## Support

For questions about the API or documentation:

- **Documentation Issues**: Create an issue in this repository
- **API Support**: Contact support@mnemogram.ai
- **Community**: Visit our documentation at https://docs.mnemogram.ai