import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import { Construct } from "constructs";

export interface GitHubOidcStackProps extends cdk.StackProps {
  stage: string;
}

export class GitHubOidcStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: GitHubOidcStackProps) {
    super(scope, id, props);

    // GitHub OIDC Provider
    const githubOidcProvider = new iam.OpenIdConnectProvider(
      this,
      "GitHubOidcProvider",
      {
        url: "https://token.actions.githubusercontent.com",
        clientIds: ["sts.amazonaws.com"],
        thumbprints: [
          "6938fd4d98bab03faadb97b34396831e3780aea1",
          "1c58a3a8518e8759bf075b76b750d4f2df264fcd"
        ], // GitHub OIDC thumbprints
      }
    );

    // GitHub Deploy Role
    const githubDeployRole = new iam.Role(this, "GitHubDeployRole", {
      roleName: "mnemogram-github-deploy",
      assumedBy: new iam.WebIdentityPrincipal(
        githubOidcProvider.openIdConnectProviderArn,
        {
          StringEquals: {
            "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
          },
          StringLike: {
            "token.actions.githubusercontent.com:sub": [
              "repo:stbain/mnemogram:*",
              "repo:stbain/mnemogram-web:*"
            ],
          },
        }
      ),
      description: "Role for GitHub Actions to deploy Mnemogram infrastructure",
    });

    // Attach comprehensive deployment permissions
    githubDeployRole.addManagedPolicy(
      iam.ManagedPolicy.fromAwsManagedPolicyName("PowerUserAccess")
    );

    // Additional IAM permissions for CDK deployment
    githubDeployRole.addToPolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.ALLOW,
        actions: [
          "iam:CreateRole",
          "iam:DeleteRole",
          "iam:UpdateRole",
          "iam:GetRole",
          "iam:ListRolePolicies",
          "iam:AttachRolePolicy",
          "iam:DetachRolePolicy",
          "iam:PutRolePolicy",
          "iam:DeleteRolePolicy",
          "iam:GetRolePolicy",
          "iam:PassRole",
          "iam:TagRole",
          "iam:UntagRole",
          "sts:AssumeRole",
        ],
        resources: ["*"],
      })
    );

    // Output the role ARN for reference
    new cdk.CfnOutput(this, "GitHubDeployRoleArn", {
      value: githubDeployRole.roleArn,
      description: "ARN of the GitHub Actions deployment role",
    });
  }
}