//! meta-project library
//!
//! Provides project management commands for meta repositories.

use meta_cli::config::{self, MetaTreeNode};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

pub use meta_plugin_protocol::{
    output_execution_plan, CommandResult, ExecutionPlan, PlanResponse, PlannedCommand,
};

/// Options passed to execute_command
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecuteOptions {
    pub dry_run: bool,
    pub json_output: bool,
    pub recursive: bool,
    pub depth: Option<usize>,
    pub verbose: bool,
    pub parallel: bool,
}

// ============================================================================
// Project List Types
// ============================================================================

/// A project node in the hierarchical tree
#[derive(Debug, Clone, Serialize)]
pub struct ProjectTreeNode {
    pub name: String,
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub is_meta: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectTreeNode>,
}

/// Top-level output for `meta project list --json`
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListOutput {
    pub path: String,
    pub repo: String,
    pub projects: Vec<ProjectTreeNode>,
}

// ============================================================================
// Command Execution
// ============================================================================

/// Execute a project command and return the result
///
/// If `provided_projects` is not empty, it will be used instead of reading from .meta file.
/// This allows meta_cli to pass in the full project list when --recursive is used.
pub fn execute_command(
    command: &str,
    args: &[String],
    options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    // Intercept --help/-h before dispatching to subcommand handlers
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return CommandResult::ShowHelp(None);
    }

    // project list/ls handles its own config discovery
    if command == "project list" || command == "project ls" {
        // Check if --json was passed as a trailing arg (not extracted by meta_cli
        // because it could be intended for a subcommand in non-plugin contexts)
        let json_from_args = args.iter().any(|a| a == "--json");
        let effective_options = if json_from_args && !options.json_output {
            ExecuteOptions {
                json_output: true,
                ..*options
            }
        } else {
            ExecuteOptions { ..*options }
        };
        return handle_project_list(cwd, &effective_options);
    }

    // If we have provided projects from meta_cli (e.g., when --recursive is used),
    // we need to check each project directory for missing repos in their .meta files
    if !provided_projects.is_empty() {
        return execute_command_recursive(command, options, provided_projects, cwd);
    }

    // Fall back to reading the local meta config
    let Some((meta_path, _format)) = config::find_meta_config_in(cwd) else {
        return CommandResult::Error(format!("No .meta config found in {}", cwd.display()));
    };
    let projects = match parse_meta_projects(&meta_path) {
        Ok(projects) => projects,
        Err(e) => return CommandResult::Error(format!("Failed to parse meta config: {e}")),
    };
    let missing = find_missing_projects(&projects, cwd);

    match command {
        "project check" => {
            if missing.is_empty() {
                CommandResult::Message("All projects are cloned and present.".to_string())
            } else {
                // Print missing repos (uses visual formatting)
                print_missing(&missing, cwd);
                CommandResult::Message(format!(
                    "{} project(s) missing. Run 'meta git update' to clone them.",
                    missing.len()
                ))
            }
        }
        _ => CommandResult::ShowHelp(Some(format!(
            "unrecognized command '{command}'. Use 'meta git update' to sync projects."
        ))),
    }
}

/// Execute a project command recursively across provided project directories
///
/// This handles the case when --recursive is used. Each project directory may have
/// its own .meta file with additional projects to check/sync.
fn execute_command_recursive(
    command: &str,
    _options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    let mut all_missing: Vec<(String, String)> = Vec::new();

    // Check the root meta config first
    if let Some((root_meta_path, _format)) = config::find_meta_config_in(cwd) {
        if let Ok(projects) = parse_meta_projects(&root_meta_path) {
            let missing = find_missing_projects(&projects, cwd);
            for (name, url) in missing {
                all_missing.push((name, url));
            }
        }
    }

    // Check each provided project directory for its own meta config
    for project_path in provided_projects {
        let project_dir = cwd.join(project_path);
        if let Some((nested_meta_path, _format)) = config::find_meta_config_in(&project_dir) {
            if let Ok(projects) = parse_meta_projects(&nested_meta_path) {
                let missing = find_missing_projects(&projects, &project_dir);
                for (name, url) in missing {
                    // Use the full path relative to cwd
                    let full_path = format!("{project_path}/{name}");
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
                CommandResult::Message(format!(
                    "{} project(s) missing. Run 'meta git update' to clone them.",
                    all_missing.len()
                ))
            }
        }
        _ => CommandResult::ShowHelp(Some(format!(
            "unrecognized command '{command}'. Use 'meta git update' to sync projects."
        ))),
    }
}

// ============================================================================
// Project List Implementation
// ============================================================================

/// Handle `meta project list` / `meta project ls`
fn handle_project_list(cwd: &Path, options: &ExecuteOptions) -> CommandResult {
    let max_depth = if options.recursive {
        options.depth
    } else {
        Some(0)
    };

    let tree = match config::walk_meta_tree(cwd, max_depth) {
        Ok(t) => t,
        Err(e) => return CommandResult::Error(format!("{e}")),
    };

    let root_repo = get_git_remote_url(cwd).unwrap_or_default();
    let project_nodes: Vec<ProjectTreeNode> = tree.iter().map(to_project_tree_node).collect();

    if options.json_output {
        let output = ProjectListOutput {
            path: ".".to_string(),
            repo: root_repo,
            projects: project_nodes,
        };
        let json = match serde_json::to_string_pretty(&output) {
            Ok(j) => j,
            Err(e) => return CommandResult::Error(format!("Failed to serialize JSON: {e}")),
        };
        CommandResult::Message(json)
    } else {
        let mut output = String::new();
        output.push_str(&format!(". ({root_repo})\n"));
        format_project_tree(&project_nodes, &mut output, "");
        if output.ends_with('\n') {
            output.pop();
        }
        CommandResult::Message(output)
    }
}

fn to_project_tree_node(node: &MetaTreeNode) -> ProjectTreeNode {
    ProjectTreeNode {
        name: node.info.name.clone(),
        path: node.info.path.clone(),
        repo: node.info.repo.clone(),
        tags: node.info.tags.clone(),
        is_meta: node.is_meta,
        projects: node.children.iter().map(to_project_tree_node).collect(),
    }
}

/// Get the git remote origin URL for a directory
fn get_git_remote_url(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(dir)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Format a project tree with box-drawing characters
fn format_project_tree(nodes: &[ProjectTreeNode], output: &mut String, prefix: &str) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        let connector = if is_last {
            "\u{2514}\u{2500}\u{2500} "
        } else {
            "\u{251c}\u{2500}\u{2500} "
        };

        let tags_str = if node.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", node.tags.join(", "))
        };

        output.push_str(&format!(
            "{}{}{} ({}){}\n",
            prefix, connector, node.name, node.path, tags_str
        ));

        if !node.projects.is_empty() {
            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}\u{2502}   ")
            };
            format_project_tree(&node.projects, output, &child_prefix);
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get help text for the plugin
pub fn get_help_text() -> &'static str {
    r#"meta project - Project Inspection Plugin

Commands:
  meta project list    List all projects defined in .meta (alias: ls)
  meta project check   Check if all projects in .meta are cloned locally

Options for list:
  --json               Output as JSON
  --recursive, -r      Include nested meta repo children
  --depth N            Maximum recursion depth (default: unlimited)

To clone missing projects, use: meta git update
"#
}

fn parse_meta_projects(meta_path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let (projects, _ignore) = config::parse_meta_config(meta_path)?;
    let mut map = HashMap::new();
    for p in projects {
        // Skip projects without a repo URL (cannot clone)
        if let Some(repo) = p.repo {
            map.insert(p.path.clone(), repo);
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

fn print_missing(missing: &[(String, String)], cwd: &Path) {
    if !missing.is_empty() {
        for (name, url) in missing {
            meta_git_lib::print_missing_repo(name, url, &cwd.join(name));
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

        let options = ExecuteOptions::default();
        let result = execute_command("project check", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::Error(msg) => assert!(msg.contains("No .meta config")),
            _ => panic!("Expected Error result"),
        }
    }

    #[test]
    fn test_unknown_command() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta file
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project unknown", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::ShowHelp(Some(msg)) => assert!(msg.contains("unrecognized command")),
            _ => panic!("Expected ShowHelp result"),
        }
    }

    #[test]
    fn test_project_check_all_present() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta file with no projects
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project check", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => assert!(msg.contains("All projects are cloned")),
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_project_sync_removed() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta file
        std::fs::write(
            temp_dir.path().join(".meta"),
            r#"{"projects": {"missing-repo": "https://github.com/test/repo.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project sync", &[], &options, &[], temp_dir.path());

        // sync command should now return an error directing users to meta git update
        match result {
            CommandResult::ShowHelp(Some(msg)) => {
                assert!(msg.contains("meta git update"));
            }
            _ => panic!("Expected ShowHelp result directing to meta git update"),
        }
    }

    #[test]
    fn test_get_help_text() {
        let help = get_help_text();
        assert!(help.contains("project check"));
        assert!(help.contains("project list"));
        assert!(help.contains("meta git update")); // Points to the right command
        assert!(!help.contains("project sync")); // sync is removed
    }

    #[test]
    fn test_execution_plan_serialization() {
        let commands = vec![PlannedCommand {
            dir: ".".to_string(),
            cmd: "git clone https://example.com/repo.git repo".to_string(),
            env: None,
        }];
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands,
            post_commands: vec![],
            parallel: Some(false),
            max_parallel: None,
        };
        let response = PlanResponse { plan };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"plan\""));
        assert!(json.contains("\"commands\""));
        assert!(json.contains("git clone"));
    }

    #[test]
    fn test_project_list_basic() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta file with projects
        std::fs::write(
            temp_dir.path().join(".meta"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git", "repo2": "git@github.com:org/repo2.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => {
                assert!(msg.contains("repo1"));
                assert!(msg.contains("repo2"));
                assert!(msg.contains(".")); // root indicator
            }
            _ => panic!("Expected Message result, got error"),
        }
    }

    #[test]
    fn test_project_list_json() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join(".meta"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => {
                let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
                assert_eq!(parsed["path"], ".");
                assert!(parsed["projects"].is_array());
                let projects = parsed["projects"].as_array().unwrap();
                assert_eq!(projects.len(), 1);
                assert_eq!(projects[0]["name"], "repo1");
                assert_eq!(projects[0]["repo"], "git@github.com:org/repo1.git");
            }
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_project_ls_alias() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join(".meta"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project ls", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => {
                assert!(msg.contains("repo1"));
            }
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_project_list_recursive() {
        let temp_dir = TempDir::new().unwrap();

        // Create root .meta
        std::fs::write(
            temp_dir.path().join(".meta"),
            r#"{"projects": {"child": "git@github.com:org/child.git"}}"#,
        )
        .unwrap();

        // Create child directory with its own .meta
        let child_dir = temp_dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        std::fs::write(
            child_dir.join(".meta"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            recursive: true,
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => {
                let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
                let projects = parsed["projects"].as_array().unwrap();
                assert_eq!(projects.len(), 1);
                assert_eq!(projects[0]["name"], "child");
                assert_eq!(projects[0]["is_meta"], true);
                let sub_projects = projects[0]["projects"].as_array().unwrap();
                assert_eq!(sub_projects.len(), 1);
                assert_eq!(sub_projects[0]["name"], "grandchild");
            }
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_format_project_tree() {
        let nodes = vec![
            ProjectTreeNode {
                name: "api".to_string(),
                path: "services/api".to_string(),
                repo: Some("git@github.com:org/api.git".to_string()),
                tags: vec!["backend".to_string()],
                is_meta: false,
                projects: vec![],
            },
            ProjectTreeNode {
                name: "frontend".to_string(),
                path: "frontend".to_string(),
                repo: Some("git@github.com:org/frontend.git".to_string()),
                tags: vec![],
                is_meta: false,
                projects: vec![],
            },
        ];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert!(output.contains("api"));
        assert!(output.contains("services/api"));
        assert!(output.contains("[backend]"));
        assert!(output.contains("frontend"));
        // Last item uses └──
        assert!(output.contains("\u{2514}\u{2500}\u{2500}"));
        // Non-last item uses ├──
        assert!(output.contains("\u{251c}\u{2500}\u{2500}"));
    }
}
