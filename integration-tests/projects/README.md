# Integration test projects

The run_XXX contain input applications, including their infrastructure as code (CDK) files to deploy, and application source code.

The CDK stacks must pass the scan with `scan_cdk_security.sh`. Since these are test projects, some checks can be sensibly skipped, we maintain a list `DEFAULT_SKIP_CHECKS` in the script.
