# Azure OpenAI Integration Fixes - Notes & Learnings

## ✅ Critical Fixes Completed (2025-01-02)

### Stream Processing Fix ✅ 
- **Issue**: Lifetime compilation error in `stream_completion` method
- **Solution**: Clone model before async move block to avoid capturing `&self`
- **Status**: Fixed and compiling successfully

### Enhanced Error Handling ✅
- **Robust JSON Parsing**: Added graceful handling for malformed JSON chunks
- **Debug Logging**: Comprehensive logging for stream processing and API calls
- **Azure Error Parsing**: Proper Azure API error format parsing
- **Status**: Implemented with detailed error context

### Model-Aware Tool Support ✅
- **Dynamic Tool Detection**: Model-specific tool support checking (o1 models excluded)
- **Parallel Tool Calls**: Validation before sending to Azure API
- **Consistent Interface**: Matches OpenAI provider behavior
- **Status**: Implemented and working

### Request Validation ✅
- **Parameter Validation**: Azure-compatible parameters (max_tokens vs max_completion_tokens)
- **Model-Specific Logic**: Proper handling for reasoning models (o1/o3)
- **API Compatibility**: Azure-specific request conversion
- **Status**: Complete with proper validation

---

## Key Insights from Analysis (2025-01-02)

### Stream Processing Differences
- **Azure vs OpenAI**: Azure OpenAI sometimes sends different JSON chunk formats
- **Buffer Behavior**: Azure may send partial JSON that needs different handling
- **Error Propagation**: Current error handling terminates entire stream on parse failure

### Model Capability Inconsistencies
- **Tool Support**: Azure implementation hardcodes `supports_tools() -> true`
- **OpenAI Pattern**: OpenAI provider checks model-specific capabilities dynamically
- **o1 Models**: Known to not support tools, but Azure provider still advertises support

### Request Format Specifics
- **Headers**: Azure uses `api-key` header vs OpenAI's `Authorization: Bearer`
- **Parameters**: Azure supports both `max_tokens` and `max_completion_tokens`
- **URL Structure**: Azure uses deployment-specific URLs vs model-specific

## Bugs Identified

### Critical Bugs
1. **Stream Termination**: Line 503-519 in `azure_stream_completion` - JSON parse errors kill entire stream
2. **False Tool Advertising**: Line 414-416 - All models advertise tool support regardless of capability
3. **Missing Parallel Tool Check**: `convert_to_azure_request` doesn't validate parallel tool calls support

### Error Handling Bugs
1. **Poor Error Context**: Azure errors don't include response body for debugging
2. **Missing Logging**: No debug logs for stream processing failures
3. **Generic Error Messages**: Azure-specific errors not parsed properly

## Implementation Notes

### Code Patterns to Maintain
- **Consistency**: Match OpenAI provider patterns where possible
- **Error Handling**: Use `anyhow::Result` and context properly
- **Logging**: Use `log::debug`, `log::warn`, `log::error` appropriately
- **Testing**: Follow existing test patterns in codebase

### Azure API Quirks
- **Content Field**: Azure can return `null` content with tool calls (already handled)
- **Error Format**: Azure uses different error response structure than OpenAI
- **Streaming**: Azure may send empty lines or incomplete JSON chunks

### OpenAI Provider Reference Points
- **Stream Parsing**: Lines 627-656 in `crates/open_ai/src/open_ai.rs`
- **Tool Support**: `Model::supports_parallel_tool_calls()` method
- **Error Handling**: Lines 648-665 show proper error context inclusion

## Testing Ideas

### Unit Test Scenarios
- Malformed JSON chunks in stream response
- Empty lines and whitespace in stream
- o1 models with tool requests (should fail gracefully)
- Network timeouts and connection failures
- Invalid API credentials

### Integration Test Ideas  
- Long conversations that previously cut off
- Multi-turn tool calling scenarios
- Model switching during conversation
- Error recovery and retry mechanisms

### Manual Testing Checklist
- [ ] Test with GPT-4 model and tool calling
- [ ] Test with o1 model (should not show tools)
- [ ] Test long response generation (check for cutoffs)
- [ ] Test with invalid API key (check error message quality)
- [ ] Test network interruption during streaming

## Performance Considerations

### Potential Optimizations
- **Lazy Logging**: Only format debug logs when debug level enabled
- **Buffer Sizing**: Optimize buffer sizes for Azure API response patterns
- **Connection Reuse**: Ensure HTTP client reuses connections efficiently

### Memory Usage
- **Stream Buffering**: Monitor memory usage during long responses
- **Error Context**: Balance detail vs memory usage in error messages
- **Logging**: Prevent log spam during normal operation

## Future Enhancements

### Monitoring & Observability
- Add metrics for parse failure rates
- Track tool usage success/failure rates
- Monitor response times and compare to OpenAI

### Configuration Options
- Allow disabling certain models that don't work well
- Configurable retry behavior for transient failures
- Debug mode for enhanced logging

### API Evolution
- Support for newer Azure OpenAI API versions
- Handle deprecated parameter migration
- Support for Azure-specific features

## References & Documentation

### Azure OpenAI API Docs
- [Chat Completions API](https://learn.microsoft.com/en-us/azure/ai-services/openai/reference#chat-completions)
- [Error Response Format](https://learn.microsoft.com/en-us/azure/ai-services/openai/troubleshooting)
- [Model Capabilities](https://learn.microsoft.com/en-us/azure/ai-services/openai/concepts/models)

### Zed Codebase Patterns
- Error handling: `crates/language_models/src/provider/open_ai.rs`
- Stream processing: `crates/open_ai/src/open_ai.rs:627-656`
- Model capabilities: `crates/open_ai/src/open_ai.rs:199-216`

### Related Issues
- User reports of response cutoffs
- Tool recognition failures with certain models
- Inconsistent behavior between OpenAI and Azure providers 