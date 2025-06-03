# Azure OpenAI Integration Fixes - Implementation Plan (TDD)

## Overview
This plan outlines the step-by-step implementation to fix Azure OpenAI streaming cutoffs and tool recognition issues.

## ✅ Phase 1: Critical Fixes (COMPLETED - 2025-01-02)

### ✅ 1. Fix Stream Response Parsing
**File**: `crates/language_models/src/provider/azure_openai.rs`

#### ✅ Step 1.1: Write failing tests for malformed JSON handling
- [x] Create test for invalid JSON chunks in stream
- [x] Create test for partial JSON chunks
- [x] Create test for empty lines in stream
- [x] Verified tests fail with current implementation

#### ✅ Step 1.2: Implement robust JSON parsing
- [x] Add debug logging for raw stream lines
- [x] Implement error handling that continues stream on parse failures
- [x] Add warnings for malformed JSON instead of hard failures
- [x] Ensure tests pass

#### ✅ Step 1.3: Refactor stream processing for resilience
- [x] Add line trimming and validation
- [x] Implement graceful handling of malformed chunks
- [x] Add metrics/counters for parsing failures
- [x] Run tests to ensure no regressions
- [x] Fix lifetime compilation issue in stream_completion

### ✅ 2. Fix Tool Support Detection
**File**: `crates/language_models/src/provider/azure_openai.rs`

#### ✅ Step 2.1: Write failing tests for model-specific tool support
- [x] Create test for o1 models (should not support tools)
- [x] Create test for GPT-4 models (should support tools)
- [x] Create test for custom models (should check configuration)
- [x] Verified tests fail with current hardcoded `true` return

#### ✅ Step 2.2: Implement model-aware tool support
- [x] Add `supports_tools()` logic matching OpenAI provider
- [x] Check model type before advertising tool capabilities
- [x] Handle custom model configurations properly
- [x] Ensure tests pass

#### ✅ Step 2.3: Add parallel tool calls capability checking
- [x] Implement `supports_parallel_tool_calls()` check in request conversion
- [x] Add tests for models that don't support parallel calls
- [x] Modify `convert_to_azure_request()` to respect model capabilities
- [x] Validate with integration tests

### ✅ 3. Enhance Error Handling and Logging
**File**: `crates/language_models/src/provider/azure_openai.rs`

#### ✅ Step 3.1: Write tests for error scenarios
- [x] Create test for API error responses
- [x] Create test for network timeout scenarios
- [x] Create test for authentication failures
- [x] Verified current error handling is insufficient

#### ✅ Step 3.2: Implement comprehensive error handling
- [x] Add response body to error messages for debugging
- [x] Implement Azure-specific error parsing
- [x] Add structured logging with request/response context
- [x] Ensure tests pass with better error information

#### ✅ Step 3.3: Add debug logging for troubleshooting
- [x] Log request details before sending to Azure
- [x] Log response headers and status codes
- [x] Add timing information for performance debugging
- [x] Test logging works in development mode

## Phase 2: Robustness Improvements (Short-term)

### 4. Request Validation Layer
**File**: `crates/language_models/src/provider/azure_openai.rs`

#### Step 4.1: Write tests for invalid request parameters
- [ ] Test o1 models with max_tokens vs max_completion_tokens
- [ ] Test models with unsupported parameters
- [ ] Test parameter combinations that Azure rejects

#### Step 4.2: Implement parameter validation
- [ ] Add validation before converting to Azure request
- [ ] Implement model-specific parameter filtering
- [ ] Add warnings for unsupported parameter combinations

### 5. Azure-Specific API Compatibility
**File**: `crates/language_models/src/provider/azure_openai.rs`

#### Step 5.1: Write tests for Azure API differences
- [ ] Test response format differences
- [ ] Test error format differences
- [ ] Test header requirements (api-key vs Authorization)

#### Step 5.2: Implement Azure compatibility layer
- [ ] Handle Azure-specific response formats
- [ ] Parse Azure error responses correctly
- [ ] Ensure proper header handling

## Testing Strategy

### Unit Tests
- **Location**: `crates/language_models/src/provider/azure_openai.rs` (inline tests)
- **Focus**: Individual function behavior, error handling, model capability detection
- **Coverage**: Aim for 90%+ coverage of critical paths

### Integration Tests  
- **Location**: `crates/language_models/tests/azure_openai_integration.rs`
- **Focus**: End-to-end streaming, tool usage, error scenarios
- **Mock Strategy**: Use HTTP client mocks for predictable testing

### Manual Testing Scenarios
1. **Stream Cutoff**: Long conversation with complex responses
2. **Tool Usage**: Multi-step tool calling with different models
3. **Error Handling**: Invalid API keys, rate limiting, model unavailability
4. **Model Switching**: Testing different model types and their capabilities

## Success Criteria

### For Stream Parsing Fix
- [ ] No more mid-response cutoffs during long generations
- [ ] Graceful handling of malformed JSON chunks
- [ ] Comprehensive logging for debugging stream issues

### For Tool Support Fix
- [ ] Tool capabilities correctly advertised per model
- [ ] No tool calls sent to incompatible models
- [ ] Parallel tool calls only used when supported

### For Error Handling Fix
- [ ] Clear, actionable error messages for users
- [ ] Sufficient debugging information for developers
- [ ] Azure-specific errors properly parsed and displayed

## Risk Mitigation

### Backward Compatibility
- Ensure all changes maintain existing API compatibility
- Add feature flags for new behaviors if needed
- Extensive testing with existing Azure OpenAI configurations

### Performance Impact
- Monitor performance impact of additional logging
- Ensure error handling doesn't add significant latency
- Profile streaming performance before and after changes

### Error Recovery
- Implement graceful degradation when Azure API behaves unexpectedly
- Add circuit breaker patterns for repeated failures
- Ensure failures don't crash the application 