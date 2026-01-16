use std::{collections::HashMap, path::PathBuf};
use serde_json::Value as JsonValue;
use log::info;
use std::process::Command;
use crate::{context_fetcher::Arn, errors::ExtractorError};
use serde_json::from_str;

/// Terraform state extraction result
pub struct TerraformStateContext {
    /// map from service:resource to arns
    pub resource_arns: HashMap<String, Vec<Arn>>,
    // region: Option<String>
}

impl TerraformStateContext {
    /// constructor
    pub fn new(resource_arns: HashMap<String, Vec<Arn>>) -> Self {
        TerraformStateContext {
            resource_arns: resource_arns
        }
    }

    /// read it
    pub(crate) fn read_from_terraform_reader(terraform_show: TerraformShowReader) -> Result<TerraformStateContext, ExtractorError> {

        let map = terraform_show.terraform_output.as_object().ok_or( ExtractorError::terraform_state_parse("Terraform show object is not a map".to_string(), JsonValue::to_string(&terraform_show.terraform_output)))?;

        let values_map = map.get("values").ok_or(ExtractorError::terraform_state_parse("Terraform show object does not have values field".to_string(), JsonValue::to_string(&terraform_show.terraform_output)))?;


        let root_module_map = values_map.get("root_module").ok_or(ExtractorError::terraform_state_parse("Terraform show object does not have values.root_module field".to_string(), JsonValue::to_string(&terraform_show.terraform_output)))?;

        let resources = root_module_map.get("resources").ok_or(ExtractorError::terraform_state_parse("Terraform show object does not have values.root_module.resources field".to_string(), JsonValue::to_string(&terraform_show.terraform_output)))?;

        let resource_arr = resources.as_array().ok_or(ExtractorError::terraform_state_parse("Terraform resources object is not an array".to_string(), JsonValue::to_string(&terraform_show.terraform_output)))?;

        let mut resource_arn_map = HashMap::<String, Vec<Arn>>::new();

        for resource in resource_arr {
            let Some(value) = resource.get("values") else {
                continue;
            };

            let Some(value_map) = value.as_object() else {
                continue;
            };

            let Some(arn_val) = value_map.get("arn") else {
                continue;
            };

            let Some(arn_str) = arn_val.as_str() else {
                continue;
            };

            let arn = Arn::new(arn_str.to_string());

            let map_key = format!("{}:{}", arn.service, arn.resource_type);

            if !resource_arn_map.contains_key(&map_key){
                resource_arn_map.insert(map_key.clone(), Vec::new());
            } 

            resource_arn_map.get_mut(&map_key).unwrap().push(arn);
        }

        Ok(TerraformStateContext::new(resource_arn_map))


    }
}

pub(crate) struct TerraformShowReader{
    terraform_output: JsonValue
}

impl TerraformShowReader {

    pub(crate) fn retrieve_terraform_state(terraform_dir: &PathBuf) -> Result<TerraformShowReader, ExtractorError> {
        info!("Retrieving terraform state from {:?}", terraform_dir);

        // TODO: format can vary by platform, e.g. windows. see https://doc.rust-lang.org/std/process/struct.Command.html
        let mut cmd = Command::new("terraform");
        cmd.arg("show").arg("-json").current_dir(&terraform_dir);
        
        let cmd_str = format!("{:?} {:?}", cmd, cmd.get_args());
        let output = cmd.output().expect("Failed to run terraform show.");

        info!("Terraform show output: {:?}", String::from_utf8_lossy(&output.stdout));

        if !output.status.success() {
            Err(ExtractorError::terraform_state_command(cmd_str, String::from_utf8_lossy(&output.stderr).to_string()))
        } else {
            let json = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(TerraformShowReader { terraform_output:  from_str(&json)? })
        }
        
    }
}
