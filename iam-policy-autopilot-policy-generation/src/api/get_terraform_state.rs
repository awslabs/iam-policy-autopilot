use std::path::PathBuf;
use anyhow::{Context, Result};

use crate::context_fetcher::{TerraformProjectExplorer, terraform_state::TerraformStateContext};

/// get the terraform state.
pub async fn get_terraform_state(terraform_dir: PathBuf) -> Result<(TerraformStateContext)> {
    
    let terraform_context = TerraformProjectExplorer::new(&terraform_dir)?;

    Ok(terraform_context.terraform_state_context)
}