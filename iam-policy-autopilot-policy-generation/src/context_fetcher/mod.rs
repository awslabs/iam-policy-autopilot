use std::{path::PathBuf, str::FromStr};

use serde::Serialize;
use serde_json;

use crate::{context_fetcher::terraform_state::{TerraformShowReader, TerraformStateContext}, errors::ExtractorError};

/// wraps around resourceexplorer and sts
pub mod service;

/// HUH
pub mod terraform_state;

/// ARNs
#[derive(Serialize)]
pub struct Arn {
    /// ARN
    pub arn: String,
    #[serde(skip_serializing)]
    service: String,
    #[serde(skip_serializing)]
    resource_type: String
}

impl Arn {
    /// new arn
    pub fn new(arn: String) -> Self {
        Arn {
            arn: arn.clone(),
            service: Self::parse_service_part(&arn),
            resource_type: Self::parse_resource_part(&arn)
        }
    }

    fn parse_service_part(arn: &String) -> String {
        if arn.eq("*") {
            return "*".to_string();
        }
        arn.split(':').collect::<Vec<_>>().get(2).unwrap().to_string()
    }
    fn parse_resource_part(arn: &String) -> String {
        if arn.eq("*") {
            return "*".to_string();
        }
        let resource_final = arn.split(':').collect::<Vec<_>>().get(5).unwrap().to_string();

        resource_final.split('/').collect::<Vec<_>>().get(0).unwrap().to_string()
    }
}

pub(crate) struct TerraformProjectExplorer {
       pub(crate) terraform_state_context: TerraformStateContext,
       //TODO: add terraform plan extractor
}

impl TerraformProjectExplorer {
    pub(crate) fn new(terraform_dir: &PathBuf) -> Result<Self, ExtractorError> {

        let terraform_show_reader = TerraformShowReader::retrieve_terraform_state(terraform_dir)?;

        Ok(TerraformProjectExplorer { terraform_state_context:  TerraformStateContext::read_from_terraform_reader(terraform_show_reader)? })
    }
}