# Mnemogram Rollback Architecture and Procedures

## Document Information
- **Ticket:** MNEM-240
- **Author:** Cody (Development Agent)
- **Date:** 2026-02-17
- **Version:** 1.0

## Executive Summary

This document defines a comprehensive rollback strategy for Mnemogram covering infrastructure, data, and application layers. The strategy prioritizes service availability while ensuring data integrity and minimizing customer impact.

## Rollback Decision Criteria

### Automatic Rollback Triggers
- **Critical Service Failures:** API Gateway 5xx error rate > 50% for 10+ minutes
- **Database Failures:** DynamoDB throttling > 80% for 5+ minutes
- **Lambda Failures:** Any core function (search, recall, ingest) error rate > 25% for 5+ minutes
- **S3 Access Failures:** Memory bucket access errors > 10% for 5+ minutes
- **Performance Degradation:** API response times > 30s for 15+ minutes

### Manual Rollback Criteria
- **Data Corruption:** Evidence of corrupted .mv2 files or metadata inconsistencies  
- **Security Incidents:** Suspected compromise requiring immediate reversion
- **Critical Bug Discovery:** Functionality-breaking bugs affecting core features
- **Cost Overruns:** Unexpected AWS billing spikes indicating runaway processes
- **Compliance Issues:** Regulatory or security policy violations

## Infrastructure Rollback Procedure

### CDK Stack Rollback
```bash
# 1. Identify last known good deployment
cdk diff --stage dev/staging/prod

# 2. Revert to previous CDK stack version
git checkout <previous-commit-hash>
cdk deploy MnemogramStack-{stage} --require-approval never

# 3. Monitor deployment progress
aws cloudformation describe-stack-events --stack-name MnemogramStack-{stage}
```

### Lambda Function Rollback
```bash
# 1. List function versions
aws lambda list-versions-by-function --function-name mnemogram-{stage}-{function}

# 2. Update function to previous version
aws lambda update-alias --function-name mnemogram-{stage}-{function} \
  --name LIVE --function-version {previous-version}

# 3. Verify rollback
aws lambda get-alias --function-name mnemogram-{stage}-{function} --name LIVE
```

### API Gateway Rollback
- **Stage Rollback:** Promote previous deployment to current stage
- **Routing Rollback:** Update weighted routing to redirect traffic
- **DNS Rollback:** CloudFlare/Route53 changes if custom domain affected

## Data Rollback Strategy

### S3 Memory Files (.mv2)
```bash
# 1. Enable S3 versioning (already configured in CDK)
# 2. Restore specific versions
aws s3api list-object-versions --bucket mnemogram-{stage}-memories-{account}-{region}

# 3. Bulk restore from backup timestamp
aws s3 sync s3://mnemogram-{stage}-backup/{timestamp}/ s3://mnemogram-{stage}-memories-{account}-{region}/
```

### DynamoDB Data Rollback
```bash
# 1. Point-in-time recovery (already enabled in CDK)
aws dynamodb restore-table-to-point-in-time \
  --source-table-name mnemogram-{stage}-{table} \
  --target-table-name mnemogram-{stage}-{table}-restored \
  --restore-date-time {timestamp}

# 2. Swap table names after validation
# 3. Update CDK environment variables to point to restored table
```

### Backup-Based Recovery
```bash
# 1. List available backups
aws backup list-recovery-points --backup-vault-name mnemogram-{stage}-backup-vault

# 2. Restore from backup
aws backup start-restore-job \
  --recovery-point-arn {backup-arn} \
  --metadata {restore-metadata}
```

## Application Deployment Rollback

### GitHub Actions Rollback
1. **Revert Commit:** Create revert commit for problematic deployment
2. **Manual Trigger:** Trigger previous successful workflow run
3. **Branch Protection:** Merge revert to main/dev branch
4. **Monitor Deployment:** Watch GitHub Actions deployment status

### Container/Lambda Code Rollback
```bash
# 1. Identify last known good build
git log --oneline --grep="feat\|fix" | head -10

# 2. Create rollback branch
git checkout -b rollback/{ticket-number} {good-commit-hash}

# 3. Force push to trigger deployment
git push -f origin rollback/{ticket-number}:dev
```

### Configuration Rollback
- **Environment Variables:** Update Lambda function configs
- **Feature Flags:** Disable problematic features via environment variables
- **Route Weights:** Adjust API Gateway weighted routing

## Service Continuity Planning

### Zero-Downtime Rollback Process
1. **Traffic Diversion:** Route traffic to healthy instances/regions
2. **Parallel Deployment:** Deploy rollback version alongside current
3. **Gradual Cutover:** Shift traffic percentage-wise (10% → 50% → 100%)
4. **Health Monitoring:** Continuous monitoring during transition
5. **Fallback Ready:** Keep previous version available for immediate fallback

### Multi-Region Considerations
- **Primary Region:** us-east-1 (production workloads)
- **Secondary Regions:** us-west-2, eu-west-1 (disaster recovery)
- **Cross-Region Replication:** S3 Cross-Region Replication for critical data
- **Route 53 Health Checks:** Automatic DNS failover

### Monitoring During Rollback
```yaml
Critical Metrics:
  - API Gateway latency and error rates
  - Lambda function success rates and duration
  - DynamoDB read/write capacity utilization
  - S3 bucket access patterns and error rates
  - Application-specific metrics (search accuracy, memory upload success)

Alerting Thresholds:
  - API 5xx errors > 5% for 2 minutes
  - Lambda errors > 10% for 1 minute  
  - DynamoDB throttling > 20% for 1 minute
  - S3 access errors > 5% for 2 minutes
```

## Communication Plan for Rollback

### Internal Communication
1. **Incident Commander:** Stuart (Product Owner) or designated on-call
2. **Development Team:** Cody (Dev Agent), Larry (Business Agent)
3. **Status Page:** Update status.mnemogram.com with incident details
4. **Slack/Discord:** Real-time updates in #incidents channel

### External Communication
1. **Customer Notification:** Email to affected users within 30 minutes
2. **Status Page:** Public incident updates every 15 minutes
3. **Social Media:** Twitter/LinkedIn updates for major incidents
4. **Documentation:** Post-incident report within 48 hours

### Escalation Matrix
```
Level 1: Minor issues → Cody (Automated fixes)
Level 2: Service degradation → Stuart notification
Level 3: Service outage → Stuart + customer communication
Level 4: Data integrity issues → Full team + leadership escalation
```

## Rollback Testing Procedures

### Staging Environment Testing
- **Monthly Rollback Drills:** Practice full rollback procedures
- **Data Integrity Verification:** Validate data consistency post-rollback
- **Performance Validation:** Ensure service performance meets SLA
- **Recovery Time Measurement:** Document and optimize RTO/RPO

### Production Readiness Checklist
- [ ] Infrastructure rollback scripts tested in staging
- [ ] Data backup and restore procedures validated
- [ ] Monitoring and alerting configured for rollback scenarios
- [ ] Communication templates prepared
- [ ] Team roles and responsibilities documented
- [ ] External dependencies identified and contingencies planned

## Recovery Time & Point Objectives

### Targets
- **RTO (Recovery Time Objective):** < 30 minutes for infrastructure
- **RPO (Recovery Point Objective):** < 5 minutes for data loss
- **MTTR (Mean Time To Recovery):** < 15 minutes for automated scenarios

### Implementation
- **Automated Monitoring:** CloudWatch alarms trigger automated responses
- **Runbook Automation:** Scripts ready for one-click execution
- **Pre-deployed Resources:** Warm standby resources in secondary regions

## Risk Assessment

### High Risk Scenarios
- **Data Corruption:** S3 versioning + DynamoDB point-in-time recovery mitigate
- **Complete Region Failure:** Multi-region deployment with automatic failover
- **GitHub Actions Failure:** Local CDK deployment capabilities as backup

### Mitigation Strategies
- **Blue-Green Deployments:** Zero-downtime deployments with instant rollback
- **Canary Releases:** Gradual rollout with automated monitoring
- **Circuit Breakers:** Automatic service protection during failures

## Tools and Scripts

### Required Tools
- AWS CLI (latest version)
- CDK CLI (typescript)
- jq (JSON processing)
- Git (version control)
- GitHub CLI (workflow management)

### Automation Scripts
```bash
# scripts/rollback-infrastructure.sh - CDK stack rollback
# scripts/rollback-data.sh - Database and S3 rollback  
# scripts/health-check.sh - Post-rollback validation
# scripts/notify-stakeholders.sh - Communication automation
```

## Documentation Maintenance

### Review Schedule
- **Monthly:** Review rollback procedures and update based on changes
- **Quarterly:** Full rollback drill and procedure validation
- **Post-Incident:** Update procedures based on lessons learned

### Version Control
- **Git Repository:** Store all rollback scripts and documentation
- **Change Management:** All procedure changes require peer review
- **Backup Copies:** Offline copies of critical rollback procedures

## Conclusion

This rollback architecture provides comprehensive coverage of infrastructure, data, and application rollback scenarios. The strategy emphasizes automation, monitoring, and clear communication to minimize service impact and ensure rapid recovery from deployment issues.

**Next Steps:**
1. Implement automated rollback scripts (MNEM-241)
2. Test rollback procedures in staging (MNEM-242)
3. Train team on rollback execution
4. Establish monitoring dashboards for rollback scenarios

---
*Document maintained by Cody Development Agent*
*Last Updated: 2026-02-17*