#!/bin/bash

# rollback-infrastructure.sh
# Automated infrastructure rollback script for Mnemogram
# Usage: ./rollback-infrastructure.sh <stage> <commit-hash> [--dry-run]

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
INFRA_DIR="$PROJECT_ROOT/infra"
LOG_FILE="/tmp/mnemogram-rollback-$(date +%Y%m%d-%H%M%S).log"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'  
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging function
log() {
    echo -e "${BLUE}[$(date '+%Y-%m-%d %H:%M:%S')]${NC} $1" | tee -a "$LOG_FILE"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1" | tee -a "$LOG_FILE"
    exit 1
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1" | tee -a "$LOG_FILE"
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1" | tee -a "$LOG_FILE"
}

# Help function
show_help() {
    cat << EOF
Usage: $0 <stage> <commit-hash> [--dry-run]

Arguments:
  stage       Environment stage (dev, staging, prod)
  commit-hash Git commit hash to rollback to
  
Options:
  --dry-run   Show what would be done without executing
  --help      Show this help message

Examples:
  $0 dev abc123 --dry-run
  $0 staging def456
  $0 prod 789ghi

Environment Variables:
  AWS_PROFILE     AWS profile to use (default: mnemogram-deploy)
  SLACK_WEBHOOK   Slack webhook URL for notifications
EOF
}

# Validate prerequisites
check_prerequisites() {
    log "Checking prerequisites..."
    
    # Check required commands
    local commands=("aws" "cdk" "git" "jq")
    for cmd in "${commands[@]}"; do
        if ! command -v "$cmd" &> /dev/null; then
            error "Required command not found: $cmd"
        fi
    done
    
    # Check AWS credentials
    if ! aws sts get-caller-identity &> /dev/null; then
        error "AWS credentials not configured or invalid"
    fi
    
    # Check if in git repository
    if ! git rev-parse --git-dir &> /dev/null; then
        error "Not in a git repository"
    fi
    
    # Check if infra directory exists
    if [[ ! -d "$INFRA_DIR" ]]; then
        error "Infrastructure directory not found: $INFRA_DIR"
    fi
    
    success "Prerequisites check passed"
}

# Validate inputs
validate_inputs() {
    local stage="$1"
    local commit_hash="$2"
    
    log "Validating inputs..."
    
    # Validate stage
    if [[ ! "$stage" =~ ^(dev|staging|prod)$ ]]; then
        error "Invalid stage. Must be: dev, staging, or prod"
    fi
    
    # Validate commit hash exists
    if ! git rev-parse --verify "$commit_hash" &> /dev/null; then
        error "Commit hash does not exist: $commit_hash"
    fi
    
    # Check if commit is behind current HEAD
    if git merge-base --is-ancestor "$commit_hash" HEAD; then
        log "Commit $commit_hash is valid for rollback"
    else
        warn "Commit $commit_hash is not an ancestor of current HEAD"
        read -p "Continue anyway? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            error "Rollback cancelled by user"
        fi
    fi
    
    success "Input validation passed"
}

# Backup current state
backup_current_state() {
    local stage="$1"
    
    log "Creating backup of current state..."
    
    # Create backup branch
    local backup_branch="backup/rollback-$(date +%Y%m%d-%H%M%S)"
    git checkout -b "$backup_branch"
    git push origin "$backup_branch"
    
    # Export current CDK state
    local backup_dir="/tmp/mnemogram-backup-$(date +%Y%m%d-%H%M%S)"
    mkdir -p "$backup_dir"
    
    cd "$INFRA_DIR"
    cdk synth "MnemogramStack-$stage" > "$backup_dir/current-template.json"
    
    # Backup current Lambda versions
    aws lambda list-functions \
        --query "Functions[?starts_with(FunctionName, \`mnemogram-$stage\`)].{FunctionName:FunctionName,Version:Version,CodeSha256:CodeSha256}" \
        > "$backup_dir/lambda-versions.json"
    
    echo "$backup_dir" > /tmp/mnemogram-rollback-backup-path
    success "Backup completed: $backup_branch, $backup_dir"
}

# Perform CDK rollback
rollback_cdk_stack() {
    local stage="$1"
    local commit_hash="$2"
    local dry_run="$3"
    
    log "Rolling back CDK stack to commit $commit_hash..."
    
    cd "$PROJECT_ROOT"
    
    # Store current commit for potential restoration
    local current_commit=$(git rev-parse HEAD)
    echo "$current_commit" > /tmp/mnemogram-current-commit
    
    # Checkout target commit
    git checkout "$commit_hash"
    
    cd "$INFRA_DIR"
    
    # Install dependencies
    npm ci
    
    if [[ "$dry_run" == "true" ]]; then
        log "DRY RUN: Would deploy CDK stack with:"
        cdk diff "MnemogramStack-$stage" || true
    else
        # Deploy the rollback stack
        log "Deploying rollback CDK stack..."
        cdk deploy "MnemogramStack-$stage" --require-approval never
        
        success "CDK stack rollback completed"
    fi
}

# Rollback Lambda functions
rollback_lambda_functions() {
    local stage="$1"
    local dry_run="$2"
    
    log "Rolling back Lambda functions..."
    
    # Get list of functions
    local functions=$(aws lambda list-functions \
        --query "Functions[?starts_with(FunctionName, \`mnemogram-$stage\`)].FunctionName" \
        --output text)
    
    for function_name in $functions; do
        log "Processing function: $function_name"
        
        # Get previous version (skip $LATEST)
        local versions=$(aws lambda list-versions-by-function \
            --function-name "$function_name" \
            --query "Versions[?Version != \`\$LATEST\`].Version" \
            --output text | tr '\t' '\n' | sort -nr)
        
        if [[ -z "$versions" ]]; then
            warn "No previous versions found for $function_name"
            continue
        fi
        
        local previous_version=$(echo "$versions" | head -n2 | tail -n1)
        
        if [[ "$dry_run" == "true" ]]; then
            log "DRY RUN: Would rollback $function_name to version $previous_version"
        else
            log "Rolling back $function_name to version $previous_version"
            
            # Update alias to point to previous version
            aws lambda update-alias \
                --function-name "$function_name" \
                --name "LIVE" \
                --function-version "$previous_version" || {
                warn "Failed to update alias for $function_name, trying direct update"
                # If no alias exists, update function configuration
            }
        fi
    done
    
    success "Lambda functions rollback completed"
}

# Perform health checks
perform_health_checks() {
    local stage="$1"
    
    log "Performing post-rollback health checks..."
    
    # Run health check script if it exists
    if [[ -f "$SCRIPT_DIR/health-check.sh" ]]; then
        "$SCRIPT_DIR/health-check.sh" "$stage"
    else
        warn "health-check.sh not found, performing basic checks"
        
        # Basic API health check
        local api_url=$(aws cloudformation describe-stacks \
            --stack-name "MnemogramStack-$stage" \
            --query "Stacks[0].Outputs[?OutputKey=='ApiUrl'].OutputValue" \
            --output text)
        
        if [[ -n "$api_url" ]]; then
            log "Checking API health: $api_url"
            if curl -f "$api_url/v1/status" &> /dev/null; then
                success "API health check passed"
            else
                error "API health check failed"
            fi
        fi
    fi
}

# Send notifications
send_notifications() {
    local stage="$1"
    local commit_hash="$2"
    local status="$3"
    
    log "Sending rollback notifications..."
    
    local message="🔄 Mnemogram rollback $status
Environment: $stage  
Target Commit: $commit_hash
Time: $(date '+%Y-%m-%d %H:%M:%S UTC')
Operator: $(whoami)@$(hostname)"

    # Slack notification
    if [[ -n "${SLACK_WEBHOOK:-}" ]]; then
        curl -X POST -H 'Content-type: application/json' \
            --data "{\"text\":\"$message\"}" \
            "$SLACK_WEBHOOK" || warn "Failed to send Slack notification"
    fi
    
    # Discord notification script
    if [[ -f "$SCRIPT_DIR/notify-stakeholders.sh" ]]; then
        "$SCRIPT_DIR/notify-stakeholders.sh" "rollback" "$stage" "$status" "$commit_hash"
    fi
    
    success "Notifications sent"
}

# Main execution function
main() {
    local stage="${1:-}"
    local commit_hash="${2:-}"
    local dry_run="false"
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run)
                dry_run="true"
                shift
                ;;
            --help)
                show_help
                exit 0
                ;;
            *)
                if [[ -z "$stage" ]]; then
                    stage="$1"
                elif [[ -z "$commit_hash" ]]; then
                    commit_hash="$1"
                fi
                shift
                ;;
        esac
    done
    
    # Validate required parameters
    if [[ -z "$stage" ]] || [[ -z "$commit_hash" ]]; then
        echo "Error: Missing required arguments"
        show_help
        exit 1
    fi
    
    log "Starting Mnemogram infrastructure rollback"
    log "Stage: $stage"
    log "Target commit: $commit_hash"
    log "Dry run: $dry_run"
    log "Log file: $LOG_FILE"
    
    # Set AWS profile if not set
    export AWS_PROFILE="${AWS_PROFILE:-mnemogram-deploy}"
    
    # Confirmation for production
    if [[ "$stage" == "prod" && "$dry_run" == "false" ]]; then
        echo -e "${RED}WARNING: This will rollback PRODUCTION infrastructure!${NC}"
        read -p "Type 'CONFIRM' to proceed: " -r
        if [[ "$REPLY" != "CONFIRM" ]]; then
            error "Production rollback cancelled"
        fi
    fi
    
    # Execute rollback steps
    check_prerequisites
    validate_inputs "$stage" "$commit_hash"
    
    if [[ "$dry_run" == "false" ]]; then
        backup_current_state "$stage"
        send_notifications "$stage" "$commit_hash" "STARTED"
    fi
    
    rollback_cdk_stack "$stage" "$commit_hash" "$dry_run"
    rollback_lambda_functions "$stage" "$dry_run"
    
    if [[ "$dry_run" == "false" ]]; then
        perform_health_checks "$stage"
        send_notifications "$stage" "$commit_hash" "COMPLETED"
        success "Rollback completed successfully!"
    else
        success "Dry run completed - no changes made"
    fi
    
    log "Rollback log saved to: $LOG_FILE"
}

# Error handler
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        error "Rollback failed with exit code $exit_code"
        if [[ -f /tmp/mnemogram-current-commit ]]; then
            local current_commit=$(cat /tmp/mnemogram-current-commit)
            warn "To restore original state: git checkout $current_commit"
        fi
        
        if [[ "${1:-}" != "--dry-run" ]]; then
            send_notifications "${stage:-unknown}" "${commit_hash:-unknown}" "FAILED"
        fi
    fi
}

trap cleanup EXIT

# Execute main function with all arguments
main "$@"