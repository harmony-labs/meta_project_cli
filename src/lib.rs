use meta_plugin_api::{Plugin, PluginError};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ProjectPlugin;

fn parse_meta_projects(meta_path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let config_str = fs::read_to_string(meta_path)?;
    let meta_config: serde_json::Value = serde_json::from_str(&config_str)?;
    let projects = meta_config["projects"].as_object().ok_or_else(|| anyhow::anyhow!("No 'projects' key in .meta"))?;
    let mut map = HashMap::new();
    for (name, url) in projects.iter() {
        if let Some(url_str) = url.as_str() {
            map.insert(name.clone(), url_str.to_string());
        }
    }
    Ok(map)
}

fn find_missing_projects(projects: &HashMap<String, String>, base_dir: &Path) -> Vec<(String, String)> {
    projects.iter()
        .filter(|(name, _)| !base_dir.join(name).is_dir())
        .map(|(name, url)| (name.clone(), url.clone()))
        .collect()
}

fn print_missing(missing: &[(String, String)]) {
    if !missing.is_empty() {
        for (name, url) in missing {
            meta_git_lib::print_missing_repo(name, url, &std::env::current_dir().unwrap().join(name));
        }
        println!();
    }
}



impl Plugin for ProjectPlugin {
    fn name(&self) -> &'static str {
        "project"
    }

    fn commands(&self) -> Vec<&'static str> {
        vec!["project check", "project sync", "project update"]
    }

    fn execute(&self, command: &str, _args: &[String]) -> anyhow::Result<()> {
        let cwd = std::env::current_dir()?;
        let meta_path = cwd.join(".meta");
        if !meta_path.exists() {
            return Err(anyhow::anyhow!("No .meta file found in {}", cwd.display()));
        }
        let projects = parse_meta_projects(&meta_path)?;
        let missing = find_missing_projects(&projects, &cwd);

        // Always check and warn, unless we're already running 'project check'
        if command != "project check" {
            print_missing(&missing);
        }

        match command {
            "project check" => {
                if missing.is_empty() {
                    println!("All projects are cloned and present.");
                } else {
                    print_missing(&missing);
                }
                Ok(())
            }
            "project sync" | "project update" => {
                if missing.is_empty() {
                    println!("All projects are cloned and present. Nothing to do.");
                    return Ok(());
                }
                for (name, url) in &missing {
    let target_dir = cwd.join(name);
    if let Err(e) = meta_git_lib::clone_repo_with_progress(url, &target_dir, None) {
        println!("Error cloning {}: {}", name, e);
    }
}
                Ok(())
            }
            _ => Err(PluginError::CommandNotFound(command.to_string()).into()),
        }
    }
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(ProjectPlugin))
}