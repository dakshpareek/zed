# Azure OpenAI Configuration Example

## Correct Settings Format

Add this to your Zed settings.json file:

```json
{
  "language_models": {
    "azure_openai": {
      "resource_name": "admin-m5utf4uq-eastus2",
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

## Key Changes Made

1. **Resource Name**: Removed `.openai` suffix - use only the base resource name
2. **max_completion_tokens**: Added this field for o3 models (required for reasoning models)
3. **API Version**: Using the latest version that supports o3 models

## Environment Variable (Alternative)

Instead of entering your API key in the UI, you can set:

```bash
export AZURE_OPENAI_API_KEY="your-azure-openai-api-key"
```

## Troubleshooting

If you still get "Response contained no choices" error:

1. Verify your resource name is correct (check Azure portal)
2. Ensure your deployment name matches exactly what's in Azure
3. Check that your API key has access to the o3-mini deployment
4. Verify the API version supports o3 models

## Notes

- o3 models use non-streaming completion (like o1 models)
- They require `max_completion_tokens` instead of `max_tokens`
- The provider will automatically handle the Azure-specific authentication headers 