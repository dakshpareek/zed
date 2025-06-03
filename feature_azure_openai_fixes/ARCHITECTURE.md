# Azure OpenAI Integration - Architecture Overview

## Current Architecture (Updated: 2025-01-02)

### Integration Components
- **Provider**: `AzureOpenAiLanguageModelProvider` - Main provider managing models and authentication
- **Model**: `AzureOpenAiLanguageModel` - Individual model implementations
- **Streaming**: `azure_stream_completion` - Stream processing for real-time responses
- **Conversion Layer**: Request/response conversion between OpenAI and Azure formats

### Current Data Flow
```
LanguageModelRequest -> into_open_ai() -> convert_to_azure_request() -> Azure API -> azure_stream_completion() -> OpenAiEventMapper
```

### Key Architecture Issues Identified (References DECISIONS.md)

#### 1. Stream Processing Fragility
- **Current**: Basic line-by-line processing without robust error handling
- **Problem**: Malformed JSON chunks cause complete stream termination
- **Impact**: Users experience cut-off responses mid-generation

#### 2. Model Capability Detection
- **Current**: Hardcoded `supports_tools() -> true` for all models
- **Problem**: Doesn't account for model-specific limitations
- **Impact**: Tool calls sent to incompatible models causing failures

#### 3. Request Conversion Logic
- **Current**: Simple mapping without Azure-specific parameter validation
- **Problem**: Missing parallel tool calls capability checks
- **Impact**: Invalid parameters sent to Azure API

#### 4. Error Handling Strategy
- **Current**: Basic error propagation with minimal context
- **Problem**: Insufficient debugging information for troubleshooting
- **Impact**: Difficult to diagnose Azure-specific API issues

## Planned Architecture Improvements

### Enhanced Stream Processing
- **Robust JSON Parsing**: Implement fallback mechanisms for malformed chunks
- **Detailed Logging**: Add comprehensive error and debug logging
- **Graceful Degradation**: Continue processing when possible, fail gracefully when not

### Model-Aware Capability Detection
- **Dynamic Tool Support**: Check model-specific capabilities before advertising features
- **Parallel Tool Call Validation**: Verify model support before setting parameters
- **Consistent Interface**: Match OpenAI provider behavior for consistency

### Improved Error Context
- **Rich Error Messages**: Include response bodies and request details in errors
- **Azure-Specific Error Handling**: Parse Azure API error formats properly
- **Debug-Friendly Logging**: Add sufficient context for troubleshooting

### Request Validation Layer
- **Parameter Validation**: Ensure Azure-compatible parameters before API calls
- **Model-Specific Logic**: Apply model-specific request modifications
- **API Compatibility**: Handle Azure OpenAI vs OpenAI API differences

## Implementation Strategy

### Phase 1: Critical Fixes (Immediate)
1. Fix stream parsing with proper error handling
2. Implement model-aware tool support detection
3. Add comprehensive logging for debugging

### Phase 2: Robustness (Short-term)
1. Enhanced error context and Azure-specific error parsing
2. Parallel tool calls capability checking
3. Request validation layer

### Phase 3: Optimization (Long-term)
1. Performance improvements for streaming
2. Caching for model capabilities
3. Advanced retry mechanisms for transient failures 