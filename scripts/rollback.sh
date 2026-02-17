#!/bin/bash

# rollback.sh
# Master rollback orchestration script for Mnemogram
# Usage: ./rollback.sh <stage> <commit-hash> [options]

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOG_FILE="/tmp/mnemogram-master-rollback-$(date +%Y%m%d-%H%M%S).log"
ROLLBACK_STATE_FILE="/tmp/mnemogram-rollback-state-$(date +%Y%m%d-%H%M%S).json"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
NC='\033[0m' # No Color

# Logging functions
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

header() {
    echo -e "${PURPLE}[ROLLBACK]${NC} $1" | tee -a "$LOG_FILE"
}

# Global state tracking
update_state() {
    local step="$1"
    local status="$2"
    local details="${3:-}"
    
    local timestamp=$(date -u '+%Y-%m-%dT%H:%M:%S.%3NZ')
    
    # Create or update state file
    local state=$(cat "$ROLLBACK_STATE_FILE" 2>/dev/null || echo '{"steps": {}}')
    local updated_state=$(echo "$state" | jq \
        --arg step "$step" \
        --arg status "$status" \
        --arg timestamp "$timestamp" \
        --arg details "$details" \
        '.steps[$step] = {status: $status, timestamp: $timestamp, details: $details}')
    
    echo "$updated_state" > "$ROLLBACK_STATE_FILE"
    log "State updated: $step = $status"
}

# Help function
show_help() {
    cat << EOF
Mnemogram Master Rollback Script

Usage: $0 <stage> <commit-hash> [options]

Arguments:
  stage       Environment stage (dev, staging, prod)
  commit-hash Git commit hash to rollback to

Options:
  --dry-run           Show what would be done without executing
  --data-timestamp    Specific timestamp for data rollback (ISO 8601)
  --skip-data         Skip data rollback (infrastructure only)
  --skip-infra        Skip infrastructure rollback (data only)  
  --force             Skip confirmation prompts (except prod)
  --timeout           Health check timeout in seconds (default: 300)
  --help              Show this help message

Rollback Components:
  1. Infrastructure (CDK stacks, Lambda functions)
  2. Data (S3 objects, DynamoDB tables) 
  3. Health validation
  4. Monitoring and alerts

Examples:
  # Full rollback with dry run
  $0 dev abc123 --dry-run
  
  # Production rollback to specific commit and data timestamp
  $0 prod def456 --data-timestamp "2026-02-17T15:00:00Z"
  
  # Infrastructure-only rollback
  $0 staging ghi789 --skip-data
  
  # Quick dev rollback with force
  $0 dev jkl012 --force

Environment Variables:
  AWS_PROFILE          AWS profile to use (default: mnemogram-deploy)
  SLACK_WEBHOOK        Slack webhook URL for notifications
  DISCORD_WEBHOOK      Discord webhook URL for notifications  
  ROLLBACK_TIMEOUT     Default timeout for operations (default: 600s)

State Tracking:
  Rollback state is saved to: $ROLLBACK_STATE_FILE
  Full log available at: $LOG_FILE

For emergency rollback assistance, contact Stuart Bain.
EOF
}

# Initialize rollback state
init_rollback_state() {
    local stage="$1"
    local commit_hash="$2"
    local dry_run="$3"
    
    cat > "$ROLLBACK_STATE_FILE" << EOF
{
  "rollback_id": "$(uuidgen 2>/dev/null || date +%s | sha256sum | head -c8)",
  "stage": "$stage",
  "target_commit": "$commit_hash", 
  "dry_run": $dry_run,
  "started_at": "$(date -u '+%Y-%m-%dT%H:%M:%S.%3NZ')",
  "initiated_by": "$(whoami)@$(hostname)",
  "steps": {}
}
EOF
    
    log "Rollback state initialized: $ROLLBACK_STATE_FILE"
}

# Validate prerequisites
check_prerequisites() {
    update_state "prerequisites" "running" "Checking system requirements"
    
    log "Validating prerequisites..."
    
    # Check if rollback scripts exist
    local required_scripts=(
        "$SCRIPT_DIR/rollback-infrastructure.sh"
        "$SCRIPT_DIR/rollback-data.sh" 
        "$SCRIPT_DIR/health-check.sh"
    )
    
    for script in "${required_scripts[@]}"; do
        if [[ ! -f "$script" ]]; then
            update_state "prerequisites" "failed" "Missing script: $script"
            error "Required script not found: $script"
        fi
        
        if [[ ! -x "$script" ]]; then
            warn "Script not executable, fixing: $script"
            chmod +x "$script"
        fi
    done
    
    # Check system dependencies
    local commands=("aws" "git" "jq" "curl")
    for cmd in "${commands[@]}"; do
        if ! command -v "$cmd" &> /dev/null; then
            update_state "prerequisites" "failed" "Missing command: $cmd"
            error "Required command not found: $cmd"
        fi
    done
    
    # Verify AWS access
    if ! aws sts get-caller-identity &> /dev/null; then
        update_state "prerequisites" "failed" "AWS credentials invalid"
        error "AWS credentials not configured or invalid"
    fi
    
    update_state "prerequisites" "completed" "All prerequisites satisfied"
    success "Prerequisites validation passed"
}

# Send notifications
send_notifications() {
    local phase="$1"  # START, SUCCESS, FAILED
    local stage="$2"
    local commit_hash="$3"
    local details="${4:-}"
    
    update_state "notifications" "running" "Sending $phase notifications"
    
    local emoji=""
    local color=""
    case "$phase" in
        "START") emoji="🚀"; color="warning" ;;
        "SUCCESS") emoji="✅"; color="good" ;;
        "FAILED") emoji="❌"; color="danger" ;;
    esac
    
    local message="$emoji Mnemogram Rollback $phase
Environment: $stage
Target Commit: $commit_hash  
Time: $(date '+%Y-%m-%d %H:%M:%S UTC')
Operator: $(whoami)@$(hostname)
Log: $LOG_FILE
State: $ROLLBACK_STATE_FILE
$details"

    # Slack notification
    if [[ -n "${SLACK_WEBHOOK:-}" ]]; then
        curl -X POST -H 'Content-type: application/json' \
            --data "{\"text\":\"$message\", \"color\":\"$color\"}" \
            "$SLACK_WEBHOOK" &> /dev/null || warn "Slack notification failed"
    fi
    
    # Discord notification  
    if [[ -n "${DISCORD_WEBHOOK:-}" ]]; then
        curl -X POST -H 'Content-Type: application/json' \
            --data "{\"content\":\"$message\"}" \
            "$DISCORD_WEBHOOK" &> /dev/null || warn "Discord notification failed"
    fi
    
    update_state "notifications" "completed" "$phase notifications sent"
}

# Infrastructure rollback
rollback_infrastructure() {
    local stage="$1"
    local commit_hash="$2" 
    local dry_run_flag="$3"
    
    header "Starting infrastructure rollback..."
    update_state "infrastructure" "running" "Rolling back CDK and Lambda functions"
    
    local cmd="$SCRIPT_DIR/rollback-infrastructure.sh $stage $commit_hash"
    if [[ "$dry_run_flag" == "true" ]]; then
        cmd="$cmd --dry-run"
    fi
    
    log "Executing: $cmd"
    
    if $cmd 2>&1 | tee -a "$LOG_FILE"; then
        update_state "infrastructure" "completed" "Infrastructure rollback successful"
        success "Infrastructure rollback completed"
    else
        update_state "infrastructure" "failed" "Infrastructure rollback failed"
        error "Infrastructure rollback failed"
    fi
}

# Data rollback
rollback_data() {
    local stage="$1"
    local data_timestamp="$2"
    local dry_run_flag="$3"
    
    header "Starting data rollback..."
    update_state "data" "running" "Rolling back S3 and DynamoDB data to $data_timestamp"
    
    local cmd="$SCRIPT_DIR/rollback-data.sh $stage $data_timestamp"
    if [[ "$dry_run_flag" == "true" ]]; then
        cmd="$cmd --dry-run"
    fi
    
    log "Executing: $cmd"
    
    if $cmd 2>&1 | tee -a "$LOG_FILE"; then
        update_state "data" "completed" "Data rollback successful"
        success "Data rollback completed"
    else
        update_state "data" "failed" "Data rollback failed" 
        error "Data rollback failed"
    fi
}

# Health validation
validate_rollback() {
    local stage="$1"
    local timeout="${2:-300}"
    
    header "Starting rollback validation..."
    update_state "validation" "running" "Running health checks with ${timeout}s timeout"
    
    local cmd="$SCRIPT_DIR/health-check.sh $stage --timeout=$timeout --verbose"
    
    log "Executing: $cmd"
    
    if $cmd 2>&1 | tee -a "$LOG_FILE"; then
        update_state "validation" "completed" "All health checks passed"
        success "Rollback validation completed"
    else
        update_state "validation" "failed" "Health checks failed"
        error "Rollback validation failed"
    fi
}

# Main rollback orchestration
main() {
    local stage=""
    local commit_hash=""
    local data_timestamp=""
    local dry_run="false"
    local skip_data="false"
    local skip_infra="false"  
    local force="false"
    local timeout="300"
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run)
                dry_run="true"
                shift
                ;;
            --data-timestamp)
                data_timestamp="$2"
                shift 2
                ;;
            --skip-data)
                skip_data="true"
                shift
                ;;
            --skip-infra)
                skip_infra="true"
                shift
                ;;
            --force)
                force="true"
                shift
                ;;
            --timeout)
                timeout="$2"
                shift 2
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
        error "Missing required arguments. Use --help for usage information."
    fi
    
    # Set default data timestamp if not provided
    if [[ -z "$data_timestamp" ]]; then
        data_timestamp=$(git show --format="%aI" "$commit_hash" --no-patch)
        log "Using commit timestamp for data rollback: $data_timestamp"
    fi
    
    # Set AWS profile
    export AWS_PROFILE="${AWS_PROFILE:-mnemogram-deploy}"
    
    # Initialize
    header "=== MNEMOGRAM MASTER ROLLBACK ==="
    log "Stage: $stage"  
    log "Target commit: $commit_hash"
    log "Data timestamp: $data_timestamp"
    log "Dry run: $dry_run"
    log "Skip data: $skip_data"
    log "Skip infrastructure: $skip_infra"
    log "Force mode: $force"
    log "Timeout: ${timeout}s"
    
    init_rollback_state "$stage" "$commit_hash" "$dry_run"
    
    # Production confirmation
    if [[ "$stage" == "prod" && "$dry_run" == "false" && "$force" == "false" ]]; then
        echo -e "${RED}⚠️  PRODUCTION ROLLBACK WARNING ⚠️${NC}"
        echo "This will rollback PRODUCTION systems to commit: $commit_hash"
        echo "Data will be restored to timestamp: $data_timestamp"
        echo ""
        read -p "Type 'ROLLBACK PRODUCTION' to confirm: " -r
        if [[ "$REPLY" != "ROLLBACK PRODUCTION" ]]; then
            error "Production rollback cancelled by user"
        fi
    fi
    
    # Start rollback process
    send_notifications "START" "$stage" "$commit_hash"
    
    check_prerequisites
    
    # Execute rollback steps
    local rollback_successful="true"
    
    if [[ "$skip_infra" == "false" ]]; then
        rollback_infrastructure "$stage" "$commit_hash" "$dry_run" || rollback_successful="false"
    else
        log "Skipping infrastructure rollback (--skip-infra)"
        update_state "infrastructure" "skipped" "Skipped by user request"
    fi
    
    if [[ "$skip_data" == "false" && "$rollback_successful" == "true" ]]; then
        rollback_data "$stage" "$data_timestamp" "$dry_run" || rollback_successful="false"
    elif [[ "$skip_data" == "true" ]]; then
        log "Skipping data rollback (--skip-data)"
        update_state "data" "skipped" "Skipped by user request"
    fi
    
    # Validate rollback (skip if dry run or if previous steps failed)
    if [[ "$dry_run" == "false" && "$rollback_successful" == "true" ]]; then
        validate_rollback "$stage" "$timeout" || rollback_successful="false"
    elif [[ "$dry_run" == "true" ]]; then
        log "Skipping validation (dry run mode)"
        update_state "validation" "skipped" "Dry run mode"
    fi
    
    # Final state update and notifications
    if [[ "$rollback_successful" == "true" ]]; then
        if [[ "$dry_run" == "true" ]]; then
            header "🎯 DRY RUN COMPLETED SUCCESSFULLY"
            success "Rollback dry run completed - no changes made"
            send_notifications "SUCCESS" "$stage" "$commit_hash" "Dry run completed successfully"
        else
            header "🎉 ROLLBACK COMPLETED SUCCESSFULLY"
            success "Mnemogram rollback completed successfully!"
            send_notifications "SUCCESS" "$stage" "$commit_hash" "Rollback completed successfully"
        fi
    else
        header "💥 ROLLBACK FAILED"
        send_notifications "FAILED" "$stage" "$commit_hash" "Rollback failed - see logs for details"
        error "Rollback failed - check logs for details"
    fi
    
    log "Master rollback log: $LOG_FILE"
    log "Rollback state file: $ROLLBACK_STATE_FILE"
}

# Cleanup function
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        update_state "rollback" "failed" "Rollback terminated with exit code $exit_code" 2>/dev/null || true
    else
        update_state "rollback" "completed" "Rollback finished successfully" 2>/dev/null || true
    fi
}

trap cleanup EXIT

# Execute main function
main "$@"