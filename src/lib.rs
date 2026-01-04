//! meta-project library
//!
//! Provides project management commands for meta repositories.

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ============================================================================
// Execution Plan Types (for subprocess plugin protocol)
// ============================================================================

/// An execution plan that tells the shim what commands to run via loop_lib
#[derive(Debug, Serialize)]
pub struct ExecutionPlan {
    pub commands: Vec<PlannedCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,
}

/// A single command in an execution plan
#[derive(Debug, Serialize)]
pub struct PlannedCommand {
    pub dir: String,
    pub cmd: String,
}

/// Response wrapper for execution plans
#[derive(Debug, Serialize)]
pub struct PlanResponse {
    pub plan: ExecutionPlan,
}

/// Output an execution plan to stdout for the shim to execute
pub fn output_execution_plan(commands: Vec<PlannedCommand>, parallel: Option<bool>) {
    let response = PlanResponse {
        plan: ExecutionPlan { commands, parallel },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
}

// ============================================================================
// Command Result Types
// ============================================================================

/// Result of executing a project command
pub enum CommandResult {
    /// A plan of commands to execute via loop_lib
    Plan(Vec<PlannedCommand>, Option<bool>),
    /// A message to display (no commands to execute)
    Message(String),
    /// An error occurred
    Error(String),
}

/// Execute a project command and return the result
///
/// If `provided_projects` is not empty, it will be used instead of reading from .meta file.
/// This allows meta_cli to pass in the full project list when --recursive is used.
pub fn execute_command(
    command: &str,
    _args: &[String],
    dry_run: bool,
    provided_projects: &[String],
) -> CommandResult {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return CommandResult::Error(format!("Failed to get current directory: {e}")),
    };

    // If we have provided projects from meta_cli (e.g., when --recursive is used),
    // we need to check each project directory for missing repos in their .meta files
    if !provided_projects.is_empty() {
        return execute_command_recursive(command, dry_run, provided_projects, &cwd);
    }

    // Fall back to reading the local .meta file
    let meta_path = cwd.join(".meta");
    if !meta_path.exists() {
        return CommandResult::Error(format!("No .meta file found in {}", cwd.display()));
    }
    let projects = match parse_meta_projects(&meta_path) {
        Ok(projects) => projects,
        Err(e) => return CommandResult::Error(format!("Failed to parse .meta: {e}")),
    };
    let missing = find_missing_projects(&projects, &cwd);

    match command {
        "project check" => {
            if missing.is_empty() {
                CommandResult::Message("All projects are cloned and present.".to_string())
            } else {
                // Print missing repos (uses visual formatting)
                print_missing(&missing);
                CommandResult::Message(format!("{} project(s) missing", missing.len()))
            }
        }
        "project sync" | "project update" => {
            if missing.is_empty() {
                return CommandResult::Message(
                    "All projects are cloned and present. Nothing to do.".to_string(),
                );
            }

            // Build clone commands for each missing project
            let commands: Vec<PlannedCommand> = missing
                .iter()
                .map(|(name, url)| {
                    let target_dir = cwd.join(name);
                    PlannedCommand {
                        dir: ".".to_string(), // Clone runs in cwd
                        cmd: format!("git clone {} {}", url, target_dir.display()),
                    }
                })
                .collect();

            if dry_run {
                // In dry_run mode, output will be shown by loop_lib
            }

            CommandResult::Plan(commands, Some(false)) // Sequential cloning
        }
        _ => CommandResult::Error(format!("Unknown command: {}", command)),
    }
}

/// Execute a project command recursively across provided project directories
///
/// This handles the case when --recursive is used. Each project directory may have
/// its own .meta file with additional projects to check/sync.
fn execute_command_recursive(
    command: &str,
    _dry_run: bool,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    let mut all_missing: Vec<(String, String)> = Vec::new();

    // Check the root .meta file first
    let root_meta_path = cwd.join(".meta");
    if root_meta_path.exists() {
        if let Ok(projects) = parse_meta_projects(&root_meta_path) {
            let missing = find_missing_projects(&projects, cwd);
            for (name, url) in missing {
                all_missing.push((name, url));
            }
        }
    }

    // Check each provided project directory for its own .meta file
    for project_path in provided_projects {
        let project_dir = cwd.join(project_path);
        let nested_meta_path = project_dir.join(".meta");
        if nested_meta_path.exists() {
            if let Ok(projects) = parse_meta_projects(&nested_meta_path) {
                let missing = find_missing_projects(&projects, &project_dir);
                for (name, url) in missing {
                    // Use the full path relative to cwd
                    let full_path = format!("{}/{}", project_path, name);
                    all_missing.push((full_path, url));
                }
            }
        }
    }

    match command {
        "project check" => {
            if all_missing.is_empty() {
                CommandResult::Message("All projects are cloned and present.".to_string())
            } else {
                // Print all missing repos
                for (name, url) in &all_missing {
                    meta_git_lib::print_missing_repo(name, url, &cwd.join(name));
                }
                println!();
                CommandResult::Message(format!("{} project(s) missing", all_missing.len()))
            }
        }
        "project sync" | "project update" => {
            if all_missing.is_empty() {
                return CommandResult::Message(
                    "All projects are cloned and present. Nothing to do.".to_string(),
                );
            }

            // Build clone commands for each missing project
            let commands: Vec<PlannedCommand> = all_missing
                .iter()
                .map(|(name, url)| {
                    let target_dir = cwd.join(name);
                    PlannedCommand {
                        dir: ".".to_string(), // Clone runs in cwd
                        cmd: format!("git clone {} {}", url, target_dir.display()),
                    }
                })
                .collect();

            CommandResult::Plan(commands, Some(false)) // Sequential cloning
        }
        _ => CommandResult::Error(format!("Unknown command: {}", command)),
    }
}

/// Get help text for the plugin
pub fn get_help_text() -> &'static str {
    r#"meta project - Project Management Plugin

Commands:
  meta project check   Check if all projects in .meta are cloned locally
  meta project sync    Clone any missing projects from .meta
  meta project update  Alias for 'project sync'

This plugin helps manage multi-repository workspaces defined in .meta files.
"#
}

fn parse_meta_projects(meta_path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let config_str = fs::read_to_string(meta_path)?;
    let meta_config: serde_json::Value = serde_json::from_str(&config_str)?;
    let projects = meta_config["projects"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("No 'projects' key in .meta"))?;
    let mut map = HashMap::new();
    for (name, url) in projects.iter() {
        if let Some(url_str) = url.as_str() {
            map.insert(name.clone(), url_str.to_string());
        }
    }
    Ok(map)
}

fn find_missing_projects(
    projects: &HashMap<String, String>,
    base_dir: &Path,
) -> Vec<(String, String)> {
    projects
        .iter()
        .filter(|(name, _)| !base_dir.join(name).is_dir())
        .map(|(name, url)| (name.clone(), url.clone()))
        .collect()
}

fn print_missing(missing: &[(String, String)]) {
    if !missing.is_empty() {
        for (name, url) in missing {
            meta_git_lib::print_missing_repo(
                name,
                url,
                &std::env::current_dir().unwrap().join(name),
            );
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_execute_command_no_meta_file() {
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();

        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = execute_command("project check", &[], false, &[]);

        std::env::set_current_dir(original_dir).unwrap();

        match result {
            CommandResult::Error(msg) => assert!(msg.contains("No .meta file")),
            _ => panic!("Expected Error result"),
        }
    }

    #[test]
    fn test_unknown_command() {
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();

        // Create a .meta file
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = execute_command("project unknown", &[], false, &[]);

        std::env::set_current_dir(original_dir).unwrap();

        match result {
            CommandResult::Error(msg) => assert!(msg.contains("Unknown command")),
            _ => panic!("Expected Error result"),
        }
    }

    #[test]
    fn test_project_check_all_present() {
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();

        // Create a .meta file with no projects
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = execute_command("project check", &[], false, &[]);

        std::env::set_current_dir(original_dir).unwrap();

        match result {
            CommandResult::Message(msg) => assert!(msg.contains("All projects are cloned")),
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_project_sync_returns_plan_for_missing() {
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();

        // Create a .meta file with a missing project
        std::fs::write(
            temp_dir.path().join(".meta"),
            r#"{"projects": {"missing-repo": "https://github.com/test/repo.git"}}"#,
        )
        .unwrap();

        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = execute_command("project sync", &[], false, &[]);

        std::env::set_current_dir(original_dir).unwrap();

        match result {
            CommandResult::Plan(commands, parallel) => {
                assert_eq!(commands.len(), 1);
                assert!(commands[0].cmd.contains("git clone"));
                assert!(commands[0].cmd.contains("https://github.com/test/repo.git"));
                assert_eq!(parallel, Some(false));
            }
            _ => panic!("Expected Plan result"),
        }
    }

    #[test]
    fn test_project_sync_nothing_to_do() {
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();

        // Create a .meta file with no projects
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = execute_command("project sync", &[], false, &[]);

        std::env::set_current_dir(original_dir).unwrap();

        match result {
            CommandResult::Message(msg) => assert!(msg.contains("Nothing to do")),
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_get_help_text() {
        let help = get_help_text();
        assert!(help.contains("project check"));
        assert!(help.contains("project sync"));
    }

    #[test]
    fn test_execution_plan_serialization() {
        let commands = vec![
            PlannedCommand {
                dir: ".".to_string(),
                cmd: "git clone https://example.com/repo.git repo".to_string(),
            },
        ];
        let plan = ExecutionPlan {
            commands,
            parallel: Some(false),
        };
        let response = PlanResponse { plan };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"plan\""));
        assert!(json.contains("\"commands\""));
        assert!(json.contains("git clone"));
    }
}
