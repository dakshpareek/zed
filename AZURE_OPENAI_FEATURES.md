# Azure OpenAI Integration for Zed

This document describes the complete Azure OpenAI integration for Zed editor, including support for o3 models.

## Features

✅ **Full Azure OpenAI API Support**
- Complete authentication with Azure-specific API key headers
- Proper endpoint URL construction for Azure deployments
- Support for all Azure OpenAI models including o1 and o3 series

✅ **Reasoning Models Support** 
- Non-streaming completion for o1 and o3 models (required by Azure)
- Automatic `max_completion_tokens` handling for reasoning models

✅ **Advanced Configuration**
- Multiple model configurations with different settings
- Environment variable support for API keys
- Custom deployment name mapping

## Configuration

### Basic Configuration

```json
{
  "language_models": {
    "azure_openai": {
      "resource_name": "your-resource-name",
      "api_version": "2024-12-01-preview",
      "available_models": [
        {
          "name": "o3-mini",
          "deployment_name": "o3-mini",
          "display_name": "o3 Mini (Azure)",
          "max_tokens": 200000,
          "max_completion_tokens": 200000
        }
      ]
    }
  }
}
```

### Advanced Configuration with Multiple Models

```json
{
  "language_models": {
    "azure_openai": {
      "resource_name": "your-resource-name",
      "api_version": "2024-12-01-preview",
      "available_models": [
        {
          "name": "gpt-4o",
          "deployment_name": "gpt4o-deployment",
          "display_name": "GPT-4o (Azure)",
          "max_tokens": 128000,
          "max_output_tokens": 4096
        },
        {
          "name": "o3-mini",
          "deployment_name": "o3-mini",
          "display_name": "o3 Mini (Azure)",
          "max_tokens": 200000,
          "max_completion_tokens": 200000
        }
      ]
    }
  }
}
```

## Authentication

### Option 1: Environment Variable (Recommended)

```bash
export AZURE_OPENAI_API_KEY="your-azure-openai-api-key"
```

### Option 2: UI Configuration

1. Open Zed Settings
2. Navigate to Language Models → Azure OpenAI
3. Enter your API key in the configuration panel

## Supported Models

### Reasoning Models (o1, o3 series)
- Uses non-streaming completion
- Requires `max_completion_tokens` instead of `max_tokens`
- Automatically handles Azure-specific formatting

### Standard Models (GPT-4, GPT-3.5, etc.)
- Uses streaming completion for real-time responses
- Standard `max_tokens` parameter
- Full tool calling support
- Image input support (for vision models)

## URL Structure

The integration automatically constructs Azure OpenAI URLs in the format:
```
https://{resource_name}.openai.azure.com/openai/deployments/{deployment_name}/chat/completions?api-version={api_version}
```

## Troubleshooting

### "Response contained no choices" Error

1. **Check resource name**: Ensure it matches your Azure portal exactly
2. **Verify deployment name**: Must match the deployment in Azure
3. **API version**: Use `2024-12-01-preview` or later for o3 models
4. **API key access**: Ensure key has access to the specific deployment

### Authentication Issues

1. Check API key format (should be 32+ character string)
2. Verify resource name doesn't include `.openai.azure.com` suffix
3. Ensure API key has proper permissions in Azure

### Model-Specific Issues

- **o3 models**: Must use `max_completion_tokens` and API version `2024-12-01-preview`+
- **Vision models**: Ensure deployment supports image inputs
- **Tool calling**: Verify model deployment supports function calling

## Technical Implementation

### Key Features

1. **Automatic Model Detection**: Detects o1/o3 models and switches to non-streaming
2. **Parameter Mapping**: Converts OpenAI parameters to Azure-compatible format
3. **Error Handling**: Comprehensive error messages with response debugging
4. **Credential Management**: Secure storage using Zed's credential provider

### API Differences Handled

| Feature | OpenAI | Azure OpenAI | Implementation |
|---------|---------|--------------|----------------|
| Authentication | `Authorization: Bearer` | `api-key: <key>` | ✅ Handled |
| Endpoint | Standard URL | Resource-specific | ✅ Auto-constructed |
| Model reference | Model name | Deployment name | ✅ Configurable mapping |
| API versioning | Not required | Required parameter | ✅ Configurable |

## Future Enhancements

- [ ] Auto-discovery of available deployments
- [ ] Model cost tracking and reporting
- [ ] Advanced retry logic with backoff
- [ ] Batch processing support
- [ ] Model performance analytics

## Getting Help

If you encounter issues:

1. Check Zed logs for detailed error messages
2. Verify your Azure OpenAI deployment is active
3. Test API access with curl/Postman first
4. Ensure you're using a supported API version

For the latest updates and examples, see the [Azure OpenAI documentation](https://docs.microsoft.com/en-us/azure/cognitive-services/openai/). 