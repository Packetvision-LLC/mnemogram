#!/bin/bash

# rollback-data.sh  
# Automated data rollback script for Mnemogram S3 and DynamoDB
# Usage: ./rollback-data.sh <stage> <timestamp> [--dry-run]

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="/tmp/mnemogram-data-rollback-$(date +%Y%m%d-%H%M%S).log"

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
Usage: $0 <stage> <timestamp> [--dry-run] [--s3-only] [--dynamodb-only]

Arguments:
  stage     Environment stage (dev, staging, prod)
  timestamp Rollback timestamp (ISO 8601 format: 2026-02-17T19:00:00Z)

Options:
  --dry-run        Show what would be done without executing
  --s3-only        Only rollback S3 data
  --dynamodb-only  Only rollback DynamoDB data
  --help           Show this help message

Examples:
  $0 dev 2026-02-17T18:00:00Z --dry-run
  $0 staging 2026-02-17T12:00:00Z --s3-only
  $0 prod 2026-02-17T10:30:00Z

Environment Variables:
  AWS_PROFILE      AWS profile to use (default: mnemogram-deploy)
  BACKUP_BUCKET    S3 bucket for backups (default: mnemogram-{stage}-backups)
EOF
}

# Get AWS account and region
get_aws_context() {
    AWS_ACCOUNT=$(aws sts get-caller-identity --query Account --output text)
    AWS_REGION=$(aws configure get region || echo "us-east-1")
    
    log "AWS Context: Account $AWS_ACCOUNT, Region $AWS_REGION"
}

# Get bucket and table names for stage
get_resource_names() {
    local stage="$1"
    
    # S3 bucket
    MEMORY_BUCKET="mnemogram-${stage}-memories-${AWS_ACCOUNT}-${AWS_REGION}"
    BACKUP_BUCKET="${BACKUP_BUCKET:-mnemogram-${stage}-backups-${AWS_ACCOUNT}-${AWS_REGION}}"
    
    # DynamoDB tables
    METADATA_TABLE="mnemogram-${stage}-metadata"
    MEMORIES_TABLE="mnemogram-${stage}-memories"
    SUBSCRIPTIONS_TABLE="mnemogram-${stage}-subscriptions"
    API_KEYS_TABLE="mnemogram-${stage}-api-keys"
    USAGE_TABLE="mnemogram-${stage}-usage"
    THRESHOLD_TABLE="mnemogram-${stage}-threshold-tracking"
    
    log "Resource names configured for stage: $stage"
}

# Validate prerequisites
check_prerequisites() {
    log "Checking prerequisites..."
    
    # Check required commands
    local commands=("aws" "jq" "date")
    for cmd in "${commands[@]}"; do
        if ! command -v "$cmd" &> /dev/null; then
            error "Required command not found: $cmd"
        fi
    done
    
    # Check AWS credentials
    if ! aws sts get-caller-identity &> /dev/null; then
        error "AWS credentials not configured or invalid"
    fi
    
    success "Prerequisites check passed"
}

# Validate timestamp format
validate_timestamp() {
    local timestamp="$1"
    
    log "Validating timestamp format..."
    
    # Check ISO 8601 format
    if ! date -d "$timestamp" &> /dev/null; then
        error "Invalid timestamp format. Use ISO 8601: YYYY-MM-DDTHH:MM:SSZ"
    fi
    
    # Check if timestamp is in the past
    local ts_epoch=$(date -d "$timestamp" +%s)
    local now_epoch=$(date +%s)
    
    if [[ $ts_epoch -gt $now_epoch ]]; then
        error "Timestamp cannot be in the future"
    fi
    
    # Check if timestamp is reasonable (not too old)
    local days_old=$(( (now_epoch - ts_epoch) / 86400 ))
    if [[ $days_old -gt 30 ]]; then
        warn "Timestamp is $days_old days old. Data may not be available."
        read -p "Continue anyway? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            error "Rollback cancelled by user"
        fi
    fi
    
    success "Timestamp validation passed"
}

# Backup current S3 state
backup_s3_current_state() {
    local dry_run="$1"
    
    log "Backing up current S3 state..."
    
    local backup_prefix="rollback-backup/$(date +%Y%m%d-%H%M%S)"
    
    if [[ "$dry_run" == "true" ]]; then
        log "DRY RUN: Would backup S3 bucket $MEMORY_BUCKET to $backup_prefix"
    else
        # Create backup bucket if it doesn't exist
        if ! aws s3 ls "s3://$BACKUP_BUCKET" &> /dev/null; then
            aws s3 mb "s3://$BACKUP_BUCKET"
            log "Created backup bucket: $BACKUP_BUCKET"
        fi
        
        # Sync current state to backup
        aws s3 sync "s3://$MEMORY_BUCKET" "s3://$BACKUP_BUCKET/$backup_prefix/" \
            --exclude "*.tmp" --exclude "*.log"
        
        echo "$backup_prefix" > /tmp/mnemogram-s3-backup-prefix
        success "S3 backup completed: s3://$BACKUP_BUCKET/$backup_prefix/"
    fi
}

# Rollback S3 data using versioning
rollback_s3_versioned() {
    local stage="$1"
    local timestamp="$2"
    local dry_run="$3"
    
    log "Rolling back S3 data using versioning..."
    
    # Convert timestamp to epoch for comparison
    local target_epoch=$(date -d "$timestamp" +%s)
    
    # List all objects with versions
    local objects=$(aws s3api list-object-versions \
        --bucket "$MEMORY_BUCKET" \
        --query 'Versions[?IsLatest==`false`].[Key,VersionId,LastModified]' \
        --output json)
    
    if [[ "$objects" == "null" || "$objects" == "[]" ]]; then
        warn "No object versions found for rollback"
        return 0
    fi
    
    echo "$objects" | jq -r '.[] | @tsv' | while IFS=$'\t' read -r key version_id last_modified; do
        # Convert last_modified to epoch
        local obj_epoch=$(date -d "$last_modified" +%s)
        
        # Find the latest version before target timestamp
        if [[ $obj_epoch -le $target_epoch ]]; then
            if [[ "$dry_run" == "true" ]]; then
                log "DRY RUN: Would restore $key to version $version_id ($last_modified)"
            else
                log "Restoring $key to version $version_id ($last_modified)"
                
                # Copy the specific version to be current
                aws s3api copy-object \
                    --bucket "$MEMORY_BUCKET" \
                    --copy-source "$MEMORY_BUCKET/$key?versionId=$version_id" \
                    --key "$key"
            fi
        fi
    done
    
    success "S3 versioning rollback completed"
}

# Rollback S3 from backup
rollback_s3_from_backup() {
    local stage="$1"
    local timestamp="$2"
    local dry_run="$3"
    
    log "Rolling back S3 data from backup..."
    
    # Find the closest backup before timestamp
    local backups=$(aws s3api list-objects-v2 \
        --bucket "$BACKUP_BUCKET" \
        --prefix "scheduled-backup/" \
        --query 'Contents[].Key' \
        --output text)
    
    local best_backup=""
    local target_epoch=$(date -d "$timestamp" +%s)
    
    for backup in $backups; do
        # Extract timestamp from backup key (format: scheduled-backup/YYYYMMDD-HHMMSS/)
        if [[ $backup =~ scheduled-backup/([0-9]{8}-[0-9]{6})/ ]]; then
            local backup_ts="${BASH_REMATCH[1]}"
            local backup_epoch=$(date -d "${backup_ts:0:8} ${backup_ts:9:2}:${backup_ts:11:2}:${backup_ts:13:2}" +%s)
            
            if [[ $backup_epoch -le $target_epoch ]]; then
                if [[ -z "$best_backup" ]] || [[ $backup_epoch -gt $(date -d "$(echo $best_backup | sed 's/.*\/\([0-9-]*\)\/.*/\1/' | sed 's/\(.*\)-\(.*\)/\1 \2/' | sed 's/\(..\)\(..\)\(..\)/\1:\2:\3/')" +%s) ]]; then
                    best_backup="$backup"
                fi
            fi
        fi
    done
    
    if [[ -z "$best_backup" ]]; then
        error "No suitable backup found before timestamp $timestamp"
    fi
    
    log "Using backup: $best_backup"
    
    if [[ "$dry_run" == "true" ]]; then
        log "DRY RUN: Would restore from s3://$BACKUP_BUCKET/$best_backup"
    else
        # Clear current bucket (with confirmation)
        if [[ "$stage" == "prod" ]]; then
            echo -e "${RED}WARNING: This will DELETE all current data in production!${NC}"
            read -p "Type 'DELETE' to confirm: " -r
            if [[ "$REPLY" != "DELETE" ]]; then
                error "Production data rollback cancelled"
            fi
        fi
        
        # Delete current objects
        aws s3 rm "s3://$MEMORY_BUCKET" --recursive
        
        # Restore from backup
        aws s3 sync "s3://$BACKUP_BUCKET/$best_backup" "s3://$MEMORY_BUCKET/"
        
        success "S3 backup restore completed"
    fi
}

# Rollback DynamoDB using point-in-time recovery
rollback_dynamodb() {
    local stage="$1"  
    local timestamp="$2"
    local dry_run="$3"
    
    log "Rolling back DynamoDB tables using point-in-time recovery..."
    
    local tables=("$METADATA_TABLE" "$MEMORIES_TABLE" "$SUBSCRIPTIONS_TABLE" "$API_KEYS_TABLE" "$USAGE_TABLE" "$THRESHOLD_TABLE")
    
    for table in "${tables[@]}"; do
        log "Processing table: $table"
        
        # Check if point-in-time recovery is enabled
        local pitr_status=$(aws dynamodb describe-continuous-backups \
            --table-name "$table" \
            --query 'ContinuousBackupsDescription.PointInTimeRecoveryDescription.PointInTimeRecoveryStatus' \
            --output text)
        
        if [[ "$pitr_status" != "ENABLED" ]]; then
            error "Point-in-time recovery not enabled for table: $table"
        fi
        
        # Check if timestamp is within recovery window
        local earliest_recovery=$(aws dynamodb describe-continuous-backups \
            --table-name "$table" \
            --query 'ContinuousBackupsDescription.PointInTimeRecoveryDescription.EarliestRestorableDateTime' \
            --output text)
        
        local target_epoch=$(date -d "$timestamp" +%s)
        local earliest_epoch=$(date -d "$earliest_recovery" +%s)
        
        if [[ $target_epoch -lt $earliest_epoch ]]; then
            error "Timestamp $timestamp is before earliest restorable time for $table: $earliest_recovery"
        fi
        
        if [[ "$dry_run" == "true" ]]; then
            log "DRY RUN: Would restore $table to $timestamp"
            continue
        fi
        
        # Create restore table
        local restore_table="${table}-restored-$(date +%Y%m%d%H%M%S)"
        
        log "Restoring $table to $restore_table at timestamp $timestamp"
        
        aws dynamodb restore-table-to-point-in-time \
            --source-table-name "$table" \
            --target-table-name "$restore_table" \
            --restore-date-time "$timestamp"
        
        # Wait for restore to complete
        log "Waiting for restore to complete..."
        aws dynamodb wait table-exists --table-name "$restore_table"
        
        # Store restore table name for later swapping
        echo "$table:$restore_table" >> /tmp/mnemogram-restored-tables
        
        success "Table $table restored to $restore_table"
    done
    
    success "DynamoDB rollback completed"
}

# Swap restored DynamoDB tables
swap_dynamodb_tables() {
    local stage="$1"
    local dry_run="$2"
    
    if [[ ! -f /tmp/mnemogram-restored-tables ]]; then
        warn "No restored tables to swap"
        return 0
    fi
    
    log "Swapping restored DynamoDB tables..."
    
    while IFS=':' read -r original_table restore_table; do
        if [[ "$dry_run" == "true" ]]; then
            log "DRY RUN: Would swap $original_table with $restore_table"
            continue
        fi
        
        log "Swapping $original_table with $restore_table"
        
        # Backup original table
        local backup_table="${original_table}-backup-$(date +%Y%m%d%H%M%S)"
        
        # This is a complex operation requiring application downtime
        # In practice, you would:
        # 1. Stop applications
        # 2. Rename original table to backup
        # 3. Rename restored table to original name
        # 4. Update CDK/application configs
        # 5. Restart applications
        
        warn "Table swapping requires manual intervention and application downtime"
        warn "Manual steps required:"
        warn "1. Stop applications using $original_table"
        warn "2. Rename $original_table to $backup_table"
        warn "3. Rename $restore_table to $original_table"
        warn "4. Update application configurations"
        warn "5. Restart applications"
        
    done < /tmp/mnemogram-restored-tables
    
    success "DynamoDB table swap instructions provided"
}

# Verify data integrity after rollback
verify_data_integrity() {
    local stage="$1"
    local dry_run="$2"
    
    log "Verifying data integrity..."
    
    if [[ "$dry_run" == "true" ]]; then
        log "DRY RUN: Would verify data integrity"
        return 0
    fi
    
    # S3 integrity checks
    log "Checking S3 data integrity..."
    
    local s3_object_count=$(aws s3 ls "s3://$MEMORY_BUCKET" --recursive | wc -l)
    log "S3 objects found: $s3_object_count"
    
    # Check for .mv2 files
    local mv2_files=$(aws s3 ls "s3://$MEMORY_BUCKET" --recursive | grep "\.mv2$" | wc -l)
    log ".mv2 memory files found: $mv2_files"
    
    # DynamoDB integrity checks
    log "Checking DynamoDB data integrity..."
    
    for table in "$METADATA_TABLE" "$MEMORIES_TABLE" "$SUBSCRIPTIONS_TABLE" "$API_KEYS_TABLE" "$USAGE_TABLE" "$THRESHOLD_TABLE"; do
        if aws dynamodb describe-table --table-name "$table" &> /dev/null; then
            local item_count=$(aws dynamodb scan \
                --table-name "$table" \
                --select "COUNT" \
                --query "Count" \
                --output text)
            log "Table $table items: $item_count"
        else
            error "Table $table not accessible"
        fi
    done
    
    success "Data integrity verification completed"
}

# Main execution function
main() {
    local stage="${1:-}"
    local timestamp="${2:-}"
    local dry_run="false"
    local s3_only="false"
    local dynamodb_only="false"
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run)
                dry_run="true"
                shift
                ;;
            --s3-only)
                s3_only="true"
                shift
                ;;
            --dynamodb-only)
                dynamodb_only="true"
                shift
                ;;
            --help)
                show_help
                exit 0
                ;;
            *)
                if [[ -z "$stage" ]]; then
                    stage="$1"
                elif [[ -z "$timestamp" ]]; then
                    timestamp="$1"
                fi
                shift
                ;;
        esac
    done
    
    # Validate required parameters
    if [[ -z "$stage" ]] || [[ -z "$timestamp" ]]; then
        echo "Error: Missing required arguments"
        show_help
        exit 1
    fi
    
    log "Starting Mnemogram data rollback"
    log "Stage: $stage"
    log "Target timestamp: $timestamp"
    log "Dry run: $dry_run"
    log "S3 only: $s3_only"
    log "DynamoDB only: $dynamodb_only"
    log "Log file: $LOG_FILE"
    
    # Set AWS profile if not set
    export AWS_PROFILE="${AWS_PROFILE:-mnemogram-deploy}"
    
    # Confirmation for production
    if [[ "$stage" == "prod" && "$dry_run" == "false" ]]; then
        echo -e "${RED}WARNING: This will rollback PRODUCTION data!${NC}"
        read -p "Type 'CONFIRM' to proceed: " -r
        if [[ "$REPLY" != "CONFIRM" ]]; then
            error "Production data rollback cancelled"
        fi
    fi
    
    # Execute rollback steps
    check_prerequisites
    validate_timestamp "$timestamp"
    get_aws_context
    get_resource_names "$stage"
    
    # S3 rollback
    if [[ "$dynamodb_only" != "true" ]]; then
        backup_s3_current_state "$dry_run"
        
        # Try versioning rollback first, fall back to backup restore
        if ! rollback_s3_versioned "$stage" "$timestamp" "$dry_run"; then
            warn "Versioning rollback failed, trying backup restore"
            rollback_s3_from_backup "$stage" "$timestamp" "$dry_run"
        fi
    fi
    
    # DynamoDB rollback  
    if [[ "$s3_only" != "true" ]]; then
        rollback_dynamodb "$stage" "$timestamp" "$dry_run"
        swap_dynamodb_tables "$stage" "$dry_run"
    fi
    
    verify_data_integrity "$stage" "$dry_run"
    
    if [[ "$dry_run" == "false" ]]; then
        success "Data rollback completed successfully!"
        log "Rollback details saved to: $LOG_FILE"
        
        # Clean up temporary files
        rm -f /tmp/mnemogram-restored-tables /tmp/mnemogram-s3-backup-prefix
    else
        success "Dry run completed - no changes made"
    fi
}

# Error handler
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        error "Data rollback failed with exit code $exit_code"
        
        # Show recovery instructions
        if [[ -f /tmp/mnemogram-s3-backup-prefix ]]; then
            local backup_prefix=$(cat /tmp/mnemogram-s3-backup-prefix)
            warn "S3 backup available at: s3://$BACKUP_BUCKET/$backup_prefix/"
        fi
        
        if [[ -f /tmp/mnemogram-restored-tables ]]; then
            warn "Restored DynamoDB tables may need cleanup:"
            cat /tmp/mnemogram-restored-tables
        fi
    fi
}

trap cleanup EXIT

# Execute main function with all arguments
main "$@"