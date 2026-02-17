#!/bin/bash

# health-check.sh
# Comprehensive health check script for Mnemogram post-rollback validation
# Usage: ./health-check.sh <stage> [--timeout=300] [--verbose]

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="/tmp/mnemogram-health-check-$(date +%Y%m%d-%H%M%S).log"
DEFAULT_TIMEOUT=300 # 5 minutes

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log() {
    echo -e "${BLUE}[$(date '+%Y-%m-%d %H:%M:%S')]${NC} $1" | tee -a "$LOG_FILE"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1" | tee -a "$LOG_FILE"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1" | tee -a "$LOG_FILE"
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1" | tee -a "$LOG_FILE"
}

# Global variables for tracking
TOTAL_CHECKS=0
PASSED_CHECKS=0
FAILED_CHECKS=0
WARNINGS=0

# Check tracking functions
start_check() {
    local check_name="$1"
    log "🔍 Starting check: $check_name"
    ((TOTAL_CHECKS++))
}

pass_check() {
    local check_name="$1"
    success "✅ PASSED: $check_name"
    ((PASSED_CHECKS++))
}

fail_check() {
    local check_name="$1"
    local reason="${2:-No reason provided}"
    error "❌ FAILED: $check_name - $reason"
    ((FAILED_CHECKS++))
}

warn_check() {
    local check_name="$1" 
    local reason="${2:-No reason provided}"
    warn "⚠️  WARNING: $check_name - $reason"
    ((WARNINGS++))
}

# Help function
show_help() {
    cat << EOF
Usage: $0 <stage> [options]

Arguments:
  stage     Environment stage (dev, staging, prod)

Options:
  --timeout=N   Timeout for health checks in seconds (default: 300)
  --verbose     Enable verbose output
  --help        Show this help message

Examples:
  $0 dev --verbose
  $0 staging --timeout=600
  $0 prod

Health Check Categories:
  - AWS Resource Connectivity
  - API Gateway Health
  - Lambda Function Health  
  - DynamoDB Health
  - S3 Bucket Health
  - Application Functionality
  - Performance Validation
EOF
}

# Get AWS context
get_aws_context() {
    AWS_ACCOUNT=$(aws sts get-caller-identity --query Account --output text)
    AWS_REGION=$(aws configure get region || echo "us-east-1")
    log "AWS Context: Account $AWS_ACCOUNT, Region $AWS_REGION"
}

# Get resource names for stage
get_resource_names() {
    local stage="$1"
    
    # CloudFormation stack
    STACK_NAME="MnemogramStack-${stage}"
    
    # S3 bucket
    MEMORY_BUCKET="mnemogram-${stage}-memories-${AWS_ACCOUNT}-${AWS_REGION}"
    
    # DynamoDB tables
    METADATA_TABLE="mnemogram-${stage}-metadata"
    MEMORIES_TABLE="mnemogram-${stage}-memories"
    SUBSCRIPTIONS_TABLE="mnemogram-${stage}-subscriptions"
    API_KEYS_TABLE="mnemogram-${stage}-api-keys"
    USAGE_TABLE="mnemogram-${stage}-usage"
    THRESHOLD_TABLE="mnemogram-${stage}-threshold-tracking"
    
    # Lambda functions
    LAMBDA_FUNCTIONS=(
        "mnemogram-${stage}-status"
        "mnemogram-${stage}-ingest"
        "mnemogram-${stage}-search"
        "mnemogram-${stage}-recall"
        "mnemogram-${stage}-manage"
        "mnemogram-${stage}-authorizer"
    )
    
    log "Resource names configured for stage: $stage"
}

# Check CloudFormation stack status
check_cloudformation_stack() {
    start_check "CloudFormation Stack Status"
    
    local stack_status=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --query 'Stacks[0].StackStatus' \
        --output text 2>/dev/null || echo "NOT_FOUND")
    
    case "$stack_status" in
        "CREATE_COMPLETE"|"UPDATE_COMPLETE")
            pass_check "CloudFormation Stack Status" 
            ;;
        "UPDATE_IN_PROGRESS"|"CREATE_IN_PROGRESS")
            warn_check "CloudFormation Stack Status" "Stack is still updating: $stack_status"
            ;;
        "NOT_FOUND")
            fail_check "CloudFormation Stack Status" "Stack not found: $STACK_NAME"
            return 1
            ;;
        *)
            fail_check "CloudFormation Stack Status" "Unexpected status: $stack_status"
            return 1
            ;;
    esac
}

# Check API Gateway health
check_api_gateway() {
    start_check "API Gateway Health"
    
    # Get API URL from CloudFormation outputs
    local api_url=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --query "Stacks[0].Outputs[?OutputKey=='ApiUrl'].OutputValue" \
        --output text 2>/dev/null)
    
    if [[ -z "$api_url" ]]; then
        fail_check "API Gateway Health" "Could not retrieve API URL from stack"
        return 1
    fi
    
    log "Testing API endpoint: $api_url/v1/status"
    
    # Test status endpoint
    local http_code=$(curl -s -o /dev/null -w "%{http_code}" \
        --max-time 30 \
        "$api_url/v1/status" || echo "000")
    
    case "$http_code" in
        200)
            pass_check "API Gateway Health"
            ;;
        000)
            fail_check "API Gateway Health" "Connection timeout or network error"
            return 1
            ;;
        *)
            fail_check "API Gateway Health" "HTTP $http_code response"
            return 1
            ;;
    esac
}

# Check Lambda function health
check_lambda_functions() {
    start_check "Lambda Functions Health"
    
    local all_healthy=true
    
    for function_name in "${LAMBDA_FUNCTIONS[@]}"; do
        log "Checking Lambda function: $function_name"
        
        # Check if function exists and get status
        local function_info=$(aws lambda get-function \
            --function-name "$function_name" \
            --query '{State:Configuration.State,LastUpdateStatus:Configuration.LastUpdateStatus}' \
            --output json 2>/dev/null || echo '{"State":"NOT_FOUND"}')
        
        local state=$(echo "$function_info" | jq -r '.State')
        local update_status=$(echo "$function_info" | jq -r '.LastUpdateStatus // "Unknown"')
        
        case "$state" in
            "Active")
                if [[ "$update_status" == "Successful" ]]; then
                    log "  ✅ $function_name: Active and up to date"
                else
                    warn "  ⚠️  $function_name: Active but update status: $update_status"
                    all_healthy=false
                fi
                ;;
            "Pending")
                warn "  ⚠️  $function_name: Pending (still updating)"
                all_healthy=false
                ;;
            "NOT_FOUND")
                error "  ❌ $function_name: Function not found"
                all_healthy=false
                ;;
            *)
                error "  ❌ $function_name: Unexpected state: $state"
                all_healthy=false
                ;;
        esac
        
        # Test function invocation (status function only)
        if [[ "$function_name" == *"-status" && "$state" == "Active" ]]; then
            log "  Testing invocation of $function_name..."
            
            local invoke_result=$(aws lambda invoke \
                --function-name "$function_name" \
                --payload '{}' \
                /tmp/lambda-test-output 2>&1 || echo "INVOKE_FAILED")
            
            if [[ "$invoke_result" == "INVOKE_FAILED" ]]; then
                error "  ❌ $function_name: Invocation failed"
                all_healthy=false
            else
                local status_code=$(echo "$invoke_result" | jq -r '.StatusCode // 999')
                if [[ "$status_code" == "200" ]]; then
                    log "  ✅ $function_name: Invocation successful"
                else
                    error "  ❌ $function_name: Invocation returned status $status_code"
                    all_healthy=false
                fi
            fi
        fi
    done
    
    if [[ "$all_healthy" == "true" ]]; then
        pass_check "Lambda Functions Health"
    else
        fail_check "Lambda Functions Health" "One or more functions unhealthy"
        return 1
    fi
}

# Check DynamoDB health
check_dynamodb_tables() {
    start_check "DynamoDB Tables Health"
    
    local tables=("$METADATA_TABLE" "$MEMORIES_TABLE" "$SUBSCRIPTIONS_TABLE" "$API_KEYS_TABLE" "$USAGE_TABLE" "$THRESHOLD_TABLE")
    local all_healthy=true
    
    for table in "${tables[@]}"; do
        log "Checking DynamoDB table: $table"
        
        # Check table status
        local table_status=$(aws dynamodb describe-table \
            --table-name "$table" \
            --query 'Table.TableStatus' \
            --output text 2>/dev/null || echo "NOT_FOUND")
        
        case "$table_status" in
            "ACTIVE")
                log "  ✅ $table: Active"
                
                # Check read/write capacity for provisioned tables
                local billing_mode=$(aws dynamodb describe-table \
                    --table-name "$table" \
                    --query 'Table.BillingModeSummary.BillingMode' \
                    --output text 2>/dev/null || echo "PAY_PER_REQUEST")
                
                if [[ "$billing_mode" == "PROVISIONED" ]]; then
                    local consumed_read=$(aws cloudwatch get-metric-statistics \
                        --namespace AWS/DynamoDB \
                        --metric-name ConsumedReadCapacityUnits \
                        --dimensions Name=TableName,Value="$table" \
                        --start-time "$(date -u -d '5 minutes ago' +%Y-%m-%dT%H:%M:%S)" \
                        --end-time "$(date -u +%Y-%m-%dT%H:%M:%S)" \
                        --period 300 \
                        --statistics Average \
                        --query 'Datapoints[0].Average' \
                        --output text 2>/dev/null || echo "0")
                    
                    log "  📊 $table: Average read capacity (5m): ${consumed_read:-0}"
                fi
                ;;
            "CREATING"|"UPDATING")
                warn "  ⚠️  $table: Still updating ($table_status)"
                all_healthy=false
                ;;
            "NOT_FOUND")
                error "  ❌ $table: Table not found"
                all_healthy=false
                ;;
            *)
                error "  ❌ $table: Unexpected status: $table_status"
                all_healthy=false
                ;;
        esac
        
        # Test basic read operation
        if [[ "$table_status" == "ACTIVE" ]]; then
            log "  Testing read operation on $table..."
            
            local scan_result=$(aws dynamodb scan \
                --table-name "$table" \
                --limit 1 \
                --query 'Count' \
                --output text 2>/dev/null || echo "SCAN_FAILED")
            
            if [[ "$scan_result" == "SCAN_FAILED" ]]; then
                error "  ❌ $table: Read operation failed"
                all_healthy=false
            else
                log "  ✅ $table: Read operation successful"
            fi
        fi
    done
    
    if [[ "$all_healthy" == "true" ]]; then
        pass_check "DynamoDB Tables Health"
    else
        fail_check "DynamoDB Tables Health" "One or more tables unhealthy"
        return 1
    fi
}

# Check S3 bucket health
check_s3_bucket() {
    start_check "S3 Bucket Health"
    
    log "Checking S3 bucket: $MEMORY_BUCKET"
    
    # Check if bucket exists and is accessible
    if ! aws s3 ls "s3://$MEMORY_BUCKET" &>/dev/null; then
        fail_check "S3 Bucket Health" "Bucket not accessible: $MEMORY_BUCKET"
        return 1
    fi
    
    # Get bucket metrics
    local object_count=$(aws s3 ls "s3://$MEMORY_BUCKET" --recursive | wc -l)
    local mv2_files=$(aws s3 ls "s3://$MEMORY_BUCKET" --recursive | grep "\.mv2$" | wc -l)
    
    log "  📊 Total objects: $object_count"
    log "  📊 .mv2 memory files: $mv2_files"
    
    # Test upload/download operation
    log "  Testing S3 operations..."
    
    local test_file="/tmp/mnemogram-health-test-$(date +%s).txt"
    local s3_test_key="health-check/test-$(date +%s).txt"
    
    echo "Health check test file - $(date)" > "$test_file"
    
    # Test upload
    if aws s3 cp "$test_file" "s3://$MEMORY_BUCKET/$s3_test_key" &>/dev/null; then
        log "  ✅ S3 upload successful"
        
        # Test download
        if aws s3 cp "s3://$MEMORY_BUCKET/$s3_test_key" "/tmp/download-test" &>/dev/null; then
            log "  ✅ S3 download successful"
            
            # Cleanup test files
            aws s3 rm "s3://$MEMORY_BUCKET/$s3_test_key" &>/dev/null
            rm -f "$test_file" "/tmp/download-test"
            
            pass_check "S3 Bucket Health"
        else
            error "  ❌ S3 download failed"
            fail_check "S3 Bucket Health" "Download operation failed"
            return 1
        fi
    else
        error "  ❌ S3 upload failed"
        fail_check "S3 Bucket Health" "Upload operation failed" 
        return 1
    fi
}

# Check application functionality
check_application_functionality() {
    start_check "Application Functionality"
    
    # Get API URL from CloudFormation outputs
    local api_url=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --query "Stacks[0].Outputs[?OutputKey=='ApiUrl'].OutputValue" \
        --output text 2>/dev/null)
    
    if [[ -z "$api_url" ]]; then
        fail_check "Application Functionality" "Could not retrieve API URL"
        return 1
    fi
    
    # Test status endpoint
    log "  Testing status endpoint..."
    local status_response=$(curl -s "$api_url/v1/status" --max-time 30 2>/dev/null || echo "FAILED")
    
    if [[ "$status_response" == "FAILED" ]]; then
        fail_check "Application Functionality" "Status endpoint not responding"
        return 1
    fi
    
    log "  ✅ Status endpoint responding"
    
    # Test CORS headers
    log "  Testing CORS headers..."
    local cors_headers=$(curl -s -I -H "Origin: https://mnemogram.com" \
        "$api_url/v1/status" --max-time 30 2>/dev/null | grep -i "access-control" || echo "")
    
    if [[ -n "$cors_headers" ]]; then
        log "  ✅ CORS headers present"
    else
        warn_check "Application Functionality" "CORS headers not found"
    fi
    
    # Additional endpoint tests (without auth for health check)
    log "  Testing API Gateway routing..."
    
    # Test different endpoints for routing
    local endpoints=("v1/status")
    
    for endpoint in "${endpoints[@]}"; do
        local http_code=$(curl -s -o /dev/null -w "%{http_code}" \
            --max-time 30 \
            "$api_url/$endpoint" 2>/dev/null || echo "000")
        
        if [[ "$http_code" =~ ^(200|401|403)$ ]]; then
            log "  ✅ Endpoint $endpoint: HTTP $http_code (expected)"
        else
            warn "  ⚠️  Endpoint $endpoint: HTTP $http_code (unexpected)"
        fi
    done
    
    pass_check "Application Functionality"
}

# Performance validation
check_performance() {
    start_check "Performance Validation"
    
    # Get API URL
    local api_url=$(aws cloudformation describe-stacks \
        --stack-name "$STACK_NAME" \
        --query "Stacks[0].Outputs[?OutputKey=='ApiUrl'].OutputValue" \
        --output text 2>/dev/null)
    
    if [[ -z "$api_url" ]]; then
        fail_check "Performance Validation" "Could not retrieve API URL"
        return 1
    fi
    
    log "  Testing API response times..."
    
    local total_time=0
    local successful_requests=0
    local max_time=0
    
    # Make 5 test requests
    for i in {1..5}; do
        log "    Request $i/5..."
        
        local response_time=$(curl -s -o /dev/null -w "%{time_total}" \
            --max-time 30 \
            "$api_url/v1/status" 2>/dev/null || echo "999")
        
        if [[ "$response_time" != "999" ]]; then
            total_time=$(echo "$total_time + $response_time" | bc -l)
            successful_requests=$((successful_requests + 1))
            
            # Track max time
            if (( $(echo "$response_time > $max_time" | bc -l) )); then
                max_time=$response_time
            fi
        fi
    done
    
    if [[ $successful_requests -eq 0 ]]; then
        fail_check "Performance Validation" "No successful requests"
        return 1
    fi
    
    local avg_time=$(echo "scale=3; $total_time / $successful_requests" | bc -l)
    
    log "  📊 Average response time: ${avg_time}s"
    log "  📊 Maximum response time: ${max_time}s"
    log "  📊 Successful requests: $successful_requests/5"
    
    # Performance thresholds
    if (( $(echo "$avg_time > 10.0" | bc -l) )); then
        fail_check "Performance Validation" "Average response time too high: ${avg_time}s"
        return 1
    elif (( $(echo "$avg_time > 5.0" | bc -l) )); then
        warn_check "Performance Validation" "Average response time elevated: ${avg_time}s"
    fi
    
    if (( $(echo "$max_time > 30.0" | bc -l) )); then
        fail_check "Performance Validation" "Maximum response time too high: ${max_time}s"
        return 1
    fi
    
    pass_check "Performance Validation"
}

# Generate health check report
generate_report() {
    local stage="$1"
    
    log "=== HEALTH CHECK REPORT ==="
    log "Stage: $stage"
    log "Timestamp: $(date '+%Y-%m-%d %H:%M:%S UTC')"
    log "Total Checks: $TOTAL_CHECKS"
    log "Passed: $PASSED_CHECKS"
    log "Failed: $FAILED_CHECKS" 
    log "Warnings: $WARNINGS"
    
    local success_rate=0
    if [[ $TOTAL_CHECKS -gt 0 ]]; then
        success_rate=$(echo "scale=1; $PASSED_CHECKS * 100 / $TOTAL_CHECKS" | bc -l)
    fi
    
    log "Success Rate: ${success_rate}%"
    
    if [[ $FAILED_CHECKS -eq 0 ]]; then
        if [[ $WARNINGS -eq 0 ]]; then
            success "🎉 ALL CHECKS PASSED - System is healthy!"
            return 0
        else
            warn "⚠️  ALL CHECKS PASSED with $WARNINGS warnings - System is mostly healthy"
            return 0
        fi
    else
        error "💥 $FAILED_CHECKS CHECKS FAILED - System has issues!"
        return 1
    fi
}

# Main execution function
main() {
    local stage="${1:-}"
    local timeout="$DEFAULT_TIMEOUT"
    local verbose="false"
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --timeout=*)
                timeout="${1#*=}"
                shift
                ;;
            --verbose)
                verbose="true"
                shift
                ;;
            --help)
                show_help
                exit 0
                ;;
            *)
                if [[ -z "$stage" ]]; then
                    stage="$1"
                fi
                shift
                ;;
        esac
    done
    
    # Validate required parameters
    if [[ -z "$stage" ]]; then
        echo "Error: Missing required stage argument"
        show_help
        exit 1
    fi
    
    if [[ ! "$stage" =~ ^(dev|staging|prod)$ ]]; then
        error "Invalid stage. Must be: dev, staging, or prod"
        exit 1
    fi
    
    log "Starting Mnemogram health check"
    log "Stage: $stage"
    log "Timeout: ${timeout}s"
    log "Verbose: $verbose"
    log "Log file: $LOG_FILE"
    
    # Set AWS profile if not set
    export AWS_PROFILE="${AWS_PROFILE:-mnemogram-deploy}"
    
    # Get AWS context and resource names
    get_aws_context
    get_resource_names "$stage"
    
    # Run health checks with timeout
    local start_time=$(date +%s)
    
    (
        # CloudFormation stack check
        check_cloudformation_stack
        
        # API Gateway health
        check_api_gateway
        
        # Lambda functions health
        check_lambda_functions
        
        # DynamoDB health
        check_dynamodb_tables
        
        # S3 bucket health
        check_s3_bucket
        
        # Application functionality
        check_application_functionality
        
        # Performance validation
        check_performance
        
    ) &
    
    local check_pid=$!
    local elapsed=0
    
    # Monitor timeout
    while kill -0 $check_pid 2>/dev/null; do
        sleep 5
        elapsed=$(($(date +%s) - start_time))
        
        if [[ $elapsed -gt $timeout ]]; then
            kill -TERM $check_pid 2>/dev/null || true
            sleep 2
            kill -KILL $check_pid 2>/dev/null || true
            error "Health check timed out after ${timeout}s"
            exit 1
        fi
        
        if [[ "$verbose" == "true" ]]; then
            log "Health check running... ${elapsed}s elapsed"
        fi
    done
    
    wait $check_pid
    local check_result=$?
    
    # Generate final report
    generate_report "$stage"
    
    log "Health check completed in ${elapsed}s"
    log "Full log available at: $LOG_FILE"
    
    exit $check_result
}

# Execute main function with all arguments
main "$@"